//! Nintendo LZ10 decompression (called "LZSS" throughout this game's tooling).
//!
//! After the 4-byte header the stream is a sequence of blocks of up to 8 tokens.
//! A block is introduced by one flag byte, whose bits are consumed from the most
//! significant down:
//!
//! * bit clear - the next byte is a literal;
//! * bit set   - the next two bytes are a back reference: `count - 3` in the high
//!   nibble of the first byte, `displacement - 1` in its low nibble plus the
//!   second byte (12 bits total).
//!
//! Decompression stops as soon as the declared decompressed size is reached, so
//! the trailing bits of the final flag byte are ignored.

use anyhow::{Result, bail};

use super::ChunkHeader;

pub const MIN_MATCH: usize = 3;
pub const MAX_MATCH: usize = 18;

pub fn decompress(input: &[u8]) -> Result<Vec<u8>> {
    if input.len() < ChunkHeader::LEN {
        bail!(
            "lz10 stream is shorter than its 4 byte header ({} bytes)",
            input.len()
        );
    }

    let word = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
    let header = ChunkHeader::parse(word);
    if header.codec != super::Codec::Lz10 {
        bail!(
            "expected an lz10 header, found codec id {}",
            (word >> 4) & 0xF
        );
    }

    let target = header.decompressed_size as usize;
    if target == 0 {
        bail!("lz10 stream declares a decompressed size of 0");
    }

    let mut out: Vec<u8> = Vec::with_capacity(target);
    let mut pos = ChunkHeader::LEN;

    'outer: loop {
        if pos >= input.len() {
            bail!(
                "lz10 stream ended after {} of {} bytes were produced",
                out.len(),
                target
            );
        }

        let flags = input[pos];
        pos += 1;

        for bit in (0..8).rev() {
            if (flags >> bit) & 1 == 0 {
                if pos >= input.len() {
                    bail!("lz10 literal at {:#x} runs past the end of the stream", pos);
                }
                out.push(input[pos]);
                pos += 1;
            } else {
                if pos + 1 >= input.len() {
                    bail!("lz10 back reference at {:#x} runs past the end of the stream", pos);
                }
                let first = input[pos] as usize;
                let second = input[pos + 1] as usize;
                pos += 2;

                let count = (first >> 4) + MIN_MATCH;
                let displacement = ((first & 0xF) << 8 | second) + 1;

                if displacement > out.len() {
                    bail!(
                        "lz10 back reference of {} at {:#x} exceeds the {} bytes produced so far",
                        displacement,
                        pos - 2,
                        out.len()
                    );
                }

                // Deliberate overlap: matches may reference bytes this loop is
                // still producing, which is how runs are encoded.
                let start = out.len() - displacement;
                for i in 0..count {
                    let byte = out[start + i];
                    out.push(byte);
                }
            }

            if out.len() >= target {
                break 'outer;
            }
        }
    }

    if out.len() != target {
        bail!("lz10 produced {} bytes, expected {}", out.len(), target);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(decompressed_size: u32, body: &[u8]) -> Vec<u8> {
        let word = (decompressed_size << 8) | (1 << 4);
        let mut out = word.to_le_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn literals_only() {
        // 3 literal tokens: `A B C`
        let data = stream(3, &[0b0000_0000, b'A', b'B', b'C']);
        assert_eq!(decompress(&data).unwrap(), b"ABC");
    }

    #[test]
    fn literals_then_a_back_reference() {
        // `A B C` then a 3-byte match at displacement 3 -> `ABCABC`.
        // flags 0b0001_0000: three literals, then a match, MSB first.
        // first byte 0x00 -> count = 0 + 3, displacement high nibble 0;
        // second byte 0x02 -> displacement = 0x002 + 1 = 3.
        let data = stream(6, &[0b0001_0000, b'A', b'B', b'C', 0x00, 0x02]);
        assert_eq!(decompress(&data).unwrap(), b"ABCABC");
    }

    #[test]
    fn overlapping_match_expands_a_run() {
        // `A` then a match of 5 at displacement 1 repeats the previous byte.
        // first byte 0x20 -> count = 2 + 3 = 5; second byte 0x00 -> displacement 1.
        let data = stream(6, &[0b0100_0000, b'A', 0x20, 0x00]);
        assert_eq!(decompress(&data).unwrap(), b"AAAAAA");
    }

    #[test]
    fn rejects_a_non_lz10_header() {
        // codec id 3 (run length) in a header that claims to be lz10
        let word = (3u32 << 8) | (3u32 << 4);
        assert!(decompress(&word.to_le_bytes()).is_err());
    }

    #[test]
    fn rejects_a_truncated_stream() {
        let data = stream(3, &[0b0000_0000, b'A']);
        assert!(decompress(&data).is_err());
    }

    #[test]
    fn rejects_a_back_reference_before_the_start() {
        // a match at displacement 1 with nothing produced yet
        let data = stream(3, &[0b1000_0000, 0x00, 0x00]);
        assert!(decompress(&data).is_err());
    }
}
