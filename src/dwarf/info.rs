//! `.debug_info` — compilation units and their DIE trees.

use crate::dwarf::abbrev;
use crate::dwarf::consts::*;
use crate::dwarf::reader::{Reader, str_at};
use crate::error::{LdbError, Result};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Addr(u64),
    Unsigned(u64),
    Signed(i64),
    Str(String),
    /// A reference, already resolved to an offset from the start of `.debug_info`.
    Ref(usize),
    Block(Vec<u8>),
    Flag(bool),
    SecOffset(u64),
}

impl AttrValue {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            AttrValue::Addr(v) | AttrValue::Unsigned(v) | AttrValue::SecOffset(v) => Some(*v),
            AttrValue::Signed(v) => Some(*v as u64),
            AttrValue::Flag(b) => Some(u64::from(*b)),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_ref_offset(&self) -> Option<usize> {
        match self {
            AttrValue::Ref(o) => Some(*o),
            _ => None,
        }
    }

    pub fn as_block(&self) -> Option<&[u8]> {
        match self {
            AttrValue::Block(b) => Some(b.as_slice()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Die {
    /// Offset from the start of `.debug_info`; this is what `DW_FORM_ref*` names.
    pub offset: usize,
    pub tag: u64,
    pub attrs: Vec<(u64, AttrValue)>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
}

impl Die {
    pub fn attr(&self, at: u64) -> Option<&AttrValue> {
        self.attrs.iter().find(|(a, _)| *a == at).map(|(_, v)| v)
    }

    pub fn name(&self) -> Option<&str> {
        self.attr(DW_AT_NAME).and_then(AttrValue::as_str)
    }
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub offset: usize,
    pub version: u16,
    pub address_size: u8,
    pub dwarf64: bool,
    pub root: usize,
    pub dies: HashMap<usize, Die>,
    /// DIE offsets in the order they appear, so listings are emission-ordered.
    pub order: Vec<usize>,
}

impl Unit {
    pub fn die(&self, offset: usize) -> Option<&Die> {
        self.dies.get(&offset)
    }

    pub fn root_die(&self) -> &Die {
        &self.dies[&self.root]
    }
}

pub struct Sections<'a> {
    pub info: &'a [u8],
    pub abbrev: &'a [u8],
    pub str_: &'a [u8],
    pub line_str: &'a [u8],
}

pub fn parse_units(sections: &Sections<'_>) -> Result<Vec<Unit>> {
    let mut units = Vec::new();
    let mut r = Reader::new(sections.info);
    while !r.done() && r.remaining() > 4 {
        let unit_start = r.pos();
        let (unit_len, dwarf64) = r.initial_length()?;
        let after_header = r.pos();
        let unit_end = usize::try_from(unit_len)
            .ok()
            .and_then(|n| after_header.checked_add(n))
            .unwrap_or(usize::MAX);
        if unit_end > sections.info.len() {
            return Err(LdbError::DwarfError(format!(
                "compilation unit at {:#x} claims {} bytes, section has {}",
                unit_start,
                unit_len,
                sections.info.len() - after_header
            )));
        }
        units.push(parse_unit(sections, unit_start, unit_end, dwarf64)?);
        r.set_pos(unit_end);
    }
    Ok(units)
}

fn parse_unit(
    sections: &Sections<'_>,
    unit_start: usize,
    unit_end: usize,
    dwarf64: bool,
) -> Result<Unit> {
    let mut r = Reader::at(&sections.info[..unit_end], unit_start);
    let (_len, _) = r.initial_length()?;
    let version = r.u16()?;
    let (abbrev_off, address_size) = match version {
        2..=4 => {
            let abbrev_off = r.offset(dwarf64)?;
            (abbrev_off, r.u8()?)
        }
        5 => {
            let unit_type = r.u8()?;
            if unit_type != 0x01 {
                return Err(LdbError::Unsupported(format!(
                    "DWARF 5 unit type {:#x}; ldb reads DW_UT_compile only",
                    unit_type
                )));
            }
            let address_size = r.u8()?;
            (r.offset(dwarf64)?, address_size)
        }
        v => {
            return Err(LdbError::Unsupported(format!(
                "DWARF version {} in .debug_info; ldb reads 2 through 5",
                v
            )));
        }
    };
    if address_size != 8 {
        return Err(LdbError::Unsupported(format!(
            "address size {}; ldb is x86_64 only",
            address_size
        )));
    }

    let table = abbrev::parse_table(sections.abbrev, abbrev_off as usize)?;

    let mut unit = Unit {
        offset: unit_start,
        version,
        address_size,
        dwarf64,
        root: 0,
        dies: HashMap::new(),
        order: Vec::new(),
    };
    let mut stack: Vec<usize> = Vec::new();
    let mut first = true;

    while r.pos() < unit_end {
        let die_offset = r.pos();
        let code = r.uleb()?;
        if code == 0 {
            stack.pop();
            if stack.is_empty() && !first {
                break;
            }
            continue;
        }
        let ab = table.get(&code).ok_or_else(|| {
            LdbError::DwarfError(format!(
                "abbreviation code {} at {:#x} is not in the table at {:#x}",
                code, die_offset, abbrev_off
            ))
        })?;
        let attrs = read_attrs(sections, &mut r, ab, unit_start, dwarf64, address_size)?;
        let parent = stack.last().copied();
        let die = Die {
            offset: die_offset,
            tag: ab.tag,
            attrs,
            children: Vec::new(),
            parent,
        };
        if let Some(p) = parent
            && let Some(pd) = unit.dies.get_mut(&p)
        {
            pd.children.push(die_offset);
        }
        if first {
            unit.root = die_offset;
            first = false;
        }
        unit.order.push(die_offset);
        let has_children = ab.has_children;
        unit.dies.insert(die_offset, die);
        if has_children {
            stack.push(die_offset);
        } else if stack.is_empty() {
            break;
        }
    }
    if unit.dies.is_empty() {
        return Err(LdbError::DwarfError(format!(
            "compilation unit at {:#x} declares no DIE, not even a root",
            unit_start
        )));
    }
    Ok(unit)
}

fn read_attrs(
    sections: &Sections<'_>,
    r: &mut Reader<'_>,
    ab: &abbrev::Abbrev,
    unit_start: usize,
    dwarf64: bool,
    address_size: u8,
) -> Result<Vec<(u64, AttrValue)>> {
    let mut out = Vec::with_capacity(ab.attrs.len());
    for spec in &ab.attrs {
        let value = read_form(
            sections,
            r,
            spec.form,
            spec.implicit_const,
            unit_start,
            dwarf64,
            address_size,
        )?;
        out.push((spec.attr, value));
    }
    Ok(out)
}

fn read_form(
    sections: &Sections<'_>,
    r: &mut Reader<'_>,
    form: u64,
    implicit_const: i64,
    unit_start: usize,
    dwarf64: bool,
    address_size: u8,
) -> Result<AttrValue> {
    let v = match form {
        DW_FORM_ADDR => AttrValue::Addr(match address_size {
            8 => r.u64()?,
            4 => u64::from(r.u32()?),
            n => {
                return Err(LdbError::Unsupported(format!("address size {}", n)));
            }
        }),
        DW_FORM_DATA1 => AttrValue::Unsigned(u64::from(r.u8()?)),
        DW_FORM_DATA2 => AttrValue::Unsigned(u64::from(r.u16()?)),
        DW_FORM_DATA4 => AttrValue::Unsigned(u64::from(r.u32()?)),
        DW_FORM_DATA8 => AttrValue::Unsigned(r.u64()?),
        DW_FORM_UDATA => AttrValue::Unsigned(r.uleb()?),
        DW_FORM_SDATA => AttrValue::Signed(r.sleb()?),
        DW_FORM_STRING => AttrValue::Str(r.cstr()?.to_string()),
        DW_FORM_STRP => {
            let off = r.offset(dwarf64)? as usize;
            AttrValue::Str(str_at(sections.str_, off)?.to_string())
        }
        0x1f => {
            // DW_FORM_line_strp
            let off = r.offset(dwarf64)? as usize;
            AttrValue::Str(str_at(sections.line_str, off)?.to_string())
        }
        DW_FORM_FLAG => AttrValue::Flag(r.u8()? != 0),
        DW_FORM_FLAG_PRESENT => AttrValue::Flag(true),
        DW_FORM_IMPLICIT_CONST => AttrValue::Signed(implicit_const),
        DW_FORM_SEC_OFFSET => AttrValue::SecOffset(r.offset(dwarf64)?),
        DW_FORM_REF1 => AttrValue::Ref(unit_start.saturating_add(usize::from(r.u8()?))),
        DW_FORM_REF2 => AttrValue::Ref(unit_start.saturating_add(usize::from(r.u16()?))),
        DW_FORM_REF4 => AttrValue::Ref(unit_start.saturating_add(r.u32()? as usize)),
        DW_FORM_REF8 => AttrValue::Ref(unit_start.saturating_add(r.u64()? as usize)),
        DW_FORM_REF_UDATA => AttrValue::Ref(unit_start.saturating_add(r.uleb()? as usize)),
        DW_FORM_REF_ADDR => AttrValue::Ref(r.offset(dwarf64)? as usize),
        DW_FORM_EXPRLOC | DW_FORM_BLOCK => {
            let n = r.uleb()? as usize;
            AttrValue::Block(r.bytes(n)?.to_vec())
        }
        DW_FORM_BLOCK1 => {
            let n = usize::from(r.u8()?);
            AttrValue::Block(r.bytes(n)?.to_vec())
        }
        DW_FORM_BLOCK2 => {
            let n = usize::from(r.u16()?);
            AttrValue::Block(r.bytes(n)?.to_vec())
        }
        DW_FORM_BLOCK4 => {
            let n = r.u32()? as usize;
            AttrValue::Block(r.bytes(n)?.to_vec())
        }
        DW_FORM_INDIRECT => {
            let actual = r.uleb()?;
            return read_form(sections, r, actual, 0, unit_start, dwarf64, address_size);
        }
        other => {
            return Err(LdbError::Unsupported(format!(
                "{} ({:#x}) in .debug_info; ldb does not read it yet",
                form_name(other),
                other
            )));
        }
    };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections<'a>(info: &'a [u8], abbrev: &'a [u8]) -> Sections<'a> {
        Sections {
            info,
            abbrev,
            str_: &[],
            line_str: &[],
        }
    }

    /// A single-CU `.debug_info` shaped the way ras emits it: DWARF 4, inline
    /// strings, `DW_AT_high_pc` as an offset in `DW_FORM_data8`.
    fn fixture() -> (Vec<u8>, Vec<u8>) {
        let abbrev = vec![
            0x01, 0x11, 0x01, // compile_unit, has children
            0x03, 0x08, // name, string
            0x11, 0x01, // low_pc, addr
            0x00, 0x00, //
            0x02, 0x2e, 0x01, // subprogram, has children
            0x03, 0x08, // name, string
            0x3b, 0x0f, // decl_line, udata
            0x49, 0x13, // type, ref4
            0x11, 0x01, // low_pc, addr
            0x12, 0x07, // high_pc, data8
            0x00, 0x00, //
            0x03, 0x05, 0x00, // formal_parameter, no children
            0x03, 0x08, // name, string
            0x00, 0x00, //
            0x04, 0x24, 0x00, // base_type, no children
            0x03, 0x08, 0x3e, 0x0b, 0x0b, 0x0b, 0x00, 0x00, //
            0x00,
        ];
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(&4u16.to_le_bytes()); // version
        body.extend_from_slice(&0u32.to_le_bytes()); // abbrev offset
        body.push(8); // address size
        body.push(0x01); // compile_unit
        body.extend_from_slice(b"t.lamina\0");
        body.extend_from_slice(&0x1000u64.to_le_bytes());
        let base_type_off = body.len() + 4; // +4 for the initial length field
        body.push(0x04); // base_type
        body.extend_from_slice(b"i64\0");
        body.push(5); // DW_ATE_signed
        body.push(8);
        body.push(0x02); // subprogram
        body.extend_from_slice(b"f\0");
        body.push(7); // decl_line
        body.extend_from_slice(&(base_type_off as u32).to_le_bytes());
        body.extend_from_slice(&0x1000u64.to_le_bytes());
        body.extend_from_slice(&0x40u64.to_le_bytes());
        body.push(0x03); // formal_parameter
        body.extend_from_slice(b"n\0");
        body.push(0x00); // end subprogram children
        body.push(0x00); // end compile_unit children
        let mut info = (body.len() as u32).to_le_bytes().to_vec();
        info.extend_from_slice(&body);
        (info, abbrev)
    }

    #[test]
    fn reads_the_die_tree() {
        let (info, abbrev) = fixture();
        let units = parse_units(&sections(&info, &abbrev)).unwrap();
        assert_eq!(units.len(), 1);
        let u = &units[0];
        assert_eq!(u.version, 4);
        assert_eq!(u.root_die().tag, DW_TAG_COMPILE_UNIT);
        assert_eq!(u.root_die().name(), Some("t.lamina"));
        assert_eq!(u.root_die().children.len(), 2);

        let sub = u
            .order
            .iter()
            .map(|o| &u.dies[o])
            .find(|d| d.tag == DW_TAG_SUBPROGRAM)
            .unwrap();
        assert_eq!(sub.name(), Some("f"));
        assert_eq!(sub.attr(DW_AT_DECL_LINE).unwrap().as_u64(), Some(7));
        assert_eq!(sub.attr(DW_AT_LOW_PC).unwrap().as_u64(), Some(0x1000));
        assert_eq!(sub.attr(DW_AT_HIGH_PC).unwrap().as_u64(), Some(0x40));
        assert_eq!(sub.children.len(), 1);

        let param = &u.dies[&sub.children[0]];
        assert_eq!(param.tag, DW_TAG_FORMAL_PARAMETER);
        assert_eq!(param.name(), Some("n"));
        assert_eq!(param.parent, Some(sub.offset));
    }

    #[test]
    fn ref4_resolves_within_the_unit() {
        let (info, abbrev) = fixture();
        let units = parse_units(&sections(&info, &abbrev)).unwrap();
        let u = &units[0];
        let sub = u
            .order
            .iter()
            .map(|o| &u.dies[o])
            .find(|d| d.tag == DW_TAG_SUBPROGRAM)
            .unwrap();
        let ty = u
            .die(sub.attr(DW_AT_TYPE).unwrap().as_ref_offset().unwrap())
            .unwrap();
        assert_eq!(ty.tag, DW_TAG_BASE_TYPE);
        assert_eq!(ty.name(), Some("i64"));
        assert_eq!(ty.attr(DW_AT_ENCODING).unwrap().as_u64(), Some(5));
        assert_eq!(ty.attr(DW_AT_BYTE_SIZE).unwrap().as_u64(), Some(8));
    }

    #[test]
    fn rejects_an_unknown_abbreviation_code() {
        let (mut info, abbrev) = fixture();
        info[11] = 0x09;
        let err = parse_units(&sections(&info, &abbrev)).unwrap_err();
        assert!(format!("{}", err).contains("abbreviation code 9"));
    }

    #[test]
    fn a_compilation_unit_of_an_unknown_version_is_refused_by_number() {
        let mut info_sec = Vec::new();
        info_sec.extend_from_slice(&7u32.to_le_bytes());
        info_sec.extend_from_slice(&99u16.to_le_bytes());
        info_sec.extend_from_slice(&0u32.to_le_bytes());
        info_sec.push(8);
        let err = parse_units(&sections(&info_sec, &[])).unwrap_err();
        assert!(format!("{err}").contains("DWARF version 99"), "{err}");
    }
    #[test]
    fn a_unit_longer_than_the_section_is_refused() {
        let mut info_sec = Vec::new();
        info_sec.extend_from_slice(&u32::MAX.wrapping_sub(0x20).to_le_bytes());
        info_sec.extend_from_slice(&4u16.to_le_bytes());
        let err = parse_units(&sections(&info_sec, &[])).unwrap_err();
        assert!(format!("{err}").contains("section has"), "{err}");
    }
    #[test]
    fn a_unit_with_no_die_at_all_is_refused_rather_than_indexed_into() {
        let mut body = Vec::new();
        body.extend_from_slice(&4u16.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(8);
        body.extend_from_slice(&[0, 0, 0, 0]); // nothing but padding where DIEs go
        let mut info_sec = (body.len() as u32).to_le_bytes().to_vec();
        info_sec.extend_from_slice(&body);
        let err = parse_units(&sections(&info_sec, &[])).unwrap_err();
        assert!(format!("{err}").contains("no DIE"), "{err}");
    }
}
