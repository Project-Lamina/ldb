//! ldb - a native Linux x86_64 ELF debugger for Lamina.
//!
//! The DWARF reader under [`dwarf`] is written here rather than taken from an
//! existing crate on purpose: reading our own debug info with our own parser is
//! how the project finds out whether what Lamina emits is actually consumable.
//!
//! Three things differ by platform and each has its own module: [`object`]
//! reads the executable file, [`inferior`] drives the traced process, and
//! [`arch`] holds the instruction-set facts. Only [`object`], [`arch`] and the
//! DWARF readers build everywhere; the process-control half is gated on
//! `linux` and `x86_64`, and a build without it refuses at startup. See
//! [`platform`].
//!
//! The command line is the interface: [`main`] runs one session from
//! `std::env::args` and exits with its status, exactly as the `ldb` binary does.

#[cfg(test)]
mod test_support;

pub mod arch;
pub mod dwarf;
pub mod error;
pub mod object;
pub mod platform;
pub mod symbols;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod cli;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod inferior;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod repl;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod session;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod unwind;

pub use error::{LdbError, Result};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use session::Session;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn main() {
    cli::run()
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn main() {
    platform::refuse()
}
