//! `.debug_abbrev` — the declaration table each compilation unit indexes into.

use crate::dwarf::consts::DW_FORM_IMPLICIT_CONST;
use crate::dwarf::reader::Reader;
use crate::error::Result;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct AttrSpec {
    pub attr: u64,
    pub form: u64,
    /// `DW_FORM_implicit_const` keeps the value in the abbreviation, not the DIE
    /// (DWARF 5 section 7.5.3).
    pub implicit_const: i64,
}

#[derive(Debug, Clone)]
pub struct Abbrev {
    pub tag: u64,
    pub has_children: bool,
    pub attrs: Vec<AttrSpec>,
}

pub type AbbrevTable = HashMap<u64, Abbrev>;

/// Parse the abbreviation table that starts at `offset` in `.debug_abbrev`.
pub fn parse_table(section: &[u8], offset: usize) -> Result<AbbrevTable> {
    let mut r = Reader::at(section, offset);
    let mut table = AbbrevTable::new();
    loop {
        if r.done() {
            break;
        }
        let code = r.uleb()?;
        if code == 0 {
            break;
        }
        let tag = r.uleb()?;
        let has_children = r.u8()? != 0;
        let mut attrs = Vec::new();
        loop {
            let attr = r.uleb()?;
            let form = r.uleb()?;
            let implicit_const = if form == DW_FORM_IMPLICIT_CONST {
                r.sleb()?
            } else {
                0
            };
            if attr == 0 && form == 0 {
                break;
            }
            attrs.push(AttrSpec {
                attr,
                form,
                implicit_const,
            });
        }
        table.insert(
            code,
            Abbrev {
                tag,
                has_children,
                attrs,
            },
        );
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dwarf::consts::{
        DW_AT_NAME, DW_FORM_STRING, DW_TAG_COMPILE_UNIT, DW_TAG_SUBPROGRAM,
    };

    #[test]
    fn parses_two_declarations() {
        let bytes = [
            0x01, 0x11, 0x01, 0x03, 0x08, 0x00,
            0x00, // 1: compile_unit, children, name/string
            0x02, 0x2e, 0x00, 0x03, 0x08, 0x00, 0x00, // 2: subprogram, no children
            0x00,
        ];
        let table = parse_table(&bytes, 0).unwrap();
        assert_eq!(table.len(), 2);
        let cu = &table[&1];
        assert_eq!(cu.tag, DW_TAG_COMPILE_UNIT);
        assert!(cu.has_children);
        assert_eq!(cu.attrs.len(), 1);
        assert_eq!(cu.attrs[0].attr, DW_AT_NAME);
        assert_eq!(cu.attrs[0].form, DW_FORM_STRING);
        assert_eq!(table[&2].tag, DW_TAG_SUBPROGRAM);
        assert!(!table[&2].has_children);
    }

    #[test]
    fn stops_at_the_terminator() {
        let bytes = [0x00, 0x01, 0x11, 0x01, 0x00, 0x00];
        assert!(parse_table(&bytes, 0).unwrap().is_empty());
    }
}
