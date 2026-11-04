//! Walking the stack with `.eh_frame` CFI.
//!
//! The walk itself is architecture-independent: it reads CFI rows and applies
//! them. What is not is the register file it starts from and the register
//! numbers it tracks, which come from [`crate::arch`].

use crate::arch::{
    DW_REG_R12, DW_REG_R13, DW_REG_R14, DW_REG_R15, DW_REG_RBP, DW_REG_RBX, DW_REG_RSP,
    UNWIND_REGISTERS,
};
use crate::dwarf::cfi::{CfaRule, EhFrame, RegRule};
use crate::error::{LdbError, Result};
use crate::inferior::Inferior;
use std::collections::HashMap;

const MAX_FRAMES: usize = 256;

/// Why `backtrace` stopped walking. A caller that prints frames must print
/// this too: a list that ends without saying why reads as a complete stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The return address register is undefined here, which is how a CIE marks
    /// the outermost frame.
    OutermostFrame,
    /// No FDE covers this address, so there is nothing to unwind with.
    NoFde(u64),
    /// `MAX_FRAMES` reached; the stack goes on.
    Truncated(usize),
    /// The return address unwound to zero.
    NullReturnAddress,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::OutermostFrame => {
                write!(
                    f,
                    "(outermost frame: .eh_frame leaves the return address undefined here)"
                )
            }
            StopReason::NoFde(pc) => write!(
                f,
                "(unwind stopped: no .eh_frame FDE covers {:#x}, so the caller cannot be found)",
                pc
            ),
            StopReason::Truncated(n) => write!(
                f,
                "(More stack frames follow: ldb stopped after {} frames)",
                n
            ),
            StopReason::NullReturnAddress => {
                write!(f, "(unwind stopped: the return address unwound to 0)")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub pc: u64,
    pub cfa: u64,
    /// `false` for frame 0; for every caller `pc` is a return address, so the
    /// call site is `pc - 1`.
    pub is_return_address: bool,
}

impl Frame {
    /// The address to look up in the line table and in `.eh_frame`.
    pub fn lookup_pc(&self) -> u64 {
        if self.is_return_address {
            self.pc.wrapping_sub(1)
        } else {
            self.pc
        }
    }
}

pub fn backtrace(inf: &Inferior, eh: &EhFrame, bias: u64) -> Result<(Vec<Frame>, StopReason)> {
    let regs = inf.regs()?;
    let mut file: HashMap<u64, u64> = HashMap::new();
    file.insert(DW_REG_RBX, regs.rbx);
    file.insert(DW_REG_RBP, regs.rbp);
    file.insert(DW_REG_RSP, regs.rsp);
    file.insert(DW_REG_R12, regs.r12);
    file.insert(DW_REG_R13, regs.r13);
    file.insert(DW_REG_R14, regs.r14);
    file.insert(DW_REG_R15, regs.r15);

    let mut frames = Vec::new();
    let mut pc = regs.rip;
    let mut is_ret = false;
    let mut stop = StopReason::Truncated(MAX_FRAMES);

    while frames.len() < MAX_FRAMES {
        let lookup = if is_ret { pc.wrapping_sub(1) } else { pc };
        let Some(row) = eh.row_for_pc(lookup.wrapping_sub(bias))? else {
            frames.push(Frame {
                pc,
                cfa: 0,
                is_return_address: is_ret,
            });
            stop = StopReason::NoFde(lookup);
            break;
        };
        let CfaRule::RegisterOffset { reg, offset } = row.cfa else {
            return Err(LdbError::DwarfError(format!(
                "no CFA rule at {:#x}; .eh_frame cannot unwind this frame",
                lookup
            )));
        };
        let base = *file.get(&reg).ok_or_else(|| {
            LdbError::Unsupported(format!("CFA names DWARF register {}, untracked", reg))
        })?;
        let cfa = base.wrapping_add(offset as u64);

        frames.push(Frame {
            pc,
            cfa,
            is_return_address: is_ret,
        });

        let ra = match row.regs.get(&row.return_address_register) {
            Some(RegRule::Offset(o)) => inf.peek_word(cfa.wrapping_add(*o as u64))?,
            Some(RegRule::Undefined) | None | Some(RegRule::SameValue) => {
                stop = StopReason::OutermostFrame;
                break;
            }
            Some(RegRule::Register(r)) => file.get(r).copied().unwrap_or(0),
            Some(RegRule::ValOffset(o)) => cfa.wrapping_add(*o as u64),
        };
        if ra == 0 {
            stop = StopReason::NullReturnAddress;
            break;
        }

        let mut next = file.clone();
        for reg in UNWIND_REGISTERS {
            match row.regs.get(&reg) {
                Some(RegRule::Offset(o)) => {
                    next.insert(reg, inf.peek_word(cfa.wrapping_add(*o as u64))?);
                }
                Some(RegRule::ValOffset(o)) => {
                    next.insert(reg, cfa.wrapping_add(*o as u64));
                }
                Some(RegRule::Register(r)) => {
                    next.insert(reg, file.get(r).copied().unwrap_or(0));
                }
                Some(RegRule::Undefined) => {
                    next.remove(&reg);
                }
                Some(RegRule::SameValue) | None => {}
            }
        }
        next.insert(DW_REG_RSP, cfa);
        file = next;
        pc = ra;
        is_ret = true;
    }
    Ok((frames, stop))
}
