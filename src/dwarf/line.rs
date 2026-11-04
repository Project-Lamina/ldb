//! `.debug_line` — the line number program, run to produce a sorted row table.

use crate::dwarf::reader::Reader;
use crate::error::{LdbError, Result};

const DW_LNS_COPY: u8 = 0x01;
const DW_LNS_ADVANCE_PC: u8 = 0x02;
const DW_LNS_ADVANCE_LINE: u8 = 0x03;
const DW_LNS_SET_FILE: u8 = 0x04;
const DW_LNS_SET_COLUMN: u8 = 0x05;
const DW_LNS_NEGATE_STMT: u8 = 0x06;
const DW_LNS_SET_BASIC_BLOCK: u8 = 0x07;
const DW_LNS_CONST_ADD_PC: u8 = 0x08;
const DW_LNS_FIXED_ADVANCE_PC: u8 = 0x09;
const DW_LNS_SET_PROLOGUE_END: u8 = 0x0a;
const DW_LNS_SET_EPILOGUE_BEGIN: u8 = 0x0b;
const DW_LNS_SET_ISA: u8 = 0x0c;

const DW_LNE_END_SEQUENCE: u8 = 0x01;
const DW_LNE_SET_ADDRESS: u8 = 0x02;
const DW_LNE_DEFINE_FILE: u8 = 0x03;
const DW_LNE_SET_DISCRIMINATOR: u8 = 0x04;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub address: u64,
    pub file: u64,
    pub line: u64,
    pub column: u64,
    pub is_stmt: bool,
    pub prologue_end: bool,
    /// The row that closes a sequence. It marks the end address and carries no
    /// source position (DWARF 4 section 6.2.5.3).
    pub end_sequence: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LineProgram {
    pub version: u16,
    /// File names indexed the way this unit's version indexes them: DWARF 2-4
    /// number files from 1, DWARF 5 from 0.
    pub files: Vec<String>,
    pub file_base: u64,
    pub rows: Vec<Row>,
}

impl LineProgram {
    pub fn file_name(&self, index: u64) -> Option<&str> {
        let i = index.checked_sub(self.file_base)?;
        self.files.get(i as usize).map(String::as_str)
    }

    /// The row covering `pc`, meaning the last row at or below it inside a
    /// sequence that has not yet ended.
    pub fn row_for_pc(&self, pc: u64) -> Option<&Row> {
        let idx = match self.rows.binary_search_by(|r| r.address.cmp(&pc)) {
            Ok(mut i) => {
                while i + 1 < self.rows.len() && self.rows[i + 1].address == pc {
                    i += 1;
                }
                i
            }
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let row = &self.rows[idx];
        if row.end_sequence { None } else { Some(row) }
    }

    /// The first address at which `file`/`line` begins a row.
    pub fn address_for_line(&self, file: u64, line: u64) -> Option<u64> {
        self.rows
            .iter()
            .filter(|r| !r.end_sequence && r.file == file && r.line == line)
            .map(|r| r.address)
            .min()
    }

    /// The lowest row address strictly inside `[low, high)`, which is where a
    /// function's body begins when the producer emits no `DW_LNS_set_prologue_end`.
    pub fn first_row_in(&self, low: u64, high: u64) -> Option<&Row> {
        self.rows
            .iter()
            .filter(|r| !r.end_sequence && r.address >= low && r.address < high)
            .min_by_key(|r| r.address)
    }

    pub fn covers(&self, pc: u64) -> bool {
        self.row_for_pc(pc).is_some()
    }
}

struct State {
    address: u64,
    file: u64,
    line: i64,
    column: u64,
    is_stmt: bool,
    prologue_end: bool,
}

impl State {
    fn new(default_is_stmt: bool) -> Self {
        State {
            address: 0,
            file: 1,
            line: 1,
            column: 0,
            is_stmt: default_is_stmt,
            prologue_end: false,
        }
    }
}

pub struct LineSections<'a> {
    pub line: &'a [u8],
    pub line_str: &'a [u8],
    pub str_: &'a [u8],
}

pub fn parse_program(sections: &LineSections<'_>, offset: usize) -> Result<LineProgram> {
    if offset >= sections.line.len() {
        return Err(LdbError::DwarfError(format!(
            "DW_AT_stmt_list points at {:#x}, past the {} byte .debug_line",
            offset,
            sections.line.len()
        )));
    }
    let mut r = Reader::at(sections.line, offset);
    let (unit_len, dwarf64) = r.initial_length()?;
    let unit_end = usize::try_from(unit_len)
        .ok()
        .and_then(|n| r.pos().checked_add(n))
        .unwrap_or(usize::MAX);
    if unit_end > sections.line.len() {
        return Err(LdbError::DwarfError(format!(
            "line program at {:#x} claims {} bytes, .debug_line has {} left",
            offset,
            unit_len,
            sections.line.len() - r.pos()
        )));
    }
    let version = r.u16()?;
    if !(2..=5).contains(&version) {
        return Err(LdbError::Unsupported(format!(
            "DWARF version {} in .debug_line; ldb reads 2 through 5",
            version
        )));
    }
    if version >= 5 {
        let address_size = r.u8()?;
        let _segment_selector_size = r.u8()?;
        if address_size != 8 {
            return Err(LdbError::Unsupported(format!(
                "address size {} in .debug_line; ldb is x86_64 only",
                address_size
            )));
        }
    }
    let header_len = r.offset(dwarf64)?;
    let program_start = usize::try_from(header_len)
        .ok()
        .and_then(|n| r.pos().checked_add(n))
        .filter(|p| *p <= unit_end)
        .ok_or_else(|| {
            LdbError::DwarfError(format!(
                "line program header at {:#x} claims {} bytes, more than the unit holds",
                offset, header_len
            ))
        })?;
    let min_inst_len = r.u8()?;
    let max_ops_per_inst = if version >= 4 { r.u8()? } else { 1 };
    if max_ops_per_inst != 1 {
        return Err(LdbError::Unsupported(format!(
            "maximum_operations_per_instruction {}; ldb has no VLIW support",
            max_ops_per_inst
        )));
    }
    let default_is_stmt = r.u8()? != 0;
    let line_base = i64::from(r.i8()?);
    let line_range = u64::from(r.u8()?);
    let opcode_base = r.u8()?;
    let mut std_opcode_lengths = vec![0u8; usize::from(opcode_base.saturating_sub(1))];
    for slot in &mut std_opcode_lengths {
        *slot = r.u8()?;
    }
    if line_range == 0 {
        return Err(LdbError::DwarfError(
            "line_range is 0; the special opcode formula divides by it".into(),
        ));
    }

    let (files, file_base) = if version >= 5 {
        (read_file_table_v5(sections, &mut r, dwarf64)?, 0)
    } else {
        (read_file_table_v4(&mut r)?, 1)
    };

    let mut program = LineProgram {
        version,
        files,
        file_base,
        rows: Vec::new(),
    };

    r.set_pos(program_start);
    let mut st = State::new(default_is_stmt);

    while r.pos() < unit_end {
        let opcode = r.u8()?;
        if opcode >= opcode_base {
            let adj = u64::from(opcode - opcode_base);
            st.address = st
                .address
                .wrapping_add(u64::from(min_inst_len).wrapping_mul(adj / line_range));
            st.line = st
                .line
                .saturating_add(line_base.saturating_add((adj % line_range) as i64));
            push_row(&mut program, &st, false);
            st.prologue_end = false;
        } else if opcode == 0 {
            let len = r.uleb()? as usize;
            let Some(next) = r.pos().checked_add(len).filter(|n| *n <= unit_end) else {
                return Err(LdbError::DwarfError(format!(
                    "extended line opcode at {:#x} claims {} bytes, past the end of the unit",
                    r.pos(),
                    len
                )));
            };
            let sub = r.u8()?;
            match sub {
                DW_LNE_END_SEQUENCE => {
                    push_row(&mut program, &st, true);
                    st = State::new(default_is_stmt);
                }
                DW_LNE_SET_ADDRESS => st.address = r.u64()?,
                DW_LNE_DEFINE_FILE => {
                    let name = r.cstr()?.to_string();
                    let _dir = r.uleb()?;
                    let _mtime = r.uleb()?;
                    let _size = r.uleb()?;
                    program.files.push(name);
                }
                DW_LNE_SET_DISCRIMINATOR => {
                    let _ = r.uleb()?;
                }
                other => {
                    log::debug!("ignoring extended line opcode {:#x}", other);
                }
            }
            r.set_pos(next);
        } else {
            match opcode {
                DW_LNS_COPY => {
                    push_row(&mut program, &st, false);
                    st.prologue_end = false;
                }
                DW_LNS_ADVANCE_PC => {
                    st.address = st
                        .address
                        .wrapping_add(u64::from(min_inst_len).wrapping_mul(r.uleb()?))
                }
                DW_LNS_ADVANCE_LINE => st.line = st.line.saturating_add(r.sleb()?),
                DW_LNS_SET_FILE => st.file = r.uleb()?,
                DW_LNS_SET_COLUMN => st.column = r.uleb()?,
                DW_LNS_NEGATE_STMT => st.is_stmt = !st.is_stmt,
                DW_LNS_SET_BASIC_BLOCK => {}
                DW_LNS_CONST_ADD_PC => {
                    let adj = u64::from(255 - opcode_base);
                    st.address = st
                        .address
                        .wrapping_add(u64::from(min_inst_len).wrapping_mul(adj / line_range));
                }
                DW_LNS_FIXED_ADVANCE_PC => {
                    st.address = st.address.wrapping_add(u64::from(r.u16()?))
                }
                DW_LNS_SET_PROLOGUE_END => st.prologue_end = true,
                DW_LNS_SET_EPILOGUE_BEGIN => {}
                DW_LNS_SET_ISA => {
                    let _ = r.uleb()?;
                }
                other => {
                    let argc = std_opcode_lengths
                        .get(usize::from(other) - 1)
                        .copied()
                        .unwrap_or(0);
                    for _ in 0..argc {
                        let _ = r.uleb()?;
                    }
                }
            }
        }
    }

    // At one address a sequence's DW_LNE_end_sequence row can sit alongside the
    // first row of the next sequence. Sorting the terminator first leaves the
    // row that actually covers the address last, which is the one to report.
    program
        .rows
        .sort_by_key(|r| (r.address, std::cmp::Reverse(r.end_sequence)));
    Ok(program)
}

fn push_row(program: &mut LineProgram, st: &State, end_sequence: bool) {
    program.rows.push(Row {
        address: st.address,
        file: st.file,
        line: st.line.max(0) as u64,
        column: st.column,
        is_stmt: st.is_stmt,
        prologue_end: st.prologue_end,
        end_sequence,
    });
}

fn read_file_table_v4(r: &mut Reader<'_>) -> Result<Vec<String>> {
    loop {
        let s = r.cstr()?;
        if s.is_empty() {
            break;
        }
    }
    let mut files = Vec::new();
    loop {
        let name = r.cstr()?;
        if name.is_empty() {
            break;
        }
        let name = name.to_string();
        let _dir = r.uleb()?;
        let _mtime = r.uleb()?;
        let _size = r.uleb()?;
        files.push(name);
    }
    Ok(files)
}

fn read_file_table_v5(
    sections: &LineSections<'_>,
    r: &mut Reader<'_>,
    dwarf64: bool,
) -> Result<Vec<String>> {
    read_entry_list_v5(sections, r, dwarf64)?;
    read_entry_list_v5(sections, r, dwarf64)
}

fn read_entry_list_v5(
    sections: &LineSections<'_>,
    r: &mut Reader<'_>,
    dwarf64: bool,
) -> Result<Vec<String>> {
    let format_count = r.u8()?;
    let mut formats = Vec::with_capacity(usize::from(format_count));
    for _ in 0..format_count {
        formats.push((r.uleb()?, r.uleb()?));
    }
    let count = r.uleb()?;
    if count > r.remaining() as u64 {
        return Err(LdbError::DwarfError(format!(
            "line header entry list claims {} entries, more than the {} bytes left",
            count,
            r.remaining()
        )));
    }
    let mut names = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut name = String::new();
        for &(content_type, form) in &formats {
            let value = read_line_form(sections, r, form, dwarf64)?;
            if content_type == 0x01 {
                name = value;
            }
        }
        names.push(name);
    }
    Ok(names)
}

fn read_line_form(
    sections: &LineSections<'_>,
    r: &mut Reader<'_>,
    form: u64,
    dwarf64: bool,
) -> Result<String> {
    use crate::dwarf::consts::*;
    use crate::dwarf::reader::str_at;
    let s = match form {
        DW_FORM_STRING => r.cstr()?.to_string(),
        DW_FORM_STRP => str_at(sections.str_, r.offset(dwarf64)? as usize)?.to_string(),
        0x1f => str_at(sections.line_str, r.offset(dwarf64)? as usize)?.to_string(),
        DW_FORM_UDATA => {
            let _ = r.uleb()?;
            String::new()
        }
        DW_FORM_DATA1 => {
            let _ = r.u8()?;
            String::new()
        }
        DW_FORM_DATA2 => {
            let _ = r.u16()?;
            String::new()
        }
        DW_FORM_DATA4 => {
            let _ = r.u32()?;
            String::new()
        }
        DW_FORM_DATA8 => {
            let _ = r.u64()?;
            String::new()
        }
        0x1e => {
            // DW_FORM_data16, the MD5 slot
            r.skip(16)?;
            String::new()
        }
        DW_FORM_BLOCK => {
            let n = r.uleb()? as usize;
            r.skip(n)?;
            String::new()
        }
        other => {
            return Err(LdbError::Unsupported(format!(
                "{} ({:#x}) in the .debug_line file table",
                crate::dwarf::consts::form_name(other),
                other
            )));
        }
    };
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(line: &[u8]) -> LineSections<'_> {
        LineSections {
            line,
            line_str: &[],
            str_: &[],
        }
    }

    /// A DWARF 4 line program with the header shape ras emits: line_base -5,
    /// line_range 14, opcode_base 13, one file, no directories.
    fn fixture() -> Vec<u8> {
        let mut header: Vec<u8> = vec![
            1,            // minimum_instruction_length
            1,            // maximum_operations_per_instruction
            1,            // default_is_stmt
            (-5i8) as u8, // line_base
            14,           // line_range
            13,           // opcode_base
        ];
        header.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
        header.push(0); // empty directory table
        header.extend_from_slice(b"t.lamina\0");
        header.extend_from_slice(&[0, 0, 0]); // dir, mtime, size
        header.push(0); // end of file table

        let mut prog: Vec<u8> = Vec::new();
        prog.extend_from_slice(&[0x00, 0x09, 0x02]); // DW_LNE_set_address
        prog.extend_from_slice(&0x1000u64.to_le_bytes());
        prog.push(DW_LNS_COPY);
        prog.push(DW_LNS_ADVANCE_PC);
        prog.push(0x10);
        prog.push(DW_LNS_ADVANCE_LINE);
        prog.push(0x04);
        prog.push(DW_LNS_COPY);
        prog.push(DW_LNS_ADVANCE_PC);
        prog.push(0x10);
        prog.extend_from_slice(&[0x00, 0x01, DW_LNE_END_SEQUENCE]);

        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(&4u16.to_le_bytes());
        body.extend_from_slice(&(header.len() as u32).to_le_bytes());
        body.extend_from_slice(&header);
        body.extend_from_slice(&prog);
        let mut out = (body.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn runs_the_state_machine() {
        let bytes = fixture();
        let p = parse_program(&sections(&bytes), 0).unwrap();
        assert_eq!(p.version, 4);
        assert_eq!(p.files, vec!["t.lamina".to_string()]);
        assert_eq!(p.file_name(1), Some("t.lamina"));
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.rows[0].address, 0x1000);
        assert_eq!(p.rows[0].line, 1);
        assert!(p.rows[0].is_stmt);
        assert_eq!(p.rows[1].address, 0x1010);
        assert_eq!(p.rows[1].line, 5);
        assert!(p.rows[2].end_sequence);
        assert_eq!(p.rows[2].address, 0x1020);
    }

    #[test]
    fn pc_lookup_uses_the_covering_row() {
        let bytes = fixture();
        let p = parse_program(&sections(&bytes), 0).unwrap();
        assert!(p.row_for_pc(0x0fff).is_none());
        assert_eq!(p.row_for_pc(0x1000).unwrap().line, 1);
        assert_eq!(p.row_for_pc(0x1008).unwrap().line, 1);
        assert_eq!(p.row_for_pc(0x1010).unwrap().line, 5);
        assert_eq!(p.row_for_pc(0x101f).unwrap().line, 5);
        assert!(
            p.row_for_pc(0x1020).is_none(),
            "end_sequence covers nothing"
        );
        assert!(p.row_for_pc(0x2000).is_none());
    }

    #[test]
    fn line_lookup_takes_the_lowest_address() {
        let bytes = fixture();
        let p = parse_program(&sections(&bytes), 0).unwrap();
        assert_eq!(p.address_for_line(1, 5), Some(0x1010));
        assert_eq!(p.address_for_line(1, 99), None);
    }

    /// ras starts a new sequence at the address where the previous one ends, so
    /// two rows share that address: the terminator and the live row.
    #[test]
    fn a_new_sequence_starting_at_an_end_sequence_still_covers_it() {
        let mut p = LineProgram {
            version: 4,
            files: vec!["t.lamina".into()],
            file_base: 1,
            rows: vec![
                Row {
                    address: 0x2000,
                    file: 1,
                    line: 28,
                    column: 0,
                    is_stmt: true,
                    prologue_end: false,
                    end_sequence: false,
                },
                Row {
                    address: 0x2000,
                    file: 1,
                    line: 0,
                    column: 0,
                    is_stmt: true,
                    prologue_end: false,
                    end_sequence: true,
                },
            ],
        };
        p.rows
            .sort_by_key(|r| (r.address, std::cmp::Reverse(r.end_sequence)));
        assert_eq!(p.row_for_pc(0x2000).map(|r| r.line), Some(28));
        assert_eq!(p.row_for_pc(0x2004).map(|r| r.line), Some(28));
    }

    #[test]
    fn refuses_a_stmt_list_past_the_section() {
        let bytes = fixture();
        let err = parse_program(&sections(&bytes), 9999).unwrap_err();
        assert!(format!("{}", err).contains("past the"));
    }
}
