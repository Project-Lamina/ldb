//! A DWARF reader written for Lamina's own output.
//!
//! Deliberately not `gimli`: reading our DWARF with our own parser is how we
//! find out whether what Lamina emits is actually consumable.

pub mod abbrev;
pub mod cfi;
pub mod consts;
pub mod info;
pub mod line;
pub mod reader;
