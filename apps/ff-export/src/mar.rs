//! The `MAR\0` container (8,041 of this ROM's files are one).
//!
//! ```text
//! 0x00  4          "MAR\0"
//! 0x04  4          u32 entry count
//! 0x08  count x 8  { u32 offset, u32 decompressed size }   offset from the archive start
//!        payload     concatenated entries; stored size = next offset - this offset
//! ```
//!
//! Offsets are strictly increasing and the first payload begins immediately after
//! the index, i.e. `offset[0] == 8 + 8 * count`. Both facts are asserted on parse
//! because they are the cheapest possible corruption checks.

use anyhow::{Result, bail};

use crate::bytes::u32_at;

pub const MAGIC: &[u8; 3] = b"MAR";

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// Offset of the entry's stored bytes, from the start of the archive.
    pub offset: u32,
    /// Size the entry decompresses to.
    pub decompressed_size: u32,
}

pub struct Archive<'a> {
    data: &'a [u8],
    pub entries: Vec<Entry>,
}

impl<'a> Archive<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        if data.len() < 8 {
            bail!("MAR archive is too short ({} bytes)", data.len());
        }
        if &data[0..3] != MAGIC {
            bail!("not a MAR archive (magic {:02X?})", &data[0..3]);
        }

        let count = u32_at(data, 4)? as usize;
        let index_end = match count.checked_mul(8).and_then(|n| n.checked_add(8)) {
            Some(end) => end,
            None => bail!("MAR entry count {} overflows the index size", count),
        };

        if index_end > data.len() {
            bail!(
                "MAR index claims {} entries ({:#x} bytes) but the archive is {} bytes",
                count,
                index_end,
                data.len()
            );
        }

        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            entries.push(Entry {
                offset: u32_at(data, 8 + i * 8)?,
                decompressed_size: u32_at(data, 8 + i * 8 + 4)?,
            });
        }

        if let Some(first) = entries.first() {
            if first.offset as usize != index_end {
                bail!(
                    "MAR first entry is at {:#x}, expected {:#x} (8 + 8 x {})",
                    first.offset,
                    index_end,
                    count
                );
            }
        }
        for pair in entries.windows(2) {
            if pair[1].offset <= pair[0].offset {
                bail!(
                    "MAR offsets are not strictly increasing ({:#x} then {:#x})",
                    pair[0].offset,
                    pair[1].offset
                );
            }
        }
        if let Some(last) = entries.last() {
            if last.offset as usize > data.len() {
                bail!(
                    "MAR last entry starts at {:#x}, past the end of a {} byte archive",
                    last.offset,
                    data.len()
                );
            }
        }

        Ok(Self { data, entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry's bytes exactly as stored, still compressed.
    pub fn stored(&self, index: usize) -> Result<&'a [u8]> {
        let entry = self.entries.get(index).ok_or_else(|| {
            anyhow::anyhow!(
                "MAR entry {} is out of range (archive has {})",
                index,
                self.entries.len()
            )
        })?;

        let start = entry.offset as usize;
        let end = match self.entries.get(index + 1) {
            Some(next) => next.offset as usize,
            None => self.data.len(),
        };

        if start > end || end > self.data.len() {
            bail!(
                "MAR entry {} spans {:#x}..{:#x}, outside a {} byte archive",
                index,
                start,
                end,
                self.data.len()
            );
        }

        Ok(&self.data[start..end])
    }

    /// The entry's decompressed bytes. The `MCM` header's declared size is
    /// cross-checked against the size the `MAR` index declares for the same entry,
    /// because a mismatch means one of the two tables has been misread.
    pub fn decompressed(&self, index: usize) -> Result<Vec<u8>> {
        let (data, info) = crate::mcm::decompress(self.stored(index)?)?;
        let declared = self.entries[index].decompressed_size;
        if declared != info.decompressed_size {
            bail!(
                "MAR entry {} declares {} bytes but its MCM header says {}",
                index,
                declared,
                info.decompressed_size
            );
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a MAR archive of uncompressed (MCM "none") entries.
    fn build(payloads: &[&[u8]]) -> Vec<u8> {
        let count = payloads.len();

        // each payload gets wrapped in its own MCM entry
        let wrapped: Vec<Vec<u8>> = payloads.iter().map(|p| wrap_mcm(p)).collect();

        let mut out = Vec::new();
        out.extend_from_slice(b"MAR\0");
        out.extend_from_slice(&(count as u32).to_le_bytes());

        let mut offset = (8 + count * 8) as u32;
        for (entry, payload) in wrapped.iter().zip(payloads) {
            out.extend_from_slice(&offset.to_le_bytes());
            // the index records the *decompressed* size, not the stored size
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            offset += entry.len() as u32;
        }
        for entry in &wrapped {
            out.extend_from_slice(entry);
        }
        out
    }

    /// A single-chunk, uncompressed MCM entry holding `payload`.
    fn wrap_mcm(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"MCM\0");
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x2000u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.push(0);
        out.push(0);
        out.extend_from_slice(&[0, 0]);
        // header is 0x14 plus one offset per chunk plus the end offset
        out.extend_from_slice(&0x1Cu32.to_le_bytes()); // chunk 0 offset
        out.extend_from_slice(&(0x1C + payload.len() as u32).to_le_bytes()); // end
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn indexes_and_decompresses_entries() {
        let archive = build(&[b"abc", b"de"]);
        let parsed = Archive::parse(&archive).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.entries[0].decompressed_size, 3);
        assert_eq!(parsed.entries[1].decompressed_size, 2);
        assert_eq!(parsed.decompressed(0).unwrap(), b"abc");
        assert_eq!(parsed.decompressed(1).unwrap(), b"de");
    }

    #[test]
    fn rejects_a_wrong_magic() {
        let mut archive = build(&[b"abc"]);
        archive[0] = b'X';
        assert!(Archive::parse(&archive).is_err());
    }

    #[test]
    fn rejects_a_bogus_entry_count() {
        let mut archive = build(&[b"abc"]);
        archive[4..8].copy_from_slice(&1000u32.to_le_bytes());
        assert!(Archive::parse(&archive).is_err());
    }

    #[test]
    fn rejects_a_first_offset_that_leaves_a_gap() {
        let mut archive = build(&[b"abc"]);
        archive[8..12].copy_from_slice(&0x100u32.to_le_bytes());
        assert!(Archive::parse(&archive).is_err());
    }

    #[test]
    fn rejects_non_increasing_offsets() {
        let mut archive = build(&[b"abc", b"de"]);
        let second = 8 + 8;
        archive[second..second + 4].copy_from_slice(&16u32.to_le_bytes());
        assert!(Archive::parse(&archive).is_err());
    }

    #[test]
    fn an_out_of_range_entry_is_an_error() {
        let archive = build(&[b"abc"]);
        let parsed = Archive::parse(&archive).unwrap();
        assert!(parsed.stored(7).is_err());
        assert!(parsed.decompressed(7).is_err());
    }

    #[test]
    fn a_size_mismatch_between_the_two_tables_is_caught() {
        let mut archive = build(&[b"abc"]);
        // MAR says 3, the MCM header says 3; change the MCM header to 4
        archive[8 + 8 + 4..8 + 8 + 8].copy_from_slice(&4u32.to_le_bytes());
        let parsed = Archive::parse(&archive).unwrap();
        assert!(parsed.decompressed(0).is_err());
    }
}
