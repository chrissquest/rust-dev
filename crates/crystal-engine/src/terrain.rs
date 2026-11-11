//! Heightmap terrain with pixel-art splat blending: folder on disk -> [`Terrain`].
//!
//! A terrain is data, not a baked mesh:
//!
//! ```text
//! <folder>/
//!   terrain.json   size, height range, layers, optional props .glb
//!   height.png     greyscale (8 or 16 bit), one sample per grid vertex
//!   splat.png      RGBA weights for layers 0-3, covering the whole patch
//!   splat2.png     optional, RGBA weights for layers 4-7
//! ```
//!
//! Image row 0 is the patch's -Z edge (north), column 0 its -X edge, so a top-down
//! view of the PNG matches a top-down view of the world. The shader (see
//! `shaders/terrain.wgsl`) picks one layer per *texel* - never a colour mix - which
//! is what keeps borders crisp and on-palette. Like the rest of the CPU side, this
//! module knows nothing about wgpu.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::math::{Aabb, Vec3};
use crate::scene::{Primitive, Scene, Texture, TextureFilter};

/// The engine supports two RGBA splat maps, so up to eight layers.
pub const MAX_LAYERS: usize = 8;

pub const MANIFEST: &str = "terrain.json";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// World size of the patch in metres, `[x, z]`, centred on the origin.
    size: [f32; 2],
    /// Heights the 0 and max values of `height.png` stand for.
    height_range: [f32; 2],
    #[serde(default = "default_height")]
    height: String,
    #[serde(default = "default_splat")]
    splat: String,
    #[serde(default)]
    splat2: Option<String>,
    #[serde(default)]
    blend: BlendSettings,
    layers: Vec<LayerManifest>,
    /// A glTF scene (props, water...) loaded alongside, in the same world space.
    #[serde(default)]
    props: Option<String>,
}

fn default_height() -> String {
    String::from("height.png")
}

fn default_splat() -> String {
    String::from("splat.png")
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerManifest {
    name: String,
    /// Path to the layer's tiling texture, relative to `terrain.json`.
    texture: String,
    /// How strongly the texel's brightness lifts this layer over its neighbours.
    #[serde(default = "default_gain")]
    gain: f32,
    /// Weight added on slopes: `smoothstep(slope[0], slope[1], 1 - normal.y)`.
    #[serde(default)]
    slope: Option<[f32; 2]>,
}

fn default_gain() -> f32 {
    0.35
}

/// Look parameters for the splat blend; the defaults match `mock_transitions.py`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BlendSettings {
    /// Texture texels per metre; blend decisions snap to this grid.
    pub texels_per_metre: f32,
    /// Metres one repeat of a layer texture covers.
    pub tile_metres: f32,
    /// Amplitude of the low-frequency noise that makes borders meander.
    pub wobble: f32,
    /// Period of that noise, in metres.
    pub wobble_period: f32,
    /// Amplitude of the per-texel noise that breaks up straight runs.
    pub grain: f32,
    /// Brightness multiplier for a texel next to a higher layer (1 = off).
    pub contact_shadow: f32,
}

impl Default for BlendSettings {
    fn default() -> Self {
        Self {
            texels_per_metre: 16.0,
            tile_metres: 2.0,
            wobble: 0.45,
            wobble_period: 0.8,
            grain: 0.10,
            contact_shadow: 0.62,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TerrainLayer {
    pub name: String,
    /// sRGB RGBA8; every layer of one terrain has the same size.
    pub texture: Texture,
    pub gain: f32,
    pub slope: Option<[f32; 2]>,
}

/// A loaded terrain patch, already in world space.
#[derive(Clone, Debug)]
pub struct Terrain {
    pub name: String,
    /// `[min_x, min_z, size_x, size_z]` of the patch.
    pub rect: [f32; 4],
    pub blend: BlendSettings,
    pub layers: Vec<TerrainLayer>,
    /// Linear RGBA8 weights for layers 0-3 and 4-7; the second is all zero when unused.
    pub splats: [Texture; 2],
    /// World-space grid mesh with smooth normals. `uvs` and skinning are unused.
    pub mesh: Primitive,
}

impl Terrain {
    pub fn bounds(&self) -> Aabb {
        let mut bounds = Aabb::empty();
        for position in &self.mesh.positions {
            bounds.expand(Vec3::from_array(*position));
        }
        bounds
    }
}

/// True for a folder that holds a `terrain.json`.
pub fn is_terrain_folder(path: &Path) -> bool {
    path.is_dir() && path.join(MANIFEST).is_file()
}

/// Loads a terrain folder - plus its `props` scene, when it names one - as a scene.
pub fn load(folder: &Path) -> Result<Scene> {
    let manifest_path = folder.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("could not read {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a valid terrain manifest", manifest_path.display()))?;

    let terrain = build(folder, &manifest)?;

    let mut scene = match &manifest.props {
        Some(props) => {
            let path = folder.join(props);
            crate::loader::load(&path)
                .with_context(|| format!("could not load the terrain's props {}", path.display()))?
        }
        None => Scene::default(),
    };
    scene.terrains.push(terrain);
    scene.source = folder.display().to_string();
    scene.recompute_bounds();
    Ok(scene)
}

fn build(folder: &Path, manifest: &Manifest) -> Result<Terrain> {
    ensure!(!manifest.layers.is_empty(), "a terrain needs at least one layer");
    ensure!(
        manifest.layers.len() <= MAX_LAYERS,
        "a terrain has at most {MAX_LAYERS} layers, this one has {}",
        manifest.layers.len()
    );
    ensure!(
        manifest.layers.len() <= 4 || manifest.splat2.is_some(),
        "layers 5-8 need a `splat2` image"
    );
    let [size_x, size_z] = manifest.size;
    ensure!(size_x > 0.0 && size_z > 0.0, "terrain size must be positive");

    let height = read_png(&folder.join(&manifest.height))?;
    ensure!(
        height.width >= 2 && height.height >= 2,
        "{} needs at least 2x2 samples",
        manifest.height
    );
    let heights = height.grey_unit();
    let [low, high] = manifest.height_range;
    let heights: Vec<f32> = heights.iter().map(|&h| low + h * (high - low)).collect();

    let rect = [-size_x / 2.0, -size_z / 2.0, size_x, size_z];
    let mesh = grid_mesh(&heights, height.width as usize, height.height as usize, rect);

    let mut layers = Vec::with_capacity(manifest.layers.len());
    for layer in &manifest.layers {
        let path = folder.join(&layer.texture);
        let image = read_png(&path)?;
        let texture = Texture {
            name: layer.name.clone(),
            width: image.width,
            height: image.height,
            rgba: image.rgba8(),
            filter: TextureFilter::Nearest,
        };
        if let Some(first) = layers.first() {
            let first: &TerrainLayer = first;
            ensure!(
                (texture.width, texture.height) == (first.texture.width, first.texture.height),
                "layer `{}` is {}x{} but `{}` is {}x{}; all layers must match",
                layer.name,
                texture.width,
                texture.height,
                first.name,
                first.texture.width,
                first.texture.height
            );
        }
        layers.push(TerrainLayer {
            name: layer.name.clone(),
            texture,
            gain: layer.gain,
            slope: layer.slope,
        });
    }

    let splat = splat_texture(&folder.join(&manifest.splat), "splat")?;
    let splat2 = match &manifest.splat2 {
        Some(name) => splat_texture(&folder.join(name), "splat2")?,
        None => Texture {
            name: String::from("splat2"),
            width: splat.width,
            height: splat.height,
            rgba: vec![0; splat.rgba.len()],
            filter: TextureFilter::Linear,
        },
    };
    ensure!(
        (splat.width, splat.height) == (splat2.width, splat2.height),
        "splat and splat2 must be the same size"
    );

    Ok(Terrain {
        name: folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("terrain")),
        rect,
        blend: manifest.blend,
        layers,
        splats: [splat, splat2],
        mesh,
    })
}

fn splat_texture(path: &Path, name: &str) -> Result<Texture> {
    let image = read_png(path)?;
    Ok(Texture {
        name: name.to_string(),
        width: image.width,
        height: image.height,
        rgba: image.rgba8(),
        filter: TextureFilter::Linear,
    })
}

/// A regular grid over `rect`, one vertex per height sample, with smooth normals
/// from central differences.
fn grid_mesh(heights: &[f32], columns: usize, rows: usize, rect: [f32; 4]) -> Primitive {
    let [min_x, min_z, size_x, size_z] = rect;
    let step_x = size_x / (columns - 1) as f32;
    let step_z = size_z / (rows - 1) as f32;
    let at = |column: usize, row: usize| heights[row * columns + column];

    let mut positions = Vec::with_capacity(columns * rows);
    let mut normals = Vec::with_capacity(columns * rows);
    for row in 0..rows {
        for column in 0..columns {
            positions.push([
                min_x + column as f32 * step_x,
                at(column, row),
                min_z + row as f32 * step_z,
            ]);

            // Central differences, one-sided at the edges.
            let (left, right) = (column.saturating_sub(1), (column + 1).min(columns - 1));
            let (up, down) = (row.saturating_sub(1), (row + 1).min(rows - 1));
            let slope_x = (at(right, row) - at(left, row)) / ((right - left) as f32 * step_x);
            let slope_z = (at(column, down) - at(column, up)) / ((down - up) as f32 * step_z);
            let normal = Vec3::new(-slope_x, 1.0, -slope_z).normalize();
            normals.push(normal.to_array());
        }
    }

    let mut indices = Vec::with_capacity((columns - 1) * (rows - 1) * 6);
    for row in 0..rows - 1 {
        for column in 0..columns - 1 {
            let a = (row * columns + column) as u32;
            let b = a + 1;
            let c = a + columns as u32;
            let d = c + 1;
            // Alternate the diagonal so long slopes do not all crease the same way.
            if (row + column) % 2 == 0 {
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            } else {
                indices.extend_from_slice(&[a, c, d, a, d, b]);
            }
        }
    }

    Primitive {
        positions,
        normals,
        indices,
        ..Default::default()
    }
}

struct Png {
    width: u32,
    height: u32,
    channels: usize,
    sixteen_bit: bool,
    data: Vec<u8>,
}

impl Png {
    fn sample(&self, index: usize, channel: usize) -> f32 {
        let offset = index * self.channels + channel;
        if self.sixteen_bit {
            u16::from_be_bytes([self.data[offset * 2], self.data[offset * 2 + 1]]) as f32 / 65535.0
        } else {
            self.data[offset] as f32 / 255.0
        }
    }

    fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// First channel as 0..1, which is the grey value for grey images.
    fn grey_unit(&self) -> Vec<f32> {
        (0..self.pixel_count()).map(|index| self.sample(index, 0)).collect()
    }

    fn rgba8(&self) -> Vec<u8> {
        let to_byte = |value: f32| (value * 255.0).round() as u8;
        let mut out = Vec::with_capacity(self.pixel_count() * 4);
        for index in 0..self.pixel_count() {
            let pixel = match self.channels {
                1 => {
                    let grey = to_byte(self.sample(index, 0));
                    [grey, grey, grey, 255]
                }
                2 => {
                    let grey = to_byte(self.sample(index, 0));
                    [grey, grey, grey, to_byte(self.sample(index, 1))]
                }
                3 => [
                    to_byte(self.sample(index, 0)),
                    to_byte(self.sample(index, 1)),
                    to_byte(self.sample(index, 2)),
                    255,
                ],
                _ => [
                    to_byte(self.sample(index, 0)),
                    to_byte(self.sample(index, 1)),
                    to_byte(self.sample(index, 2)),
                    to_byte(self.sample(index, 3)),
                ],
            };
            out.extend_from_slice(&pixel);
        }
        out
    }
}

fn read_png(path: &Path) -> Result<Png> {
    let file = std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Palette and sub-byte images become plain 8-bit channels; 16-bit stays 16-bit.
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .with_context(|| format!("{} is not a readable PNG", path.display()))?;
    let mut data = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut data)
        .with_context(|| format!("could not decode {}", path.display()))?;
    data.truncate(info.buffer_size());

    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => bail!("{}: palette PNG was not expanded", path.display()),
    };
    Ok(Png {
        width: info.width,
        height: info.height,
        channels,
        sixteen_bit: info.bit_depth == png::BitDepth::Sixteen,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(columns: usize, rows: usize) -> Primitive {
        grid_mesh(&vec![1.5; columns * rows], columns, rows, [-4.0, -2.0, 8.0, 4.0])
    }

    #[test]
    fn grid_spans_the_rect_and_faces_up() {
        let mesh = flat(5, 3);
        assert_eq!(mesh.positions.len(), 15);
        assert_eq!(mesh.indices.len(), 4 * 2 * 6);
        assert_eq!(mesh.positions[0], [-4.0, 1.5, -2.0]);
        assert_eq!(mesh.positions[14], [4.0, 1.5, 2.0]);
        for normal in &mesh.normals {
            assert_eq!(*normal, [0.0, 1.0, 0.0]);
        }
    }

    #[test]
    fn triangles_wind_counter_clockwise_seen_from_above() {
        let mesh = flat(4, 4);
        for triangle in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from_array(mesh.positions[triangle[i] as usize]));
            assert!((b - a).cross(c - a).y > 0.0, "triangle {triangle:?} faces down");
        }
    }

    #[test]
    fn slope_normals_lean_downhill() {
        // Height rises toward +X, so the normal must lean toward -X.
        let heights: Vec<f32> = (0..9).map(|i| (i % 3) as f32).collect();
        let mesh = grid_mesh(&heights, 3, 3, [0.0, 0.0, 2.0, 2.0]);
        assert!(mesh.normals[4][0] < -0.5);
        assert!(mesh.normals[4][2].abs() < 1e-6);
    }

    fn write_png(path: &Path, width: u32, height: u32, color: png::ColorType, depth: png::BitDepth, data: &[u8]) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        encoder.write_header().unwrap().write_image_data(data).unwrap();
    }

    #[test]
    fn loads_a_folder_with_16_bit_heights() {
        let folder = std::env::temp_dir().join(format!("crystal-terrain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();

        // 3x3 heights: 0 everywhere except the centre at full scale (65535).
        let mut heights = vec![0u8; 9 * 2];
        heights[8] = 0xFF;
        heights[9] = 0xFF;
        write_png(&folder.join("height.png"), 3, 3, png::ColorType::Grayscale, png::BitDepth::Sixteen, &heights);
        write_png(&folder.join("splat.png"), 2, 2, png::ColorType::Rgba, png::BitDepth::Eight, &[255, 0, 0, 0].repeat(4));
        write_png(&folder.join("grass.png"), 2, 2, png::ColorType::Rgb, png::BitDepth::Eight, &[10, 200, 10].repeat(4));
        std::fs::write(
            folder.join(MANIFEST),
            r#"{ "size": [4, 4], "height_range": [-1, 3],
                 "layers": [ { "name": "grass", "texture": "grass.png", "gain": 0.5 } ] }"#,
        )
        .unwrap();

        assert!(is_terrain_folder(&folder));
        let scene = load(&folder).unwrap();
        let _ = std::fs::remove_dir_all(&folder);

        let terrain = &scene.terrains[0];
        assert_eq!(terrain.rect, [-2.0, -2.0, 4.0, 4.0]);
        assert_eq!(terrain.mesh.positions[4], [0.0, 3.0, 0.0]);
        assert_eq!(terrain.mesh.positions[0], [-2.0, -1.0, -2.0]);
        assert_eq!(terrain.layers[0].texture.rgba[..4], [10, 200, 10, 255]);
        assert!(terrain.splats[1].rgba.iter().all(|&byte| byte == 0));
        assert_eq!(scene.bounds.max.y, 3.0);
    }

    #[test]
    fn manifest_defaults_fill_in() {
        let manifest: Manifest = serde_json::from_str(
            r#"{ "size": [32, 32], "height_range": [0, 4],
                 "layers": [ { "name": "grass", "texture": "grass.png" } ] }"#,
        )
        .unwrap();
        assert_eq!(manifest.height, "height.png");
        assert_eq!(manifest.blend, BlendSettings::default());
        assert_eq!(manifest.layers[0].gain, 0.35);
    }
}
