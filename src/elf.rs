//! Just enough ELF64 to find sections and function symbols.

use crate::error::{LdbError, Result};
use std::collections::HashMap;
use std::path::Path;

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

const STT_FUNC: u8 = 2;

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub is_nobits: bool,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
}

pub struct Elf {
    pub bytes: Vec<u8>,
    pub e_type: u16,
    pub entry: u64,
    sections: Vec<Section>,
    by_name: HashMap<String, usize>,
    pub functions: Vec<Symbol>,
}

impl Elf {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| LdbError::IoError(format!("{}: {}", path.display(), e)))?;
        Self::parse(bytes)
    }

    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < 64 || &bytes[0..4] != b"\x7fELF" {
            return Err(LdbError::ElfError("not an ELF file".into()));
        }
        if bytes[4] != 2 {
            return Err(LdbError::ElfError("not ELF64; ldb is x86_64 only".into()));
        }
        if bytes[5] != 1 {
            return Err(LdbError::ElfError(
                "not little-endian; ldb is x86_64 only".into(),
            ));
        }
        let e_type = u16(&bytes, 16)?;
        let e_machine = u16(&bytes, 18)?;
        if e_machine != 62 {
            return Err(LdbError::ElfError(format!(
                "e_machine {} is not EM_X86_64 (62)",
                e_machine
            )));
        }
        let entry = u64v(&bytes, 24)?;
        let shoff = u64v(&bytes, 40)? as usize;
        let shentsize = usize::from(u16(&bytes, 58)?);
        let shnum = usize::from(u16(&bytes, 60)?);
        let shstrndx = usize::from(u16(&bytes, 62)?);
        if shnum == 0 {
            return Err(LdbError::ElfError("no section headers".into()));
        }
        if shoff + shnum * shentsize > bytes.len() {
            return Err(LdbError::ElfError("section headers run past EOF".into()));
        }

        let raw: Vec<[u64; 7]> = (0..shnum)
            .map(|i| {
                let b = shoff + i * shentsize;
                Ok([
                    u64::from(u32v(&bytes, b)?),      // sh_name
                    u64::from(u32v(&bytes, b + 4)?),  // sh_type
                    u64v(&bytes, b + 16)?,            // sh_addr
                    u64v(&bytes, b + 24)?,            // sh_offset
                    u64v(&bytes, b + 32)?,            // sh_size
                    u64::from(u32v(&bytes, b + 40)?), // sh_link
                    u64v(&bytes, b + 56)?,            // sh_entsize
                ])
            })
            .collect::<Result<_>>()?;

        let shstr = {
            let s = &raw[shstrndx];
            let off = s[3] as usize;
            let size = s[4] as usize;
            bytes
                .get(off..off + size)
                .ok_or_else(|| LdbError::ElfError("section name table past EOF".into()))?
                .to_vec()
        };

        let mut sections = Vec::with_capacity(shnum);
        let mut by_name = HashMap::new();
        for (i, s) in raw.iter().enumerate() {
            let name = cstr(&shstr, s[0] as usize).unwrap_or_default().to_string();
            by_name.entry(name.clone()).or_insert(i);
            sections.push(Section {
                name,
                addr: s[2],
                offset: s[3],
                size: s[4],
                is_nobits: s[1] == 8,
            });
        }

        let mut functions = Vec::new();
        for (i, s) in raw.iter().enumerate() {
            if s[1] != 2 && s[1] != 11 {
                continue; // SHT_SYMTAB, SHT_DYNSYM
            }
            let strtab_idx = s[5] as usize;
            let st = raw.get(strtab_idx).ok_or_else(|| {
                LdbError::ElfError(format!("symbol table {} has no string table", i))
            })?;
            let strtab = bytes
                .get(st[3] as usize..(st[3] + st[4]) as usize)
                .unwrap_or(&[]);
            let entsize = if s[6] == 0 { 24 } else { s[6] as usize };
            let count = (s[4] as usize) / entsize;
            for n in 0..count {
                let b = s[3] as usize + n * entsize;
                let nameoff = u32v(&bytes, b)? as usize;
                let info = *bytes
                    .get(b + 4)
                    .ok_or_else(|| LdbError::ElfError("symbol past EOF".into()))?;
                if info & 0x0f != STT_FUNC {
                    continue;
                }
                let value = u64v(&bytes, b + 8)?;
                let size = u64v(&bytes, b + 16)?;
                let name = cstr(strtab, nameoff).unwrap_or_default().to_string();
                if name.is_empty() || value == 0 {
                    continue;
                }
                functions.push(Symbol { name, value, size });
            }
        }
        functions.sort_by_key(|f| f.value);
        functions.dedup_by(|a, b| a.value == b.value && a.name == b.name);

        Ok(Elf {
            bytes,
            e_type,
            entry,
            sections,
            by_name,
            functions,
        })
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.by_name.get(name).map(|i| &self.sections[*i])
    }

    pub fn section_data(&self, name: &str) -> &[u8] {
        match self.section(name) {
            Some(s) if !s.is_nobits => {
                let start = s.offset as usize;
                let end = start + s.size as usize;
                self.bytes.get(start..end).unwrap_or(&[])
            }
            _ => &[],
        }
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// `true` when the image is position independent and so needs a load bias.
    pub fn is_pie(&self) -> bool {
        self.e_type == ET_DYN
    }
}

fn u16(b: &[u8], off: usize) -> Result<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| LdbError::ElfError(format!("read past EOF at {}", off)))
}

fn u32v(b: &[u8], off: usize) -> Result<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| LdbError::ElfError(format!("read past EOF at {}", off)))
}

fn u64v(b: &[u8], off: usize) -> Result<u64> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
        .ok_or_else(|| LdbError::ElfError(format!("read past EOF at {}", off)))
}

fn cstr(table: &[u8], off: usize) -> Option<&str> {
    let rest = table.get(off..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    std::str::from_utf8(&rest[..end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_non_elf() {
        let err = Elf::parse(vec![0u8; 128]).err().unwrap();
        assert!(format!("{}", err).contains("not an ELF file"));
    }

    #[test]
    fn rejects_a_short_file() {
        let err = Elf::parse(b"\x7fELF".to_vec()).err().unwrap();
        assert!(format!("{}", err).contains("not an ELF file"));
    }
}
