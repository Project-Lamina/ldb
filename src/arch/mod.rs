//! Instruction-set facts the debugger needs, for the architecture this build
//! runs on.
//!
//! ldb is a native debugger: the debuggee's architecture is the host's, so the
//! module is chosen by `target_arch` and there is exactly one. What lives here
//! is what an ISA decides and an operating system does not — the breakpoint
//! trap, how the program counter reports it, and the DWARF register numbers.
//! Reading a live register set is not here, because the struct that holds one
//! belongs to the operating system; that stays in [`crate::inferior`].
//!
//! On an architecture with no module the crate still builds and
//! [`crate::platform`] refuses, so nothing outside a `target_arch = "x86_64"`
//! gate may name anything from here.

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::*;

/// The ELF `e_machine` of a file ldb can debug: `EM_X86_64`. It sits here
/// rather than in the object reader because which machines ldb understands is
/// an architecture question, and it is stated unconditionally because ldb
/// debugs x86_64 whatever host it was built for.
pub const ELF_MACHINE: u16 = 62;
