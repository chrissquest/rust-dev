//! The `MCM\0` chunk wrapper that every `MAR` entry is stored in.
//!
//! ```text
//! 0x00  4              "MCM\0"
//! 0x04  4              u32 decompressed size of the whole entry
//! 0x08  4              u32 max chunk size (0x2000 in this game)
//! 0x0C  4              u32 chunk count
//! 0x10  1              codec id of the first compression layer
//! 0x11  1              codec id of the second compression layer
//! 0x12  2              padding
//! 0x14  (count+1) x 4  chunk offsets, the last being the end of the last chunk
//!        payload         chunk size = next offset - this offset
//! ```
//!
//! When both codec fields are set a chunk is compressed twice; the inner layer is
//! applied first on the way in, so decompression applies codec 1 and then codec 2.

use anyhow::{Context, Result, bail};

use crate::bytes::u32_at;
use crate::codec::Codec;

pub const MAGIC: &[u8; 3] = b"MCM";
pub const HEADER_LEN: usize = 0x14;

#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub decompressed_size: u32,
    pub max_chunk_size: u32,
    pub chunk_count: u32,
    pub codec1: Codec,
    pub codec2: Codec,
}

impl Info {
    /// The codecs this entry needs, for coverage reporting.
    pub fn codecs(&self) -> [Codec; 2] {
        [self.codec1, self.codec2]
    }
}

/// Reads the `MCM` header without decompressing anything.
pub fn info(data: &[u8]) -> Result<Info> {
    if data.len() < HEADER_LEN {
        bail!(
            "MCM entry is shorter than its {:#x} byte header ({} bytes)",
            HEADER_LEN,
            data.len()
        );
    }
    if &data[0..3] != MAGIC {
        bail!("not an MCM entry (magic {:02X?})", &data[0..3]);
    }
    Ok(Info {
        decompressed_size: u32_at(data, 0x04)?,
        max_chunk_size: u32_at(data, 0x08)?,
        chunk_count: u32_at(data, 0x0C)?,
        codec1: Codec::from_mcm_id(data[0x10]),
        codec2: Codec::from_mcm_id(data[0x11]),
    })
}

/// Decompresses a whole `MCM` entry and returns the bytes plus the header it read.
pub fn decompress(data: &[u8]) -> Result<(Vec<u8>, Info)> {
    let info = info(data)?;
    let count = info.chunk_count as usize;

    let index_end = HEADER_LEN + (count + 1) * 4;
    if index_end > data.len() {
        bail!(
            "MCM declares {} chunk(s) but its offset table needs {:#x} bytes and the entry is {} bytes",
            count,
            index_end,
            data.len()
        );
    }

    let mut out: Vec<u8> = Vec::with_capacity(info.decompressed_size as usize);

    for i in 0..count {
        let start = u32_at(data, HEADER_LEN + i * 4)? as usize;
        let end = u32_at(data, HEADER_LEN + (i + 1) * 4)? as usize;

        if start > end || end > data.len() {
            bail!(
                "MCM chunk {} spans {:#x}..{:#x}, outside a {} byte entry",
                i,
                start,
                end,
                data.len()
            );
        }

        let stored = &data[start..end];
        let inner = info
            .codec1
            .decompress(stored)
            .with_context(|| format!("MCM chunk {} layer 1 ({})", i, info.codec1.name()))?;
        let chunk = info
            .codec2
            .decompress(&inner)
            .with_context(|| format!("MCM chunk {} layer 2 ({})", i, info.codec2.name()))?;

        out.extend_from_slice(&chunk);
    }

    if out.len() != info.decompressed_size as usize {
        bail!(
            "MCM declared {} decompressed bytes but produced {}",
            info.decompressed_size,
            out.len()
        );
    }

    Ok((out, info))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an uncompressed MCM entry out of the given chunks.
    fn build(chunks: &[&[u8]], max_chunk_size: u32) -> Vec<u8> {
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        let count = chunks.len();

        let mut out = Vec::new();
        out.extend_from_slice(b"MCM\0");
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&max_chunk_size.to_le_bytes());
        out.extend_from_slice(&(count as u32).to_le_bytes());
        out.push(0); // codec 1: none
        out.push(0); // codec 2: none
        out.extend_from_slice(&[0, 0]); // padding

        let mut offset = (HEADER_LEN + (count + 1) * 4) as u32;
        for chunk in chunks {
            out.extend_from_slice(&offset.to_le_bytes());
            offset += chunk.len() as u32;
        }
        out.extend_from_slice(&offset.to_le_bytes());

        for chunk in chunks {
            out.extend_from_slice(chunk);
        }
        out
    }

    #[test]
    fn reads_the_header() {
        let entry = build(&[b"hello"], 0x2000);
        let info = info(&entry).unwrap();
        assert_eq!(info.decompressed_size, 5);
        assert_eq!(info.max_chunk_size, 0x2000);
        assert_eq!(info.chunk_count, 1);
        assert_eq!(info.codec1, Codec::None);
    }

    #[test]
    fn joins_uncompressed_chunks() {
        let entry = build(&[b"abc", b"defg"], 4);
        let (data, _) = decompress(&entry).unwrap();
        assert_eq!(data, b"abcdefg");
    }

    #[test]
    fn an_empty_entry_is_valid() {
        let entry = build(&[], 0x2000);
        let (data, info) = decompress(&entry).unwrap();
        assert!(data.is_empty());
        assert_eq!(info.chunk_count, 0);
    }

    #[test]
    fn rejects_a_wrong_magic() {
        let mut entry = build(&[b"x"], 0x2000);
        entry[0] = b'X';
        assert!(info(&entry).is_err());
    }

    #[test]
    fn rejects_chunks_outside_the_entry() {
        let mut entry = build(&[b"abc"], 0x2000);
        let last = entry.len() - 4;
        entry[last..].copy_from_slice(&9999u32.to_le_bytes());
        assert!(decompress(&entry).is_err());
    }

    #[test]
    fn rejects_a_decompressed_size_mismatch() {
        let mut entry = build(&[b"abc"], 0x2000);
        entry[4..8].copy_from_slice(&9u32.to_le_bytes());
        assert!(decompress(&entry).is_err());
    }
}
