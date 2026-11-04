//! The executable file, and the vocabulary a reader for one has to supply.
//!
//! ELF is the only format ldb reads. What is here rather than in [`elf`] is the
//! part a Mach-O or PE reader would share: a section, a function symbol, and
//! [`DebugSection`], which names the debug sections by what they hold instead
//! of by their ELF spelling. That last one is the seam that matters. Mach-O
//! calls the same section `__debug_info` inside a `__DWARF` segment, so a
//! consumer that asks for `".debug_info"` by name is an ELF consumer however
//! format-independent its parsing is.
//!
//! There is no `ObjectFile` trait. A trait written against ELF today would be
//! wrong for both of the formats that would implement it: Mach-O usually keeps
//! its DWARF in a separate `.dSYM` bundle rather than in the executable, so the
//! lookup is not "a section of this file", and PE carries `.pdata`/`.xdata`
//! unwind tables rather than CFI, so [`crate::unwind`] would not be reading the
//! same thing at all. The boundary is named; abstracting it can wait until
//! there is a second reader to abstract over.

pub mod elf;

pub use elf::Elf;

/// A section of the file, as the reader resolved it.
#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    /// `true` when the section occupies no bytes in the file, so its contents
    /// cannot be read however large `size` is.
    pub is_nobits: bool,
}

/// A function symbol from the file's symbol table.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
}

/// A debug section named by what it holds. The object reader maps it to the
/// spelling its format uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugSection {
    Info,
    Abbrev,
    Str,
    LineStr,
    Line,
    /// Call frame information. Not a DWARF section in the usual sense — it is
    /// the platform ABI's, which is why its name differs most between formats.
    EhFrame,
}
