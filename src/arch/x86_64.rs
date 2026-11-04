//! x86_64.

/// The bytes a breakpoint writes over the instruction at its address: `int3`.
/// It is a slice and not a byte because the next architecture's trap is not one
/// byte — aarch64's `brk #0` is four — and the code that plants and lifts a
/// breakpoint has to work from the length.
pub const TRAP: &[u8] = &[0xcc];

/// How far past the trap the program counter sits when the stop is reported.
/// x86_64 reports `rip` one byte after the `int3`, so a stop has to rewind by
/// this much before the address names the breakpoint again. aarch64 reports the
/// `brk` address itself, where this is 0.
pub const TRAP_PC_ADVANCE: u64 = 1;

/// DWARF register numbers for x86_64, from the psABI's table 3.36.
pub const DW_REG_RBX: u64 = 3;
pub const DW_REG_RBP: u64 = 6;
pub const DW_REG_RSP: u64 = 7;
pub const DW_REG_R12: u64 = 12;
pub const DW_REG_R13: u64 = 13;
pub const DW_REG_R14: u64 = 14;
pub const DW_REG_R15: u64 = 15;

/// The registers an unwinder has to carry from frame to frame: the stack
/// pointer, plus the callee-saved set, because a CFA rule in an outer frame can
/// name any of them.
pub const UNWIND_REGISTERS: [u64; 7] = [
    DW_REG_RBX, DW_REG_RBP, DW_REG_RSP, DW_REG_R12, DW_REG_R13, DW_REG_R14, DW_REG_R15,
];
