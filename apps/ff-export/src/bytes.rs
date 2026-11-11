//! Little-endian byte cursor helpers.
//!
//! Every structure in this ROM is little-endian. These helpers keep the parsers
//! free of manual index arithmetic and turn an out-of-range read into an error
//! instead of a panic, which matters when walking untrusted archive headers.

use anyhow::{Result, bail};

/// A cursor over a byte slice.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            bail!(
                "seek to {:#x} is past the end of a {} byte buffer",
                pos,
                self.data.len()
            );
        }
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.seek(self.pos.saturating_add(n))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Reads and advances by `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            bail!(
                "read of {} byte(s) at {:#x} is past the end of a {} byte buffer",
                n,
                self.pos,
                self.data.len()
            );
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Reads `n` bytes as ASCII, stopping at the first NUL.
    pub fn ascii_nul(&mut self, n: usize) -> Result<String> {
        let raw = self.take(n)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
    }

    /// Everything from the cursor to the end, without advancing.
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    /// A sub-slice that does not move the cursor.
    pub fn slice(&self, off: usize, len: usize) -> Result<&'a [u8]> {
        if off + len > self.data.len() {
            bail!(
                "slice {:#x}..{:#x} is past the end of a {} byte buffer",
                off,
                off + len,
                self.data.len()
            );
        }
        Ok(&self.data[off..off + len])
    }
}

pub fn u16_at(data: &[u8], off: usize) -> Result<u16> {
    let mut r = Reader::at(data, off);
    r.u16()
}

pub fn u32_at(data: &[u8], off: usize) -> Result<u32> {
    let mut r = Reader::at(data, off);
    r.u32()
}

/// Reads a fixed-size NUL-terminated ASCII name (the shape used by mesh bone and
/// model name tables).
pub fn fixed_name(data: &[u8], off: usize, len: usize) -> Result<String> {
    let mut r = Reader::at(data, off);
    let raw = r.take(len)?;
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let mut r = Reader::new(&data);
        assert_eq!(r.u8().unwrap(), 0x01);
        assert_eq!(r.u16().unwrap(), 0x0302);
        assert_eq!(r.remaining(), 2);
    }

    #[test]
    fn past_the_end_is_an_error_not_a_panic() {
        let data = [0x00, 0x01];
        let mut r = Reader::new(&data);
        assert!(r.u32().is_err());
        assert!(Reader::at(&data, 5).u8().is_err());
    }

    #[test]
    fn at_offset_helpers() {
        let data = [0u8, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(u32_at(&data, 4).unwrap(), 0x0706_0504);
        assert_eq!(u16_at(&data, 2).unwrap(), 0x0302);
    }

    #[test]
    fn fixed_names_stop_at_nul() {
        let data = *b"waist\0\0\0\0\0\0\0\0\0\0\0";
        assert_eq!(fixed_name(&data, 0, 16).unwrap(), "waist");
    }
}
