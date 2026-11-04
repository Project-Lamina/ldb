//! Little-endian byte cursor shared by every DWARF section parser.

use crate::error::{LdbError, Result};

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Reader { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn done(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn need(&self, n: usize) -> Result<()> {
        if self.remaining() < n {
            return Err(LdbError::DwarfError(format!(
                "truncated section: wanted {} bytes at offset {}, {} left",
                n,
                self.pos,
                self.remaining()
            )));
        }
        Ok(())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.need(n)?;
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.need(n)?;
        self.pos += n;
        Ok(())
    }

    pub fn u8(&mut self) -> Result<u8> {
        self.need(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn uleb(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = self.u8()?;
            if shift < 64 {
                result |= u64::from(byte & 0x7f) << shift;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
            if shift > 128 {
                return Err(LdbError::DwarfError("ULEB128 runs past 128 bits".into()));
            }
        }
        Ok(result)
    }

    pub fn sleb(&mut self) -> Result<i64> {
        let mut result: i64 = 0;
        let mut shift = 0u32;
        let mut byte;
        loop {
            byte = self.u8()?;
            if shift < 64 {
                result |= i64::from(byte & 0x7f) << shift;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
            if shift > 128 {
                return Err(LdbError::DwarfError("SLEB128 runs past 128 bits".into()));
            }
        }
        if shift < 64 && byte & 0x40 != 0 {
            result |= -1i64 << shift;
        }
        Ok(result)
    }

    /// NUL-terminated string, as `DW_FORM_string` stores it inline in `.debug_info`.
    pub fn cstr(&mut self) -> Result<&'a str> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0 {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err(LdbError::DwarfError(format!(
                "unterminated string at offset {}",
                start
            )));
        }
        let raw = &self.data[start..self.pos];
        self.pos += 1;
        std::str::from_utf8(raw)
            .map_err(|_| LdbError::DwarfError(format!("non-UTF-8 string at offset {}", start)))
    }

    /// An initial-length field. Returns the unit length and whether the unit uses
    /// 64-bit DWARF; `0xffffffff` introduces the 64-bit form (DWARF 4 section 7.4).
    pub fn initial_length(&mut self) -> Result<(u64, bool)> {
        let first = self.u32()?;
        if first == 0xffff_ffff {
            Ok((self.u64()?, true))
        } else if first >= 0xffff_fff0 {
            Err(LdbError::DwarfError(format!(
                "reserved initial length value {:#x}",
                first
            )))
        } else {
            Ok((u64::from(first), false))
        }
    }

    pub fn offset(&mut self, dwarf64: bool) -> Result<u64> {
        if dwarf64 {
            self.u64()
        } else {
            Ok(u64::from(self.u32()?))
        }
    }
}

/// Read a NUL-terminated string living at `offset` in a string section.
pub fn str_at(section: &[u8], offset: usize) -> Result<&str> {
    if offset >= section.len() {
        return Err(LdbError::DwarfError(format!(
            "string offset {} past end of section ({} bytes)",
            offset,
            section.len()
        )));
    }
    Reader::at(section, offset).cstr()
}
