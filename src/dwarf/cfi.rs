//! `.eh_frame` — call frame information, read far enough to unwind x86_64.

use crate::dwarf::reader::Reader;
use crate::error::{LdbError, Result};
use std::collections::HashMap;

const DW_CFA_ADVANCE_LOC: u8 = 0x40;
const DW_CFA_OFFSET: u8 = 0x80;
const DW_CFA_RESTORE: u8 = 0xc0;

const DW_CFA_NOP: u8 = 0x00;
const DW_CFA_SET_LOC: u8 = 0x01;
const DW_CFA_ADVANCE_LOC1: u8 = 0x02;
const DW_CFA_ADVANCE_LOC2: u8 = 0x03;
const DW_CFA_ADVANCE_LOC4: u8 = 0x04;
const DW_CFA_OFFSET_EXTENDED: u8 = 0x05;
const DW_CFA_RESTORE_EXTENDED: u8 = 0x06;
const DW_CFA_UNDEFINED: u8 = 0x07;
const DW_CFA_SAME_VALUE: u8 = 0x08;
const DW_CFA_REGISTER: u8 = 0x09;
const DW_CFA_REMEMBER_STATE: u8 = 0x0a;
const DW_CFA_RESTORE_STATE: u8 = 0x0b;
const DW_CFA_DEF_CFA: u8 = 0x0c;
const DW_CFA_DEF_CFA_REGISTER: u8 = 0x0d;
const DW_CFA_DEF_CFA_OFFSET: u8 = 0x0e;
const DW_CFA_DEF_CFA_EXPRESSION: u8 = 0x0f;
const DW_CFA_EXPRESSION: u8 = 0x10;
const DW_CFA_OFFSET_EXTENDED_SF: u8 = 0x11;
const DW_CFA_DEF_CFA_SF: u8 = 0x12;
const DW_CFA_DEF_CFA_OFFSET_SF: u8 = 0x13;
const DW_CFA_VAL_OFFSET: u8 = 0x14;
const DW_CFA_VAL_OFFSET_SF: u8 = 0x15;
const DW_CFA_VAL_EXPRESSION: u8 = 0x16;
const DW_CFA_GNU_ARGS_SIZE: u8 = 0x2e;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegRule {
    Undefined,
    SameValue,
    /// Saved at `CFA + offset`.
    Offset(i64),
    /// Its value is `CFA + offset`, not a memory location.
    ValOffset(i64),
    /// Held in another register.
    Register(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfaRule {
    Undefined,
    RegisterOffset { reg: u64, offset: i64 },
}

#[derive(Debug, Clone)]
pub struct Row {
    pub cfa: CfaRule,
    pub regs: HashMap<u64, RegRule>,
    pub return_address_register: u64,
}

#[derive(Debug, Clone)]
struct Cie {
    code_alignment: u64,
    data_alignment: i64,
    return_address_register: u64,
    fde_pointer_encoding: u8,
    has_z: bool,
    instructions: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Fde {
    pub initial_location: u64,
    pub address_range: u64,
    cie: usize,
    instructions: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct EhFrame {
    cies: Vec<Cie>,
    fdes: Vec<Fde>,
}

impl EhFrame {
    pub fn fdes(&self) -> &[Fde] {
        &self.fdes
    }

    pub fn fde_for_pc(&self, pc: u64) -> Option<&Fde> {
        self.fdes
            .iter()
            .find(|f| pc >= f.initial_location && pc - f.initial_location < f.address_range.max(1))
    }

    /// Run the CIE's initial instructions and then the FDE's, stopping at `pc`.
    pub fn row_for_pc(&self, pc: u64) -> Result<Option<Row>> {
        let Some(fde) = self.fde_for_pc(pc) else {
            return Ok(None);
        };
        let cie = &self.cies[fde.cie];
        let mut row = Row {
            cfa: CfaRule::Undefined,
            regs: HashMap::new(),
            return_address_register: cie.return_address_register,
        };
        let mut loc = fde.initial_location;
        run(&cie.instructions, cie, &mut row, &mut loc, u64::MAX)?;
        let initial = row.clone();
        let mut loc = fde.initial_location;
        run_with_initial(
            &fde.instructions,
            cie,
            &mut row,
            &mut loc,
            pc,
            &initial.regs,
        )?;
        Ok(Some(row))
    }
}

/// `location` plus `delta * code_alignment`. Both factors come from the
/// section, so the product is saturated rather than allowed to wrap.
fn advance(loc: u64, delta: u64, code_alignment: u64) -> u64 {
    loc.saturating_add(delta.saturating_mul(code_alignment))
}

/// A factored offset times the data alignment factor, both from the section.
fn scale(factored: i64, data_alignment: i64) -> i64 {
    factored.saturating_mul(data_alignment)
}

fn run(insns: &[u8], cie: &Cie, row: &mut Row, loc: &mut u64, stop_pc: u64) -> Result<()> {
    let initial = HashMap::new();
    run_with_initial(insns, cie, row, loc, stop_pc, &initial)
}

fn run_with_initial(
    insns: &[u8],
    cie: &Cie,
    row: &mut Row,
    loc: &mut u64,
    stop_pc: u64,
    initial: &HashMap<u64, RegRule>,
) -> Result<()> {
    let mut r = Reader::new(insns);
    let mut stack: Vec<(CfaRule, HashMap<u64, RegRule>)> = Vec::new();
    while !r.done() {
        let op = r.u8()?;
        let high = op & 0xc0;
        let low = op & 0x3f;
        match high {
            DW_CFA_ADVANCE_LOC => {
                *loc = advance(*loc, u64::from(low), cie.code_alignment);
                if *loc > stop_pc {
                    return Ok(());
                }
                continue;
            }
            DW_CFA_OFFSET => {
                let factored = r.uleb()?;
                row.regs.insert(
                    u64::from(low),
                    RegRule::Offset(scale(factored as i64, cie.data_alignment)),
                );
                continue;
            }
            DW_CFA_RESTORE => {
                let reg = u64::from(low);
                match initial.get(&reg) {
                    Some(rule) => {
                        row.regs.insert(reg, *rule);
                    }
                    None => {
                        row.regs.remove(&reg);
                    }
                }
                continue;
            }
            _ => {}
        }
        match op {
            DW_CFA_NOP => {}
            DW_CFA_SET_LOC => {
                *loc = r.u64()?;
                if *loc > stop_pc {
                    return Ok(());
                }
            }
            DW_CFA_ADVANCE_LOC1 => {
                *loc = advance(*loc, u64::from(r.u8()?), cie.code_alignment);
                if *loc > stop_pc {
                    return Ok(());
                }
            }
            DW_CFA_ADVANCE_LOC2 => {
                *loc = advance(*loc, u64::from(r.u16()?), cie.code_alignment);
                if *loc > stop_pc {
                    return Ok(());
                }
            }
            DW_CFA_ADVANCE_LOC4 => {
                *loc = advance(*loc, u64::from(r.u32()?), cie.code_alignment);
                if *loc > stop_pc {
                    return Ok(());
                }
            }
            DW_CFA_OFFSET_EXTENDED => {
                let reg = r.uleb()?;
                let factored = r.uleb()? as i64;
                row.regs
                    .insert(reg, RegRule::Offset(scale(factored, cie.data_alignment)));
            }
            DW_CFA_OFFSET_EXTENDED_SF => {
                let reg = r.uleb()?;
                let factored = r.sleb()?;
                row.regs
                    .insert(reg, RegRule::Offset(scale(factored, cie.data_alignment)));
            }
            DW_CFA_VAL_OFFSET => {
                let reg = r.uleb()?;
                let factored = r.uleb()? as i64;
                row.regs
                    .insert(reg, RegRule::ValOffset(scale(factored, cie.data_alignment)));
            }
            DW_CFA_VAL_OFFSET_SF => {
                let reg = r.uleb()?;
                let factored = r.sleb()?;
                row.regs
                    .insert(reg, RegRule::ValOffset(scale(factored, cie.data_alignment)));
            }
            DW_CFA_RESTORE_EXTENDED => {
                let reg = r.uleb()?;
                match initial.get(&reg) {
                    Some(rule) => {
                        row.regs.insert(reg, *rule);
                    }
                    None => {
                        row.regs.remove(&reg);
                    }
                }
            }
            DW_CFA_UNDEFINED => {
                let reg = r.uleb()?;
                row.regs.insert(reg, RegRule::Undefined);
            }
            DW_CFA_SAME_VALUE => {
                let reg = r.uleb()?;
                row.regs.insert(reg, RegRule::SameValue);
            }
            DW_CFA_REGISTER => {
                let reg = r.uleb()?;
                let other = r.uleb()?;
                row.regs.insert(reg, RegRule::Register(other));
            }
            DW_CFA_REMEMBER_STATE => stack.push((row.cfa, row.regs.clone())),
            DW_CFA_RESTORE_STATE => {
                let (cfa, regs) = stack.pop().ok_or_else(|| {
                    LdbError::DwarfError("DW_CFA_restore_state with nothing remembered".into())
                })?;
                row.cfa = cfa;
                row.regs = regs;
            }
            DW_CFA_DEF_CFA => {
                let reg = r.uleb()?;
                let offset = r.uleb()? as i64;
                row.cfa = CfaRule::RegisterOffset { reg, offset };
            }
            DW_CFA_DEF_CFA_SF => {
                let reg = r.uleb()?;
                let offset = scale(r.sleb()?, cie.data_alignment);
                row.cfa = CfaRule::RegisterOffset { reg, offset };
            }
            DW_CFA_DEF_CFA_REGISTER => {
                let reg = r.uleb()?;
                match &mut row.cfa {
                    CfaRule::RegisterOffset { reg: cur, .. } => *cur = reg,
                    CfaRule::Undefined => {
                        return Err(LdbError::DwarfError(
                            "DW_CFA_def_cfa_register before any DW_CFA_def_cfa".into(),
                        ));
                    }
                }
            }
            DW_CFA_DEF_CFA_OFFSET => {
                let offset = r.uleb()? as i64;
                match &mut row.cfa {
                    CfaRule::RegisterOffset { offset: cur, .. } => *cur = offset,
                    CfaRule::Undefined => {
                        return Err(LdbError::DwarfError(
                            "DW_CFA_def_cfa_offset before any DW_CFA_def_cfa".into(),
                        ));
                    }
                }
            }
            DW_CFA_DEF_CFA_OFFSET_SF => {
                let offset = scale(r.sleb()?, cie.data_alignment);
                match &mut row.cfa {
                    CfaRule::RegisterOffset { offset: cur, .. } => *cur = offset,
                    CfaRule::Undefined => {
                        return Err(LdbError::DwarfError(
                            "DW_CFA_def_cfa_offset_sf before any DW_CFA_def_cfa".into(),
                        ));
                    }
                }
            }
            DW_CFA_DEF_CFA_EXPRESSION => {
                let n = r.uleb()? as usize;
                r.skip(n)?;
                return Err(LdbError::Unsupported(
                    "DW_CFA_def_cfa_expression: ldb has no DWARF expression evaluator".into(),
                ));
            }
            DW_CFA_EXPRESSION | DW_CFA_VAL_EXPRESSION => {
                let _reg = r.uleb()?;
                let n = r.uleb()? as usize;
                r.skip(n)?;
                return Err(LdbError::Unsupported(
                    "DW_CFA_expression: ldb has no DWARF expression evaluator".into(),
                ));
            }
            DW_CFA_GNU_ARGS_SIZE => {
                let _ = r.uleb()?;
            }
            other => {
                return Err(LdbError::Unsupported(format!(
                    "call frame opcode {:#x}",
                    other
                )));
            }
        }
    }
    Ok(())
}

/// Parse `.eh_frame`. `section_addr` is its virtual address, needed because
/// `DW_EH_PE_pcrel` pointers are relative to where the pointer itself lives.
pub fn parse(section: &[u8], section_addr: u64) -> Result<EhFrame> {
    let mut cies: Vec<Cie> = Vec::new();
    let mut cie_by_offset: HashMap<usize, usize> = HashMap::new();
    let mut fdes = Vec::new();
    let mut r = Reader::new(section);

    while r.remaining() >= 4 {
        let entry_start = r.pos();
        let (len, dwarf64) = r.initial_length()?;
        if len == 0 {
            break;
        }
        let after_len = r.pos();
        let entry_end = usize::try_from(len)
            .ok()
            .and_then(|n| after_len.checked_add(n))
            .unwrap_or(usize::MAX);
        if entry_end > section.len() {
            return Err(LdbError::DwarfError(format!(
                ".eh_frame entry at {:#x} runs past the section",
                entry_start
            )));
        }
        let id = r.offset(dwarf64)?;
        if id == 0 {
            let cie = parse_cie(section, &mut r, entry_end)?;
            cie_by_offset.insert(entry_start, cies.len());
            cies.push(cie);
        } else {
            // In .eh_frame the CIE pointer is the distance back from this field.
            let cie_off = after_len.checked_sub(id as usize).ok_or_else(|| {
                LdbError::DwarfError(format!(
                    "FDE at {:#x} points to a CIE before the section",
                    entry_start
                ))
            })?;
            let cie_idx = *cie_by_offset.get(&cie_off).ok_or_else(|| {
                LdbError::DwarfError(format!(
                    "FDE at {:#x} names a CIE at {:#x} that was not parsed",
                    entry_start, cie_off
                ))
            })?;
            let cie = &cies[cie_idx];
            let enc = cie.fde_pointer_encoding;
            let pc_begin_pos = r.pos();
            let initial_location = read_encoded(&mut r, enc, section_addr + pc_begin_pos as u64)?;
            let address_range = read_encoded(&mut r, enc & 0x0f, 0)?;
            if cie.has_z {
                let n = r.uleb()? as usize;
                r.skip(n)?;
            }
            if r.pos() > entry_end {
                return Err(LdbError::DwarfError(format!(
                    "FDE at {:#x} has fields that run past its own {} byte entry",
                    entry_start, len
                )));
            }
            let instructions = section[r.pos()..entry_end].to_vec();
            fdes.push(Fde {
                initial_location,
                address_range,
                cie: cie_idx,
                instructions,
            });
        }
        r.set_pos(entry_end);
    }
    fdes.sort_by_key(|f| f.initial_location);
    Ok(EhFrame { cies, fdes })
}

fn parse_cie(section: &[u8], r: &mut Reader<'_>, entry_end: usize) -> Result<Cie> {
    let version = r.u8()?;
    if version != 1 && version != 3 {
        return Err(LdbError::Unsupported(format!(
            ".eh_frame CIE version {}; ldb reads 1 and 3",
            version
        )));
    }
    let aug = r.cstr()?.to_string();
    let code_alignment = r.uleb()?;
    let data_alignment = r.sleb()?;
    let return_address_register = if version == 1 {
        u64::from(r.u8()?)
    } else {
        r.uleb()?
    };
    let has_z = aug.starts_with('z');
    let mut fde_pointer_encoding = 0x00;
    if has_z {
        let aug_len = r.uleb()? as usize;
        let aug_end = r.pos().checked_add(aug_len).filter(|e| *e <= entry_end);
        let Some(aug_end) = aug_end else {
            return Err(LdbError::DwarfError(
                "CIE augmentation data runs past the end of the CIE".into(),
            ));
        };
        for c in aug.chars().skip(1) {
            match c {
                'R' => fde_pointer_encoding = r.u8()?,
                'P' => {
                    let enc = r.u8()?;
                    read_encoded(r, enc & 0x0f, 0)?;
                }
                'L' => {
                    let _ = r.u8()?;
                }
                'S' | 'B' | 'G' => {}
                other => {
                    return Err(LdbError::Unsupported(format!(
                        ".eh_frame augmentation character '{}'",
                        other
                    )));
                }
            }
        }
        r.set_pos(aug_end);
    }
    if r.pos() > entry_end {
        return Err(LdbError::DwarfError(
            "CIE header fields run past the end of the CIE".into(),
        ));
    }
    let instructions = section[r.pos()..entry_end].to_vec();
    Ok(Cie {
        code_alignment,
        data_alignment,
        return_address_register,
        fde_pointer_encoding,
        has_z,
        instructions,
    })
}

/// Read one pointer in a `DW_EH_PE_*` encoding. `pc` is the address of the
/// pointer itself, which `DW_EH_PE_pcrel` is relative to.
fn read_encoded(r: &mut Reader<'_>, encoding: u8, pc: u64) -> Result<u64> {
    if encoding == 0xff {
        return Ok(0);
    }
    if encoding & 0x80 != 0 {
        return Err(LdbError::Unsupported(
            "DW_EH_PE_indirect in .eh_frame; ldb does not chase it".into(),
        ));
    }
    let value: i64 = match encoding & 0x0f {
        0x00 => r.u64()? as i64, // DW_EH_PE_absptr, 8 bytes on x86_64
        0x01 => r.uleb()? as i64,
        0x02 => i64::from(r.u16()?),
        0x03 => i64::from(r.u32()?),
        0x04 => r.u64()? as i64,
        0x09 => r.sleb()?,
        0x0a => i64::from(r.u16()? as i16),
        0x0b => i64::from(r.u32()? as i32),
        0x0c => r.u64()? as i64,
        other => {
            return Err(LdbError::Unsupported(format!(
                "DW_EH_PE value encoding {:#x}",
                other
            )));
        }
    };
    let base = match encoding & 0x70 {
        0x00 => 0,
        0x10 => pc,
        other => {
            return Err(LdbError::Unsupported(format!(
                "DW_EH_PE application {:#x}; ldb handles absolute and pcrel",
                other
            )));
        }
    };
    Ok(base.wrapping_add(value as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTION_ADDR: u64 = 0x1000;

    /// A `.eh_frame` with one "zR" CIE (`DW_EH_PE_pcrel | DW_EH_PE_sdata4`) and
    /// one FDE, shaped the way ras emits x86_64 frames.
    fn fixture(func: u64, range: u32) -> Vec<u8> {
        let mut cie_body: Vec<u8> = vec![
            0x00,
            0x00,
            0x00,
            0x00, // CIE_id
            0x01, // version
            b'z',
            b'R',
            0x00, // augmentation
            0x01, // code alignment factor
            0x78, // data alignment factor, sleb -8
            0x10, // return address register
            0x01, // augmentation data length
            0x1b, // DW_EH_PE_pcrel | DW_EH_PE_sdata4
            DW_CFA_DEF_CFA,
            0x07,
            0x08,
            DW_CFA_OFFSET | 0x10,
            0x01,
        ];
        while cie_body.len() % 8 != 4 {
            cie_body.push(DW_CFA_NOP);
        }
        let mut out = (cie_body.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&cie_body);

        let fde_start = out.len();
        let mut fde_body: Vec<u8> = Vec::new();
        fde_body.extend_from_slice(&((fde_start + 4) as u32).to_le_bytes()); // CIE pointer
        let pc_begin_at = SECTION_ADDR + (fde_start + 8) as u64;
        fde_body.extend_from_slice(&((func as i64 - pc_begin_at as i64) as i32).to_le_bytes());
        fde_body.extend_from_slice(&range.to_le_bytes());
        fde_body.push(0x00); // augmentation data length
        fde_body.extend_from_slice(&[
            DW_CFA_ADVANCE_LOC | 1,
            DW_CFA_DEF_CFA_OFFSET,
            0x10,
            DW_CFA_OFFSET | 0x06,
            0x02,
            DW_CFA_ADVANCE_LOC | 3,
            DW_CFA_DEF_CFA_REGISTER,
            0x06,
        ]);
        while fde_body.len() % 8 != 4 {
            fde_body.push(DW_CFA_NOP);
        }
        out.extend_from_slice(&(fde_body.len() as u32).to_le_bytes());
        out.extend_from_slice(&fde_body);
        out
    }

    #[test]
    fn finds_the_fde_through_a_pcrel_pointer() {
        let bytes = fixture(0x2000, 0x40);
        let eh = parse(&bytes, SECTION_ADDR).unwrap();
        assert_eq!(eh.fdes().len(), 1);
        assert_eq!(eh.fdes()[0].initial_location, 0x2000);
        assert_eq!(eh.fdes()[0].address_range, 0x40);
        assert!(eh.fde_for_pc(0x2000).is_some());
        assert!(eh.fde_for_pc(0x203f).is_some());
        assert!(eh.fde_for_pc(0x2040).is_none());
        assert!(eh.fde_for_pc(0x1fff).is_none());
    }

    #[test]
    fn the_cie_row_holds_before_the_prologue() {
        let bytes = fixture(0x2000, 0x40);
        let eh = parse(&bytes, SECTION_ADDR).unwrap();
        let row = eh.row_for_pc(0x2000).unwrap().unwrap();
        assert_eq!(row.return_address_register, 16);
        assert_eq!(row.cfa, CfaRule::RegisterOffset { reg: 7, offset: 8 });
        assert_eq!(row.regs.get(&16), Some(&RegRule::Offset(-8)));
        assert_eq!(row.regs.get(&6), None);
    }

    #[test]
    fn the_fde_rows_apply_in_program_order() {
        let bytes = fixture(0x2000, 0x40);
        let eh = parse(&bytes, SECTION_ADDR).unwrap();

        let after_push = eh.row_for_pc(0x2001).unwrap().unwrap();
        assert_eq!(
            after_push.cfa,
            CfaRule::RegisterOffset { reg: 7, offset: 16 }
        );
        assert_eq!(after_push.regs.get(&6), Some(&RegRule::Offset(-16)));

        let after_mov = eh.row_for_pc(0x2004).unwrap().unwrap();
        assert_eq!(
            after_mov.cfa,
            CfaRule::RegisterOffset { reg: 6, offset: 16 }
        );
        assert_eq!(after_mov.regs.get(&16), Some(&RegRule::Offset(-8)));
    }

    #[test]
    fn no_fde_is_not_an_error() {
        let bytes = fixture(0x2000, 0x40);
        let eh = parse(&bytes, SECTION_ADDR).unwrap();
        assert!(eh.row_for_pc(0x9000).unwrap().is_none());
    }

    #[test]
    fn an_eh_frame_entry_running_past_the_section_is_refused() {
        let mut sec = u32::MAX.wrapping_sub(0x10).to_le_bytes().to_vec();
        sec.extend_from_slice(&[0u8; 16]);
        let err = parse(&sec, SECTION_ADDR).unwrap_err();
        assert!(format!("{err}").contains("runs past"), "{err}");
    }
    #[test]
    fn an_fde_whose_fields_overrun_its_own_entry_is_refused() {
        // A "zR"/absptr CIE, then an FDE whose declared length cannot hold the
        // 8-byte initial_location the encoding demands.
        let cie_body: Vec<u8> = vec![
            0x00, 0x00, 0x00, 0x00, // CIE_id
            0x01, // version
            b'z', b'R', 0x00, // augmentation
            0x01, // code alignment
            0x78, // data alignment
            0x10, // return address register
            0x01, // augmentation data length
            0x00, // DW_EH_PE_absptr
        ];
        let mut sec = (cie_body.len() as u32).to_le_bytes().to_vec();
        sec.extend_from_slice(&cie_body);
        let cie_ptr = (sec.len() + 4) as u32;
        let fde_body = cie_ptr.to_le_bytes().to_vec();
        sec.extend_from_slice(&(fde_body.len() as u32).to_le_bytes());
        sec.extend_from_slice(&fde_body);
        sec.extend_from_slice(&[0u8; 32]); // room for the overrun to land in
        let err = parse(&sec, SECTION_ADDR).unwrap_err();
        assert!(format!("{err}").contains("run past its own"), "{err}");
    }
    #[test]
    fn an_fde_at_the_top_of_the_address_space_does_not_overflow_a_lookup() {
        let cie_body: Vec<u8> = vec![
            0x00, 0x00, 0x00, 0x00, 0x01, b'z', b'R', 0x00, 0x01, 0x78, 0x10, 0x01, 0x00,
        ];
        let mut sec = (cie_body.len() as u32).to_le_bytes().to_vec();
        sec.extend_from_slice(&cie_body);
        let cie_ptr = (sec.len() + 4) as u32;
        let mut fde_body = cie_ptr.to_le_bytes().to_vec();
        fde_body.extend_from_slice(&u64::MAX.to_le_bytes()); // initial_location
        fde_body.extend_from_slice(&u64::MAX.to_le_bytes()); // address_range
        fde_body.push(0x00);
        sec.extend_from_slice(&(fde_body.len() as u32).to_le_bytes());
        sec.extend_from_slice(&fde_body);
        let eh = parse(&sec, SECTION_ADDR).unwrap();
        assert!(eh.fde_for_pc(0).is_none());
        assert!(eh.fde_for_pc(u64::MAX).is_some());
    }
}
