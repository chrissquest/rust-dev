//! Sprites: textured quads, described as plain data.
//!
//! A [`Sprite`] is one quad in world space. It has a full 3D position even though
//! the 2D games built on this draw them flat: `z` is height above the ground, so a
//! straight-down camera sees today's top-down picture and a tilted one later sees
//! the same data with depth. Nothing here knows about wgpu; `render2d` uploads a
//! slice of sprites as one instance buffer and draws it in one call.
//!
//! A [`Sheet`] is the CPU-side description of a sprite sheet: its size in pixels and
//! how to turn a pixel rectangle into UVs. Pixel rectangles use image coordinates
//! (origin top-left, y down), which is what PNG files and art tools use.

use std::path::Path;

use anyhow::{Context, Result, bail};
use bytemuck::{Pod, Zeroable};

use crate::math::Vec3;
use crate::scene::{Texture, TextureFilter};

/// One quad. `#[repr(C)]` and `Pod`: a slice of these is the instance buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Sprite {
    /// World position of the pivot. x right, y up, z height above the ground.
    pub position: [f32; 3],
    /// Counter-clockwise rotation about the pivot, in radians.
    pub rotation: f32,
    /// Width and height in world units. A negative value mirrors that axis.
    pub size: [f32; 2],
    /// Which point of the quad sits at `position`: (0, 0) bottom-left,
    /// (0.5, 0.5) centre, (1, 1) top-right.
    pub pivot: [f32; 2],
    /// `[u0, v0, u1, v1]`: top-left and bottom-right corners in texture space.
    pub uv: [f32; 4],
    /// Multiplied with the texel. Straight (not premultiplied) alpha.
    pub color: [f32; 4],
}

impl Sprite {
    pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    /// A centred, untinted, unrotated sprite showing `uv`.
    pub fn new(position: Vec3, size: [f32; 2], uv: [f32; 4]) -> Self {
        Self {
            position: position.to_array(),
            rotation: 0.0,
            size,
            pivot: [0.5, 0.5],
            uv,
            color: Self::WHITE,
        }
    }

    pub fn with_pivot(mut self, pivot: [f32; 2]) -> Self {
        self.pivot = pivot;
        self
    }

    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    pub fn with_rotation(mut self, radians: f32) -> Self {
        self.rotation = radians;
        self
    }

    /// Mirrors the picture left-to-right without moving the quad.
    pub fn flipped_x(mut self) -> Self {
        self.uv.swap(0, 2);
        self
    }
}

/// A sprite sheet's dimensions, and how its pixels map to world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sheet {
    pub width: u32,
    pub height: u32,
    /// How many sheet pixels make one world unit.
    pub pixels_per_unit: f32,
}

impl Sheet {
    pub fn new(width: u32, height: u32, pixels_per_unit: f32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            pixels_per_unit,
        }
    }

    pub fn of(texture: &Texture, pixels_per_unit: f32) -> Self {
        Self::new(texture.width, texture.height, pixels_per_unit)
    }

    /// UVs of a pixel rectangle (image coordinates: origin top-left, y down).
    pub fn uv(&self, x: u32, y: u32, width: u32, height: u32) -> [f32; 4] {
        let w = self.width as f32;
        let h = self.height as f32;
        [
            x as f32 / w,
            y as f32 / h,
            (x + width) as f32 / w,
            (y + height) as f32 / h,
        ]
    }

    /// World size of a pixel rectangle.
    pub fn size(&self, width: u32, height: u32) -> [f32; 2] {
        [
            width as f32 / self.pixels_per_unit,
            height as f32 / self.pixels_per_unit,
        ]
    }

    /// The sheet cut into equal cells, read left to right, top to bottom.
    pub fn grid(&self, cell_width: u32, cell_height: u32) -> Grid {
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);
        Grid {
            sheet: *self,
            cell_width,
            cell_height,
            columns: self.width / cell_width,
            rows: self.height / cell_height,
        }
    }
}

/// A sheet of equal cells: a tileset, or the frames of a flipbook animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub sheet: Sheet,
    pub cell_width: u32,
    pub cell_height: u32,
    pub columns: u32,
    pub rows: u32,
}

impl Grid {
    pub fn len(&self) -> u32 {
        self.columns * self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// UVs of cell `index`. Out-of-range indices wrap, so a bad frame number shows
    /// a wrong picture rather than sampling outside the sheet.
    pub fn uv(&self, index: u32) -> [f32; 4] {
        if self.is_empty() {
            return [0.0, 0.0, 1.0, 1.0];
        }
        let index = index % self.len();
        self.sheet.uv(
            (index % self.columns) * self.cell_width,
            (index / self.columns) * self.cell_height,
            self.cell_width,
            self.cell_height,
        )
    }

    /// World size of one cell.
    pub fn cell_size(&self) -> [f32; 2] {
        self.sheet.size(self.cell_width, self.cell_height)
    }
}

/// Decodes a PNG file into an RGBA8 texture that samples with hard pixel edges.
pub fn load_png(path: &Path) -> Result<Texture> {
    let bytes = std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    decode_png(&name, &bytes).with_context(|| format!("could not decode {}", path.display()))
}

/// Decodes PNG bytes into an RGBA8 texture with nearest filtering.
pub fn decode_png(name: &str, bytes: &[u8]) -> Result<Texture> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Palettes and low bit depths become plain 8-bit channels; 16-bit is narrowed.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());

    let pixel_count = info.width as usize * info.height as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    match info.color_type {
        png::ColorType::Rgba => rgba = buffer,
        png::ColorType::Rgb => {
            for pixel in buffer.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in buffer.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for &grey in &buffer {
                rgba.extend_from_slice(&[grey, grey, grey, 255]);
            }
        }
        png::ColorType::Indexed => bail!("palette PNG was not expanded"),
    }
    if rgba.len() != pixel_count * 4 {
        bail!("decoded {} bytes for {}x{} pixels", rgba.len(), info.width, info.height);
    }

    Ok(Texture {
        name: name.to_string(),
        width: info.width,
        height: info.height,
        rgba,
        filter: TextureFilter::Nearest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_is_sixty_four_bytes() {
        // The shader's instance layout is written against these offsets.
        assert_eq!(std::mem::size_of::<Sprite>(), 64);
    }

    #[test]
    fn grid_cells_read_left_to_right_then_down() {
        let grid = Sheet::new(64, 32, 64.0).grid(32, 32);
        assert_eq!(grid.len(), 2);
        assert_eq!(grid.uv(0), [0.0, 0.0, 0.5, 1.0]);
        assert_eq!(grid.uv(1), [0.5, 0.0, 1.0, 1.0]);
        // Wraps instead of leaving the sheet.
        assert_eq!(grid.uv(2), grid.uv(0));
        assert_eq!(grid.cell_size(), [0.5, 0.5]);
    }

    #[test]
    fn flipping_swaps_the_u_edges_only() {
        let sprite = Sprite::new(Vec3::ZERO, [1.0, 1.0], [0.25, 0.0, 0.5, 1.0]).flipped_x();
        assert_eq!(sprite.uv, [0.5, 0.0, 0.25, 1.0]);
    }

    #[test]
    fn decodes_rgb_and_adds_opaque_alpha() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3, 4, 5, 6]).unwrap();
        }
        let texture = decode_png("two", &bytes).unwrap();
        assert_eq!((texture.width, texture.height), (2, 1));
        assert_eq!(texture.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(texture.filter, TextureFilter::Nearest);
    }
}
