//! The compression codecs wrapped by `MCM` chunks.
//!
//! A compressed chunk begins with a 4-byte header:
//!
//! ```text
//! bits  0..4   data size          (only meaningful for Huffman; 0 otherwise)
//! bits  4..8   codec id
//! bits  8..32  decompressed size  (24 bits)
//! ```
//!
//! There are **two different codec id spaces** in this format, and conflating them
//! silently mislabels every entry:
//!
//! | id | `MCM` header field (0x10 / 0x11) | compressed chunk header (bits 4..8) |
//! | -- | -------------------------------- | ----------------------------------- |
//! | 0  | none                             | unused                              |
//! | 1  | run length                       | LZ10                                |
//! | 2  | LZ10                             | Huffman                             |
//! | 3  | Huffman                          | run length                          |
//!
//! Verified against `model/battle/arcdin`: 1,451 entries carry `0x10 = 2` in the
//! `MCM` header (LZ10) while their chunks begin with `0x1?` (Nintendo id 1). The
//! `MCM` field values match the classic Nitro/"MCM" numbering, the chunk field
//! values match the Nitro compression types.
//!
//! Measured across all 8,041 archives of this game, the two `MCM` slots hold
//! `none` 29,269, `lzss` 9,158, `huffman` 2,143 and `rle` 668 times. Huffman is implemented
//! because most of the game's text uses it (including `text/japanese`, which holds the creature
//! names); run length is not implemented and fails loudly rather than guessing.

pub mod huffman;
pub mod lz10;

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    None,
    Lz10,
    Huffman,
    RunLength,
}

impl Codec {
    /// Id as stored in the `MCM` header's two codec fields.
    pub fn from_mcm_id(id: u8) -> Self {
        match id {
            1 => Codec::RunLength,
            2 => Codec::Lz10,
            3 => Codec::Huffman,
            _ => Codec::None,
        }
    }

    /// Id as stored in the 4-byte header that prefixes a compressed chunk.
    pub fn from_chunk_id(id: u8) -> Self {
        match id {
            1 => Codec::Lz10,
            2 => Codec::Huffman,
            3 => Codec::RunLength,
            _ => Codec::None,
        }
    }

    pub fn mcm_id(self) -> u8 {
        match self {
            Codec::None => 0,
            Codec::RunLength => 1,
            Codec::Lz10 => 2,
            Codec::Huffman => 3,
        }
    }

    pub fn chunk_id(self) -> u8 {
        match self {
            Codec::None => 0,
            Codec::Lz10 => 1,
            Codec::Huffman => 2,
            Codec::RunLength => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Codec::None => "none",
            Codec::Lz10 => "lz10",
            Codec::Huffman => "huffman",
            Codec::RunLength => "rle",
        }
    }

    pub fn is_implemented(self) -> bool {
        matches!(self, Codec::None | Codec::Lz10)
    }

    pub fn decompress(self, data: &[u8]) -> Result<Vec<u8>> {
        match self {
            Codec::None => Ok(data.to_vec()),
            Codec::Lz10 => lz10::decompress(data),
            Codec::RunLength => bail!(
                "run-length compressed chunks are not implemented yet ({} bytes)",
                data.len()
            ),
            Codec::Huffman => huffman::decompress(data),
        }
    }
}

/// The 4-byte header that prefixes every compressed chunk.
#[derive(Debug, Clone, Copy)]
pub struct ChunkHeader {
    pub data_size: u8,
    pub codec: Codec,
    pub decompressed_size: u32,
}

impl ChunkHeader {
    pub const LEN: usize = 4;

    pub fn parse(word: u32) -> Self {
        Self {
            data_size: (word & 0xF) as u8,
            codec: Codec::from_chunk_id(((word >> 4) & 0xF) as u8),
            decompressed_size: word >> 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_bit_layout() {
        let header = ChunkHeader::parse((6 << 8) | (1 << 4));
        assert_eq!(header.decompressed_size, 6);
        assert_eq!(header.codec, Codec::Lz10);
        assert_eq!(header.data_size, 0);
    }

    #[test]
    fn mcm_and_chunk_ids_are_different_spaces() {
        // the same byte means LZ10 in the MCM header but Huffman in a chunk header
        assert_eq!(Codec::from_mcm_id(2), Codec::Lz10);
        assert_eq!(Codec::from_chunk_id(2), Codec::Huffman);
        assert_eq!(Codec::Lz10.mcm_id(), 2);
        assert_eq!(Codec::Lz10.chunk_id(), 1);
        assert_eq!(Codec::RunLength.mcm_id(), 1);
        assert_eq!(Codec::RunLength.chunk_id(), 3);
    }

    #[test]
    fn unknown_codec_ids_read_as_none() {
        assert_eq!(Codec::from_mcm_id(0), Codec::None);
        assert_eq!(Codec::from_mcm_id(15), Codec::None);
        assert_eq!(Codec::from_chunk_id(0), Codec::None);
    }

    #[test]
    fn unimplemented_codecs_are_reported_not_guessed() {
        assert!(Codec::RunLength.decompress(&[0, 0, 0, 0]).is_err());
        assert!(Codec::Huffman.decompress(&[0, 0, 0, 0]).is_err());
        assert_eq!(Codec::None.decompress(&[1, 2, 3]).unwrap(), vec![1, 2, 3]);
    }
}
