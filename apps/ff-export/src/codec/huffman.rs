//! The game's Huffman chunk codec.
//!
//! After the 4-byte chunk header the stream is:
//!
//! ```text
//! 0x00  1  branch node count      the tree is (2 * count + 1) bytes long
//! 0x01     the tree, one byte per node
//!          the bitstream follows immediately, read as little-endian u32 words,
//!          bits consumed from the most significant down
//! ```
//!
//! A tree byte describes one branch node:
//!
//! ```text
//! bits 0..6  child base index, used as 2 * (value + 1)
//! bit  7     the left child is a symbol instead of a branch
//! bit  6     the right child is a symbol instead of a branch
//! ```
//!
//! Children are addressed from the branch node *with its lowest bit cleared*: the left child is at
//! `(node & !1) + 2 * base` and the right child one past it. That indirection is what lets the tree
//! be stored as a flat byte array rather than as pointers.
//!
//! `dataSize` in the chunk header is 4 or 8. With 8 each symbol is one output byte; with 4 the
//! symbols are nybbles packed two per byte, where the **first** decoded symbol becomes the low
//! nibble (matching the reference implementation and the game's own output).

use anyhow::{Result, bail};

use super::ChunkHeader;

/// Reads the tree byte at `node` and returns the child index and whether that child is a symbol.
fn child(data: u8, node: usize, right: bool) -> (usize, bool) {
    let base = node & !1;
    let offset = (data & 0x3F) as usize + 1;
    if right {
        (base + 2 * offset + 1, (data >> 6) & 1 != 0)
    } else {
        (base + 2 * offset, (data >> 7) & 1 != 0)
    }
}

pub fn decompress(input: &[u8]) -> Result<Vec<u8>> {
    if input.len() < ChunkHeader::LEN + 4 {
        bail!("huffman stream is too short ({} bytes)", input.len());
    }

    let word = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
    let header = ChunkHeader::parse(word);
    if header.codec != super::Codec::Huffman {
        bail!("expected a huffman header, found codec id {}", (word >> 4) & 0xF);
    }
    if header.data_size != 4 && header.data_size != 8 {
        bail!("huffman data size must be 4 or 8, found {}", header.data_size);
    }
    let target = header.decompressed_size as usize;
    if target == 0 {
        bail!("huffman stream declares a decompressed size of 0");
    }

    let stream = &input[ChunkHeader::LEN..];
    let branch_count = stream[0] as usize;
    let tree_len = 2 * branch_count + 1;

    // The first tree node sits directly after the branch count byte.
    let root = 1usize;
    let mut bitstream = root + tree_len;
    if bitstream + 4 > stream.len() {
        bail!(
            "huffman tree ({tree_len} bytes) leaves no room for a bitstream in {} bytes",
            stream.len()
        );
    }

    let mut out: Vec<u8> = Vec::with_capacity(target);
    let mut node = root;
    let mut pending_nybble: Option<u8> = None;

    'outer: loop {
        if bitstream + 4 > stream.len() {
            bail!(
                "huffman bitstream ended after {} of {} bytes were produced",
                out.len(),
                target
            );
        }
        let chunk = u32::from_le_bytes([
            stream[bitstream],
            stream[bitstream + 1],
            stream[bitstream + 2],
            stream[bitstream + 3],
        ]);
        bitstream += 4;

        for bit in (0..32).rev() {
            let data = stream[node];
            let (next, is_symbol) = child(data, node, (chunk >> bit) & 1 != 0);
            if next >= stream.len() {
                bail!("huffman child index {next} is past the end of the stream");
            }
            node = next;

            if is_symbol {
                let symbol = stream[node];
                if header.data_size == 4 {
                    match pending_nybble.take() {
                        Some(first) => out.push((symbol << 4) | first),
                        None => pending_nybble = Some(symbol),
                    }
                } else {
                    out.push(symbol);
                }
                node = root;
            }

            if out.len() >= target {
                break 'outer;
            }
        }
    }

    if out.len() != target {
        bail!("huffman produced {} bytes, expected {}", out.len(), target);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a stream: 4-byte chunk header (huffman, id 2) then the tree and bitstream.
    fn stream(decompressed_size: u32, data_size: u8, body: &[u8]) -> Vec<u8> {
        let word = (decompressed_size << 8) | (2 << 4) | data_size as u32;
        let mut out = word.to_le_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn reads_one_symbol_through_the_left_branch() {
        // One branch node at index 1 whose left child (index 2) is the symbol `A`.
        // Node byte 0x80: base 0 -> left child = (1 & !1) + 2 = 2, and bit 7 marks it a symbol.
        // All-zero bits always take the left branch, so every decoded symbol is `A`.
        let body = [1u8, 0x80, b'A', 0x00, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(decompress(&stream(3, 8, &body)).unwrap(), b"AAA");
    }

    #[test]
    fn packs_two_nybbles_into_one_byte() {
        // Left child (index 2) is 0x0A and right child (index 3) is 0x0B: node byte 0xC0 sets both
        // symbol bits. Bit 31 = 0 decodes `A`, bit 30 = 1 decodes `B`, and the first symbol read
        // becomes the low nibble, so the packed byte is (0x0B << 4) | 0x0A.
        let body = [1u8, 0xC0, 0x0A, 0x0B, 0x00, 0x00, 0x00, 0x40];
        assert_eq!(decompress(&stream(1, 4, &body)).unwrap(), vec![0xBA]);
    }

    #[test]
    fn rejects_a_non_huffman_header() {
        let word = (3u32 << 8) | (1u32 << 4) | 8;
        assert!(decompress(&word.to_le_bytes()).is_err());
    }

    #[test]
    fn rejects_an_unsupported_data_size() {
        let body = [1u8, 0x80, b'A', 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(decompress(&stream(1, 7, &body)).is_err());
    }

    #[test]
    fn rejects_a_tree_that_does_not_fit() {
        let body = [200u8, 0x80, b'A'];
        assert!(decompress(&stream(1, 8, &body)).is_err());
    }

    #[test]
    fn rejects_a_child_index_past_the_end() {
        // base 63 -> offset 64 -> left child at index 128, but the stream is 8 bytes long
        let body = [1u8, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(decompress(&stream(1, 8, &body)).is_err());
    }
}
