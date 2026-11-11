//! The mesh command stream, and the Nintendo DS GPU command list it wraps.
//!
//! The stream is a run of commands, each introduced by two 16-bit words:
//!
//! ```text
//! 0x50  arg = 0     u32 length   opaque block
//! 0x51  arg = 0     u32 length   opaque block; its byte 12 is the world-root bone count
//! 0x52  arg = 0     u32 length   a GPU command list - this is where the geometry is
//! 0x53  arg <= ...  3 x u32      opaque
//! 0x0FFF arg = 0x7F              end of stream
//! ```
//!
//! The GPU list itself is standard DS hardware, not a bespoke format: commands arrive **four to a
//! word**, and each command's 32-bit arguments follow in order. Parameters are 20.12 fixed point for
//! matrices and 4.12 for vertex coordinates, while texture coordinates are **12.4** - units of 1/16
//! texel, which the texture size then normalises (see [`crate::fixed`]).
//!
//! Geometry comes out of a state machine that mirrors how the hardware works: `matrixRestore`
//! selects the bone the following vertices bind to, `texturePaletteBase` selects the material,
//! `textureCoordinate` sets the current UV, and each vertex command commits one vertex using that
//! state. `vertexBegin`/`vertexEnd` bracket a primitive.

use anyhow::{Result, bail};

use crate::bytes::Reader;
use crate::fixed;
use crate::matrix::Matrix4x3;
use crate::mesh::Mesh;

/// Every opcode in the DS GPU command set, with its argument width in 32-bit words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Noop,
    MatrixMode,
    MatrixPush,
    MatrixPop,
    MatrixStore,
    MatrixRestore,
    MatrixIdentity,
    MatrixLoad4x4,
    MatrixLoad4x3,
    MatrixMultiply4x4,
    MatrixMultiply4x3,
    MatrixMultiply3x3,
    MatrixScale,
    MatrixTranslate,
    Color,
    Normal,
    TextureCoordinate,
    Vertex16,
    Vertex10,
    VertexXy,
    VertexXz,
    VertexYz,
    VertexDiff,
    PolygonAttributes,
    TextureImageParameter,
    TexturePaletteBase,
    MaterialColor0,
    MaterialColor1,
    LightVector,
    LightColor,
    Shininess,
    VertexBegin,
    VertexEnd,
    SetViewport,
    TestBox,
    TestPosition,
    TestVector,
    /// An opcode this build does not know; the stream cannot be followed past it.
    Unknown(u8),
}

impl Opcode {
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0x00 => Opcode::Noop,
            0x10 => Opcode::MatrixMode,
            0x11 => Opcode::MatrixPush,
            0x12 => Opcode::MatrixPop,
            0x13 => Opcode::MatrixStore,
            0x14 => Opcode::MatrixRestore,
            0x15 => Opcode::MatrixIdentity,
            0x16 => Opcode::MatrixLoad4x4,
            0x17 => Opcode::MatrixLoad4x3,
            0x18 => Opcode::MatrixMultiply4x4,
            0x19 => Opcode::MatrixMultiply4x3,
            0x1A => Opcode::MatrixMultiply3x3,
            0x1B => Opcode::MatrixScale,
            0x1C => Opcode::MatrixTranslate,
            0x20 => Opcode::Color,
            0x21 => Opcode::Normal,
            0x22 => Opcode::TextureCoordinate,
            0x23 => Opcode::Vertex16,
            0x24 => Opcode::Vertex10,
            0x25 => Opcode::VertexXy,
            0x26 => Opcode::VertexXz,
            0x27 => Opcode::VertexYz,
            0x28 => Opcode::VertexDiff,
            0x29 => Opcode::PolygonAttributes,
            0x2A => Opcode::TextureImageParameter,
            0x2B => Opcode::TexturePaletteBase,
            0x30 => Opcode::MaterialColor0,
            0x31 => Opcode::MaterialColor1,
            0x32 => Opcode::LightVector,
            0x33 => Opcode::LightColor,
            0x34 => Opcode::Shininess,
            0x40 => Opcode::VertexBegin,
            0x41 => Opcode::VertexEnd,
            0x60 => Opcode::SetViewport,
            0x70 => Opcode::TestBox,
            0x71 => Opcode::TestPosition,
            0x72 => Opcode::TestVector,
            other => Opcode::Unknown(other),
        }
    }

    /// Arguments, in 32-bit words, that follow the command word.
    pub fn argument_words(self) -> usize {
        match self {
            Opcode::Noop | Opcode::MatrixPush | Opcode::MatrixIdentity | Opcode::VertexEnd => 0,
            Opcode::Vertex16 | Opcode::TestPosition => 2,
            Opcode::MatrixScale | Opcode::MatrixTranslate | Opcode::TestBox => 3,
            Opcode::MatrixMultiply3x3 => 9,
            Opcode::MatrixLoad4x3 | Opcode::MatrixMultiply4x3 => 12,
            Opcode::MatrixLoad4x4 | Opcode::MatrixMultiply4x4 => 16,
            Opcode::Shininess => 32,
            Opcode::Unknown(_) => 0,
            _ => 1,
        }
    }
}

/// The subset of GPU commands this crate interprets. Arguments we neither need nor trust are
/// consumed but dropped, which keeps the stream aligned.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Noop,
    MatrixRestore(u32),
    MatrixIdentity,
    /// `matrixMode`: 0 projection, 1 position, 2 texture. Kept because a matrix in *texture* mode
    /// would transform every texture coordinate, and the reference ignores these entirely.
    MatrixMode(u32),
    /// `matrixScale` in the current mode.
    MatrixScale([f64; 3]),
    MatrixLoad4x3([f64; 12]),
    TextureCoordinate([f64; 2]),
    Normal(u32),
    Vertex16([f64; 3]),
    /// `vertexXY`, `vertexXZ` or `vertexYZ`: two components replaced, the third kept.
    VertexPartial { axes: [u8; 2], values: [f64; 2] },
    TextureImageParameter(u32),
    TexturePaletteBase(u32),
    VertexBegin(u32),
    VertexEnd,
    /// Something we consume but do not interpret.
    Skipped(Opcode),
}

/// Reads two 4.12 coordinates packed into one 32-bit word.
fn read_pair(reader: &mut Reader<'_>) -> Result<[f64; 2]> {
    Ok([
        fixed::from_412(reader.u16()? as i16),
        fixed::from_412(reader.u16()? as i16),
    ])
}

/// Reads a texture coordinate: two **12.4** values, i.e. units of 1/16 texel, not the 4.12 the vertex
/// commands use.
fn read_uv(reader: &mut Reader<'_>) -> Result<[f64; 2]> {
    Ok([
        fixed::from_124(reader.u16()? as i16),
        fixed::from_124(reader.u16()? as i16),
    ])
}

/// Parses the GPU command list held by a `0x52` block.
pub fn parse_gpu_commands(data: &[u8]) -> Result<Vec<Command>> {
    let mut reader = Reader::new(data);
    let mut commands = Vec::new();

    // Commands arrive four to a word, so the loop stops on a whole-word boundary.
    while reader.remaining() >= 4 {
        let mut opcodes = [Opcode::Noop; 4];
        for slot in &mut opcodes {
            *slot = Opcode::from_byte(reader.u8()?);
        }

        for opcode in opcodes {
            let command = match opcode {
                Opcode::Noop => Command::Noop,
                Opcode::MatrixRestore => Command::MatrixRestore(reader.u32()?),
                Opcode::MatrixIdentity => Command::MatrixIdentity,
                Opcode::MatrixMode => Command::MatrixMode(reader.u32()?),
                Opcode::MatrixScale => {
                    let mut values = [0.0f64; 3];
                    for value in &mut values {
                        *value = fixed::from_2012(reader.u32()?);
                    }
                    Command::MatrixScale(values)
                }
                Opcode::MatrixLoad4x3 => {
                    let mut values = [0.0f64; 12];
                    for value in &mut values {
                        *value = fixed::from_2012(reader.u32()?);
                    }
                    Command::MatrixLoad4x3(values)
                }
                Opcode::TextureCoordinate => Command::TextureCoordinate(read_uv(&mut reader)?),
                Opcode::Normal => Command::Normal(reader.u32()?),
                Opcode::Vertex16 => {
                    let vertex = [
                        fixed::from_412(reader.u16()? as i16),
                        fixed::from_412(reader.u16()? as i16),
                        fixed::from_412(reader.u16()? as i16),
                    ];
                    reader.skip(2)?; // the fourth 16-bit slot is unused
                    Command::Vertex16(vertex)
                }
                Opcode::VertexXy => Command::VertexPartial {
                    axes: [0, 1],
                    values: read_pair(&mut reader)?,
                },
                Opcode::VertexXz => Command::VertexPartial {
                    axes: [0, 2],
                    values: read_pair(&mut reader)?,
                },
                Opcode::VertexYz => Command::VertexPartial {
                    axes: [1, 2],
                    values: read_pair(&mut reader)?,
                },
                Opcode::TextureImageParameter => Command::TextureImageParameter(reader.u32()?),
                Opcode::TexturePaletteBase => Command::TexturePaletteBase(reader.u32()?),
                Opcode::VertexBegin => Command::VertexBegin(reader.u32()?),
                Opcode::VertexEnd => Command::VertexEnd,
                Opcode::Unknown(byte) => {
                    bail!(
                        "unknown GPU command {byte:#04x} at offset {:#x}; the stream cannot be followed past it",
                        reader.pos() - 1
                    );
                }
                other => {
                    reader.skip(other.argument_words() * 4)?;
                    Command::Skipped(other)
                }
            };
            commands.push(command);
        }

        // When the last real command of a word takes no arguments the word is padded with four bytes.
        let last = opcodes
            .iter()
            .rev()
            .find(|opcode| **opcode != Opcode::Noop)
            .copied();
        if let Some(last) = last {
            if last.argument_words() == 0 && reader.remaining() >= 4 {
                reader.skip(4)?;
            }
        }
    }

    Ok(commands)
}

/// One command of the outer mesh command stream.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshCommand {
    /// Opaque block introduced by `0x50`.
    Block50(Vec<u8>),
    /// Opaque block introduced by `0x51`; byte 12 is the world-root bone count.
    Block51(Vec<u8>),
    /// The GPU command list from a `0x52` command - the geometry.
    Gpu(Vec<Command>),
    /// A `0x53` command: a 16-bit argument followed by three opaque words.
    Opaque53 { arg: u16, words: [u32; 3] },
    End,
}

#[derive(Debug, Clone, Default)]
pub struct Stream {
    pub commands: Vec<MeshCommand>,
}

impl Stream {
    /// How many bones precede the mesh's own bone table.
    ///
    /// The DS matrix stack starts above these, which is why `matrixRestore` indices are offset in
    /// [`build_geometry`].
    pub fn world_root_bone_count(&self) -> Result<usize> {
        for command in &self.commands {
            if let MeshCommand::Block51(bytes) = command {
                if bytes.len() > 12 {
                    return Ok(bytes[12] as usize);
                }
            }
        }
        bail!("the mesh command stream has no 0x51 block, so the world-root bone count is unknown")
    }

    /// The GPU command list, which holds all of the geometry.
    pub fn gpu_commands(&self) -> Result<&[Command]> {
        for command in &self.commands {
            if let MeshCommand::Gpu(commands) = command {
                return Ok(commands);
            }
        }
        bail!("the mesh command stream has no 0x52 block, so it holds no geometry")
    }
}

/// Parses the outer mesh command stream down to its `0x52` GPU block.
pub fn parse_stream(data: &[u8]) -> Result<Stream> {
    let mut reader = Reader::new(data);
    let mut commands = Vec::new();

    loop {
        let id = reader.u16()?;
        let arg = reader.u16()?;
        match id {
            0x50 => {
                let length = reader.u32()? as usize;
                commands.push(MeshCommand::Block50(reader.take(length)?.to_vec()));
            }
            0x51 => {
                let length = reader.u32()? as usize;
                commands.push(MeshCommand::Block51(reader.take(length)?.to_vec()));
            }
            0x52 => {
                let length = reader.u32()? as usize;
                let block = reader.take(length)?;
                commands.push(MeshCommand::Gpu(parse_gpu_commands(block)?));
            }
            0x53 => {
                let words = [reader.u32()?, reader.u32()?, reader.u32()?];
                commands.push(MeshCommand::Opaque53 { arg, words });
            }
            0x0FFF => {
                if arg != 0x7F {
                    bail!("mesh end marker has argument {arg:#06x}, expected 0x7f");
                }
                commands.push(MeshCommand::End);
                break;
            }
            other => bail!("unknown mesh command {other:#06x} at offset {:#x}", reader.pos() - 4),
        }
    }

    Ok(Stream { commands })
}

/// One corner of a polygon: a vertex plus the UV that was current when it was committed.
#[derive(Debug, Clone, PartialEq)]
pub struct Corner {
    pub vertex: usize,
    pub uv: Option<usize>,
}

/// The polygons that share one material.
#[derive(Debug, Clone)]
pub struct MaterialGroup {
    /// The `texturePaletteBase` value, which is how the texture set identifies the material.
    pub palette_base: Option<u32>,
    pub polygons: Vec<Vec<Corner>>,
}

#[derive(Debug, Clone, Default)]
pub struct Geometry {
    /// Unique (position, bone) pairs - the vertex list.
    pub positions: Vec<[f64; 3]>,
    /// The bone each entry of `positions` is bound to; skinning in this game is rigid.
    pub bones: Vec<usize>,
    /// Unique texture coordinates, in order of first use.
    pub uvs: Vec<[f64; 2]>,
    /// Polygons grouped by material, in order of first use.
    pub groups: Vec<MaterialGroup>,
}

/// How the DS's texture coordinates map onto the target format.
///
/// The DS samples a texture with (0, 0) at its **top-left** and V growing **downwards**, because a
/// texture's first byte is its top-left texel and the hardware's T axis runs down from there. glTF
/// uses the same convention, so a faithful port needs no change at all.
///
/// OBJ and COLLADA are the odd ones out - their v runs *upwards* from a bottom-left origin - and that
/// is where the flip in the reference tool came from: it was written to write OBJ, flipped V for it,
/// and then left the flip in place for its USD output too (its own comment wonders about it). Our
/// output matched that flip exactly, which is why the textures came out mirrored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UvOrientation {
    /// Leave the coordinates alone: what the DS hardware sampled and what glTF expects. The default.
    #[default]
    Ds,
    /// `1 - v`, which is what the reference tool writes for every format.
    FlipV,
    /// `1 - u`.
    FlipU,
    /// Swap `u` and `v`.
    Swap,
}

impl UvOrientation {
    /// Parses a `--uv-flip` value.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "none" | "ds" => Some(UvOrientation::Ds),
            "v" | "flip-v" => Some(UvOrientation::FlipV),
            "u" | "flip-u" => Some(UvOrientation::FlipU),
            "swap" => Some(UvOrientation::Swap),
            _ => None,
        }
    }

    /// A short label, for reports and the command line.
    pub fn name(self) -> &'static str {
        match self {
            UvOrientation::Ds => "none",
            UvOrientation::FlipV => "v",
            UvOrientation::FlipU => "u",
            UvOrientation::Swap => "swap",
        }
    }

    /// Applies the mapping to one normalised texture coordinate.
    pub fn apply(self, uv: [f64; 2]) -> [f64; 2] {
        match self {
            UvOrientation::Ds => uv,
            UvOrientation::FlipV => [uv[0], 1.0 - uv[1]],
            UvOrientation::FlipU => [1.0 - uv[0], uv[1]],
            UvOrientation::Swap => [uv[1], uv[0]],
        }
    }
}


impl Geometry {
    pub fn face_count(&self) -> usize {
        self.groups.iter().map(|group| group.polygons.len()).sum()
    }

    /// Total polygon corners, which is how many UV entries a renderer needs.
    pub fn corner_count(&self) -> usize {
        self.groups
            .iter()
            .flat_map(|group| &group.polygons)
            .map(|polygon| polygon.len())
            .sum()
    }

    /// Corners, minus two per polygon, i.e. the triangle count after fan triangulation.
    pub fn triangle_count(&self) -> usize {
        self.groups
            .iter()
            .flat_map(|group| &group.polygons)
            .map(|polygon| polygon.len().saturating_sub(2))
            .sum()
    }

    /// The range the texture coordinates cover, as (min, max).
    ///
    /// Worth reporting: a creature whose whole model samples a fraction of one texel renders as a
    /// single blurry colour, which is exactly what a wrong fixed-point scale does.
    pub fn uv_bounds(&self) -> Option<([f64; 2], [f64; 2])> {
        let first = self.uvs.first()?;
        let mut min = *first;
        let mut max = *first;
        for uv in &self.uvs {
            for axis in 0..2 {
                min[axis] = min[axis].min(uv[axis]);
                max[axis] = max[axis].max(uv[axis]);
            }
        }
        Some((min, max))
    }

    /// The bounding box of the vertex list, as (min, max).
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let first = self.positions.first()?;
        let mut min = *first;
        let mut max = *first;
        for position in &self.positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis]);
                max[axis] = max[axis].max(position[axis]);
            }
        }
        Some((min, max))
    }
}

/// The state the hardware keeps while walking the command list.
struct State {
    vertex: [f64; 3],
    mode: Option<u32>,
    bone: Option<usize>,
    uv: [f64; 2],
    uv_scale: [f64; 2],
    material: Option<u32>,
    pending: Vec<Corner>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            vertex: [0.0; 3],
            mode: None,
            bone: None,
            uv: [0.0; 2],
            uv_scale: [1.0; 2],
            material: None,
            pending: Vec::new(),
        }
    }
}

fn index_of_position(geometry: &mut Geometry, position: [f64; 3], bone: usize) -> usize {
    let existing = geometry
        .positions
        .iter()
        .zip(geometry.bones.iter())
        .position(|(candidate, candidate_bone)| *candidate == position && *candidate_bone == bone);
    if let Some(index) = existing {
        return index;
    }
    geometry.positions.push(position);
    geometry.bones.push(bone);
    geometry.positions.len() - 1
}

fn index_of_uv(geometry: &mut Geometry, uv: [f64; 2]) -> usize {
    let existing = geometry.uvs.iter().position(|candidate| *candidate == uv);
    if let Some(index) = existing {
        return index;
    }
    geometry.uvs.push(uv);
    geometry.uvs.len() - 1
}

fn group_for(geometry: &mut Geometry, palette_base: Option<u32>) -> &mut MaterialGroup {
    let existing = geometry
        .groups
        .iter()
        .position(|group| group.palette_base == palette_base);
    if let Some(index) = existing {
        return &mut geometry.groups[index];
    }
    geometry.groups.push(MaterialGroup {
        palette_base,
        polygons: Vec::new(),
    });
    geometry.groups.last_mut().expect("just pushed")
}

fn commit_vertex(state: &mut State, geometry: &mut Geometry, matrices: &[Matrix4x3]) -> Result<()> {
    let Some(bone) = state.bone else {
        bail!(
            "a vertex was committed before any bone was selected; the reference reports this as \
             \"THE BONE IS NEGATIVE 1\""
        );
    };

    let position = matrices[bone].transform(state.vertex);
    let vertex = index_of_position(geometry, position, bone);
    let uv = match state.material {
        Some(_) => Some(index_of_uv(geometry, state.uv)),
        None => None,
    };

    state.pending.push(Corner { vertex, uv });
    Ok(())
}

fn chunk_exact(corners: &[Corner], size: usize) -> Result<Vec<Vec<Corner>>> {
    if corners.len() % size != 0 {
        bail!(
            "{} vertices do not divide into {size}-vertex primitives",
            corners.len()
        );
    }
    Ok(corners.chunks(size).map(<[Corner]>::to_vec).collect())
}

fn triangle_strip(corners: &[Corner]) -> Result<Vec<Vec<Corner>>> {
    if corners.len() < 3 {
        bail!("a triangle strip needs at least 3 vertices, got {}", corners.len());
    }
    let mut polygons = Vec::new();
    for (index, window) in corners.windows(3).enumerate() {
        // every other triangle has its winding reversed
        polygons.push(if index % 2 == 0 {
            vec![window[0].clone(), window[1].clone(), window[2].clone()]
        } else {
            vec![window[1].clone(), window[0].clone(), window[2].clone()]
        });
    }
    Ok(polygons)
}

fn quad_strip(corners: &[Corner]) -> Result<Vec<Vec<Corner>>> {
    if corners.len() < 4 || corners.len() % 2 != 0 {
        bail!(
            "a quadrilateral strip needs an even count of at least 4 vertices, got {}",
            corners.len()
        );
    }
    let mut polygons = Vec::new();
    let mut index = 0;
    while index + 4 <= corners.len() {
        polygons.push(vec![
            corners[index].clone(),
            corners[index + 1].clone(),
            corners[index + 3].clone(),
            corners[index + 2].clone(),
        ]);
        index += 2;
    }
    Ok(polygons)
}

fn commit_polygons(state: &mut State, geometry: &mut Geometry) -> Result<()> {
    let Some(mode) = state.mode else {
        bail!("vertexEnd arrived before vertexBegin");
    };
    let pending = std::mem::take(&mut state.pending);

    let primitives = match mode {
        0 => chunk_exact(&pending, 3)?,
        1 => chunk_exact(&pending, 4)?,
        2 => triangle_strip(&pending)?,
        3 => quad_strip(&pending)?,
        other => bail!("unknown vertex mode {other}"),
    };

    // A primitive may name the same vertex twice - strips and degenerate triangles do. The
    // reference keeps only the first occurrence, and its UV with it.
    let polygons: Vec<Vec<Corner>> = primitives
        .into_iter()
        .map(|polygon| {
            let mut kept: Vec<Corner> = Vec::with_capacity(polygon.len());
            for corner in polygon {
                if !kept.iter().any(|existing| existing.vertex == corner.vertex) {
                    kept.push(corner);
                }
            }
            kept
        })
        .collect();

    group_for(geometry, state.material).polygons.extend(polygons);
    Ok(())
}

/// Runs a GPU command list and produces the geometry it draws.
///
/// `world_root_bone_count` comes from the mesh stream's `0x51` block. The DS matrix stack numbers
/// its first usable slot five above that count, which is the offset applied to `matrixRestore`.
pub fn build_geometry(
    mesh: &Mesh,
    world_root_bone_count: usize,
    commands: &[Command],
) -> Result<Geometry> {
    let matrices: Vec<Matrix4x3> = mesh.bones.iter().map(|bone| bone.matrix).collect();
    if matrices.is_empty() {
        bail!("the mesh has no bone table, so its vertices cannot be placed");
    }

    let mut geometry = Geometry::default();
    let mut state = State::default();

    for command in commands {
        match command {
            Command::MatrixIdentity => state.bone = None,
            Command::MatrixRestore(index) => {
                let bone = *index as i64 - 5 + world_root_bone_count as i64;
                if bone < 0 || bone as usize >= matrices.len() {
                    bail!(
                        "matrixRestore {index} selects bone {bone}, but the mesh has {} bones",
                        matrices.len()
                    );
                }
                state.bone = Some(bone as usize);
            }
            Command::TextureCoordinate(uv) => {
                // The DS's own space: origin top-left, V down, which is already what glTF wants, so
                // nothing is flipped or swapped here. See [`UvOrientation`] for why the reference tool
                // does flip, and for the flag that reproduces it.
                state.uv = [uv[0] * state.uv_scale[0], uv[1] * state.uv_scale[1]];
            }
            Command::TextureImageParameter(raw) => {
                let width_shift = (raw >> 20) & 0b111;
                let height_shift = (raw >> 23) & 0b111;
                state.uv_scale = [
                    1.0 / ((8u32 << width_shift) as f64),
                    1.0 / ((8u32 << height_shift) as f64),
                ];
            }
            Command::TexturePaletteBase(value) => state.material = Some(*value),
            Command::VertexBegin(mode) => state.mode = Some(*mode),
            Command::Vertex16(vertex) => {
                state.vertex = *vertex;
                commit_vertex(&mut state, &mut geometry, &matrices)?;
            }
            Command::VertexPartial { axes, values } => {
                for (slot, value) in axes.iter().zip(values.iter()) {
                    state.vertex[*slot as usize] = *value;
                }
                commit_vertex(&mut state, &mut geometry, &matrices)?;
            }
            Command::VertexEnd => commit_polygons(&mut state, &mut geometry)?,
            // Consumed but not acted on: no-ops, opaque skipped commands, the transform commands
            // the reference also ignores, and normals (parsed, but not yet used).
            Command::Noop
            | Command::Skipped(_)
            | Command::MatrixMode(_)
            | Command::MatrixScale(_)
            | Command::MatrixLoad4x3(_)
            | Command::Normal(_) => {}
        }
    }

    Ok(geometry)
}
