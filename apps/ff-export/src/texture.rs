//! The texture block: one creature's images and their palettes.
//!
//! ```text
//! 0x00  4  u32 image count
//! 0x04  4  u32 bitmaps length
//! 0x08  4  u32 palettes length
//! 0x0c     imageCount x 28 byte header:
//!            0x00 16  name
//!            0x10  4  bitmap offset, from the start of the bitmaps section
//!            0x14  4  palette offset, from the start of the palettes section
//!            0x18  2  unknown
//!            0x1a  2  info: bits 0..4 unknown, 4..7 width (8 << n), 7..10 height (8 << n),
//!                          10..13 format, 13 transparent, 14 and 15 unknown
//!          then the bitmaps, then the palettes
//! ```
//!
//! Offsets are relative to their own section, and a zero offset after the first means an empty entry
//! whose length runs to the next offset. A palette is a run of DS colours: 16 bits, five per channel,
//! red in the low bits. A mesh's material key is a palette offset divided by 16 (8 for two-bit
//! images), so that is how a material finds its image.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use crate::bytes::{fixed_name, u16_at, u32_at};

/// The size of one image header: a 16-byte name, two offsets, a two-byte unknown and the info word.
pub const HEADER_LEN: usize = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 5 bits of palette index, 3 bits of alpha.
    A3I5,
    TwoBits,
    FourBits,
    EightBits,
    Compressed,
    /// 3 bits of palette index, 5 bits of alpha.
    A5I3,
    /// No palette: one 16-bit colour per pixel.
    Direct,
}

impl Format {
    fn from_raw(raw: u16) -> Result<Self> {
        Ok(match raw {
            1 => Format::A3I5,
            2 => Format::TwoBits,
            3 => Format::FourBits,
            4 => Format::EightBits,
            5 => Format::Compressed,
            6 => Format::A5I3,
            7 => Format::Direct,
            other => bail!("unknown texture format {other}"),
        })
    }

    /// How a palette offset relates to the mesh's material key.
    fn palette_shift(self) -> u32 {
        match self {
            Format::TwoBits => 3,
            _ => 4,
        }
    }

    /// A readable name, for reports.
    pub fn name(self) -> &'static str {
        match self {
            Format::A3I5 => "a3i5",
            Format::TwoBits => "two-bit",
            Format::FourBits => "four-bit",
            Format::EightBits => "eight-bit",
            Format::Compressed => "compressed",
            Format::A5I3 => "a5i3",
            Format::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Image {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub format: Format,
    /// When set, palette entry 0 is transparent.
    pub transparent: bool,
    pub palette_offset: u32,
    /// The palette, expanded to RGBA8, as used for the pixels.
    pub palette: Vec<[u8; 4]>,
    /// RGBA8, row by row.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Textures {
    pub images: Vec<Image>,
}

impl Image {
    /// The image encoded as a PNG, for writing out on its own.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        encode_png(self.width as u32, self.height as u32, &self.pixels)
    }
}

/// Encodes RGBA8 pixels as a PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(rgba)?;
    }
    Ok(out)
}

impl Textures {
    /// Maps a mesh material key (`texturePaletteBase`) to its image.
    pub fn by_material_key(&self) -> HashMap<u32, &Image> {
        self.images
            .iter()
            .map(|image| (image.palette_offset >> image.format.palette_shift(), image))
            .collect()
    }
}

/// A DS colour: five bits per channel, red in the low bits.
fn color555(raw: u16) -> [u8; 4] {
    let expand = |value: u32| ((value * 255) / 31) as u8;
    [
        expand(u32::from(raw & 0x1f)),
        expand(u32::from((raw >> 5) & 0x1f)),
        expand(u32::from((raw >> 10) & 0x1f)),
        255,
    ]
}

/// Expands a palette to RGBA8.
///
/// When the image is flagged transparent, entry 0 becomes transparent: that is the DS convention, and
/// entry 0 is the colour-keyed background.
fn palette_rgba(palette: &[u8], transparent: bool) -> Vec<[u8; 4]> {
    let mut colors: Vec<[u8; 4]> = palette
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| color555(u16::from_le_bytes(*pair)))
        .collect();
    if transparent && !colors.is_empty() {
        colors[0][3] = 0;
    }
    colors
}

/// Looks up a palette entry.
fn color_at(colors: &[[u8; 4]], index: usize) -> Result<[u8; 4]> {
    colors.get(index).copied().ok_or_else(|| {
        anyhow::anyhow!(
            "palette index {index} is out of range ({} colours)",
            colors.len()
        )
    })
}

/// Turns one entry's bitmap and palette into RGBA8.
fn decode(
    format: Format,
    width: usize,
    height: usize,
    bitmap: &[u8],
    colors: &[[u8; 4]],
) -> Result<Vec<u8>> {
    if colors.is_empty() {
        bail!("the image has no palette");
    }

    let wanted = width * height;
    let mut pixels: Vec<u8> = Vec::with_capacity(wanted * 4);

    match format {
        Format::EightBits => {
            if bitmap.len() < wanted {
                bail!(
                    "an 8-bit image of {width}x{height} needs {wanted} bytes, found {}",
                    bitmap.len()
                );
            }
            for &index in &bitmap[..wanted] {
                pixels.extend_from_slice(&color_at(colors, usize::from(index))?);
            }
        }
        Format::FourBits => {
            // the low nibble comes first
            'outer: for &byte in bitmap {
                for value in [byte & 0x0f, byte >> 4] {
                    pixels.extend_from_slice(&color_at(colors, usize::from(value))?);
                    if pixels.len() == wanted * 4 {
                        break 'outer;
                    }
                }
            }
        }
        Format::TwoBits => {
            'outer: for &byte in bitmap {
                for shift in [0, 2, 4, 6] {
                    pixels.extend_from_slice(&color_at(
                        colors,
                        usize::from((byte >> shift) & 0b11),
                    )?);
                    if pixels.len() == wanted * 4 {
                        break 'outer;
                    }
                }
            }
        }
        Format::A3I5 | Format::A5I3 => {
            let (index_mask, alpha_shift, alpha_max) = match format {
                Format::A3I5 => (0x1f_u8, 5, 7.0),
                _ => (0x07, 3, 31.0),
            };
            for &byte in bitmap.iter().take(wanted) {
                let index = usize::from(byte & index_mask);
                let mut color = color_at(colors, index)?;
                color[3] = (f64::from(byte >> alpha_shift) / alpha_max * 255.0).round() as u8;
                pixels.extend_from_slice(&color);
            }
        }
        Format::Direct => {
            if bitmap.len() < wanted * 2 {
                bail!("a direct image of {width}x{height} needs {} bytes", wanted * 2);
            }
            for pair in bitmap[..wanted * 2].as_chunks::<2>().0 {
                let color = color555(u16::from_le_bytes(*pair));
                pixels.extend_from_slice(&color);
            }
        }
        Format::Compressed => {
            bail!("4x4 compressed textures are not implemented (and do not occur in this game)")
        }
    }

    if pixels.len() != wanted * 4 {
        bail!(
            "decoding a {width}x{height} image produced {} pixels",
            pixels.len() / 4
        );
    }

    Ok(pixels)
}

/// A zero offset after the first describes an empty entry, so point it at the next one: its length
/// then comes out as zero.
fn fix_zero_offsets(offsets: &mut [usize], section_length: usize) {
    for index in 1..offsets.len() {
        if offsets[index] == 0 {
            offsets[index] = offsets.get(index + 1).copied().unwrap_or(section_length);
        }
    }
}

/// The `[start, end)` byte range of entry `index` inside its section.
fn span(offsets: &[usize], index: usize, section_length: usize) -> Result<(usize, usize)> {
    let start = offsets.get(index).copied().unwrap_or(0);
    let end = offsets.get(index + 1).copied().unwrap_or(section_length);
    if start > end || end > section_length {
        bail!("entry {index} spans {start:#x}..{end:#x} of a {section_length:#x} byte section");
    }
    Ok((start, end))
}

/// Parses a texture block, decoding every image to RGBA8.
pub fn parse(data: &[u8]) -> Result<Textures> {
    if data.len() < 0x0c {
        bail!(
            "texture block is shorter than its header ({} bytes)",
            data.len()
        );
    }

    let image_count = u32_at(data, 0x00)? as usize;
    let bitmaps_length = u32_at(data, 0x04)? as usize;
    let palettes_length = u32_at(data, 0x08)? as usize;

    let bitmaps_start = 0x0c + image_count * HEADER_LEN;
    let palettes_start = bitmaps_start + bitmaps_length;
    let end = palettes_start + palettes_length;

    if end > data.len() {
        bail!(
            "texture block declares {image_count} images, {bitmaps_length} bitmap bytes and \
             {palettes_length} palette bytes, reaching {end:#x} of a {} byte entry",
            data.len()
        );
    }

    // Read the headers first: a zero offset means "runs to the next one", so the whole list is needed
    // before any range can be resolved.
    let mut headers = Vec::with_capacity(image_count);
    let mut bitmap_offsets = Vec::with_capacity(image_count);
    let mut palette_offsets = Vec::with_capacity(image_count);
    for index in 0..image_count {
        let base = 0x0c + index * HEADER_LEN;
        headers.push((
            fixed_name(data, base, 16)?,
            u16_at(data, base + 0x1a)?,
        ));
        bitmap_offsets.push(u32_at(data, base + 0x10)? as usize);
        palette_offsets.push(u32_at(data, base + 0x14)? as usize);
    }
    fix_zero_offsets(&mut bitmap_offsets, bitmaps_length);
    fix_zero_offsets(&mut palette_offsets, palettes_length);

    let mut images = Vec::with_capacity(image_count);
    for (index, (name, info)) in headers.into_iter().enumerate() {
        let width = 8usize << ((info >> 4) & 0b111);
        let height = 8usize << ((info >> 7) & 0b111);
        let format = Format::from_raw((info >> 10) & 0b111)?;
        let transparent = (info >> 13) & 1 > 0;

        let (bitmap_start, bitmap_end) = span(&bitmap_offsets, index, bitmaps_length)?;
        let (palette_start, palette_end) = span(&palette_offsets, index, palettes_length)?;

        let bitmap = &data[bitmaps_start + bitmap_start..bitmaps_start + bitmap_end];
        let palette_bytes = &data[palettes_start + palette_start..palettes_start + palette_end];
        let palette = palette_rgba(palette_bytes, transparent);

        let pixels = decode(format, width, height, bitmap, &palette)
            .with_context(|| format!("image {index} (`{name}`)"))?;

        images.push(Image {
            name,
            width,
            height,
            format,
            transparent,
            palette_offset: palette_offsets[index] as u32,
            palette,
            pixels,
        });
    }

    Ok(Textures { images })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(data: &mut [u8], at: usize, value: u32) {
        data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// One 8x8 image: the rectangle bits are zero, so width and height are 8.
    fn sample(format_bits: u16, transparent: bool, bitmap: &[u8], palette: &[u16]) -> Vec<u8> {
        let bitmaps_start = 0x0c + HEADER_LEN;
        let palettes_start = bitmaps_start + bitmap.len();
        let mut file = vec![0u8; palettes_start + palette.len() * 2];

        put_u32(&mut file, 0x00, 1);
        put_u32(&mut file, 0x04, bitmap.len() as u32);
        put_u32(&mut file, 0x08, (palette.len() * 2) as u32);

        file[0x0c..0x0c + 8].copy_from_slice(b"din030_a");
        put_u32(&mut file, 0x0c + 0x10, 0); // bitmap offset
        put_u32(&mut file, 0x0c + 0x14, 0); // palette offset
        let info = (format_bits << 10) | (u16::from(transparent) << 13);
        file[0x0c + 0x1a..0x0c + 0x1c].copy_from_slice(&info.to_le_bytes());

        file[bitmaps_start..bitmaps_start + bitmap.len()].copy_from_slice(bitmap);
        for (index, color) in palette.iter().enumerate() {
            let at = palettes_start + index * 2;
            file[at..at + 2].copy_from_slice(&color.to_le_bytes());
        }

        file
    }

    #[test]
    fn decodes_an_eight_bit_image() {
        let palette = [0x001f, 0x03e0, 0x7c00, 0x7fff];
        let mut bitmap = vec![0u8; 64];
        bitmap[0] = 1;
        bitmap[1] = 2;
        bitmap[2] = 3;

        let textures = parse(&sample(4, true, &bitmap, &palette)).unwrap();
        assert_eq!(textures.images.len(), 1);

        let image = &textures.images[0];
        assert_eq!(image.name, "din030_a");
        assert_eq!((image.width, image.height), (8, 8));
        assert_eq!(image.format, Format::EightBits);
        assert!(image.transparent);
        assert_eq!(image.pixels.len(), 64 * 4);

        // the channels expand from five bits
        assert_eq!(image.palette[0], [255, 0, 0, 0], "entry 0 is the transparent key");
        assert_eq!(image.palette[1], [0, 255, 0, 255]);

        // bitmap[0] is index 1, bitmap[1] is index 2, bitmap[2] is index 3, the rest are index 0
        assert_eq!(&image.pixels[0..4], &[0, 255, 0, 255]);
        assert_eq!(&image.pixels[4..8], &[0, 0, 255, 255]);
        assert_eq!(&image.pixels[8..12], &[255, 255, 255, 255]);
        assert_eq!(&image.pixels[12..16], &[255, 0, 0, 0]);
    }

    #[test]
    fn four_bit_pixels_take_the_low_nibble_first() {
        let palette = [0x001f, 0x03e0, 0x7c00, 0x7fff];
        let mut bitmap = vec![0u8; 32]; // 8x8 at four bits per pixel
        bitmap[0] = 0x21; // low nibble 1, then high nibble 2

        let textures = parse(&sample(3, false, &bitmap, &palette)).unwrap();
        let image = &textures.images[0];
        assert_eq!(image.format, Format::FourBits);
        assert_eq!(image.pixels.len(), 64 * 4);
        assert_eq!(&image.pixels[0..4], &[0, 255, 0, 255]);
        assert_eq!(&image.pixels[4..8], &[0, 0, 255, 255]);
    }

    #[test]
    fn a_material_key_is_the_palette_offset_divided_by_sixteen() {
        let textures = parse(&sample(4, false, &[0u8; 64], &[0x001f])).unwrap();
        let by_key = textures.by_material_key();
        assert!(by_key.contains_key(&0), "palette offset 0 gives the key 0");
        assert_eq!(by_key[&0].name, "din030_a");
    }

    #[test]
    fn rejects_a_block_whose_sections_do_not_fit() {
        let mut file = sample(4, false, &[0u8; 64], &[0x001f]);
        put_u32(&mut file, 0x04, 10_000);
        assert!(parse(&file).is_err());
    }

    #[test]
    fn rejects_a_bitmap_that_is_too_small_for_its_size() {
        let mut file = sample(4, false, &[0u8; 64], &[0x001f]);
        // claim 16x16 while only 64 bytes of bitmap exist
        let info = (4u16 << 10) | (1u16 << 4);
        file[0x0c + 0x1a..0x0c + 0x1c].copy_from_slice(&info.to_le_bytes());
        assert!(parse(&file).is_err());
    }
}
