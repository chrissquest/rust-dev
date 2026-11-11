//! Writing a skinned mesh out as glTF 2.0 binary (`.glb`).
//!
//! Two things need care, and both come from the shape of the decoded data:
//!
//! * **Per-corner texture coordinates.** The command stream gives every polygon *corner* its own UV,
//!   so one vertex can appear in several polygons with a different UV each time. glTF allows one UV
//!   per vertex, so vertices are split: a new vertex is made for each distinct `(vertex, uv)` pair.
//!   Positions get duplicated where the UVs differ, which is what the hardware did as well.
//! * **Winding.** The stream's triangle order is whatever the model author used, and glTF's front
//!   face is counter-clockwise. The mesh's signed volume decides whether the indices and normals need
//!   reversing; the choice is reported so it is never a silent guess.
//!
//! Normals are computed from the geometry, not read from the command stream. The `normal` command is
//! decoded but not applied yet, and the reference ignores it entirely, which is why its exports are
//! flat shaded.

use std::borrow::Cow;
use std::collections::HashMap;
use std::mem;

use anyhow::{Context, Result, bail};
use gltf::json::{self, Index, validation::Checked::Valid, validation::USize64};

use crate::anim;
use crate::gpu::Geometry;
use crate::mesh::Mesh;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Vertices after splitting, ready to become glTF accessors.
#[derive(Default)]
pub struct Split {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// One index list per material group.
    pub groups: Vec<Vec<u32>>,
    /// True when the triangle order had to be reversed to make front faces face outwards.
    pub reversed: bool,
}

/// Area-weighted vertex normals, plus whether the winding had to be reversed.
///
/// The magnitude of a face's cross product is twice its area, so accumulating unnormalised cross
/// products weights large faces more - which is what we want.
fn normals_and_winding(geometry: &Geometry) -> (Vec<[f64; 3]>, bool) {
    let mut normals = vec![[0.0f64; 3]; geometry.positions.len()];
    let mut volume = 0.0f64;

    for group in &geometry.groups {
        for polygon in &group.polygons {
            for index in 1..polygon.len().saturating_sub(1) {
                let corners = [
                    polygon[0].vertex,
                    polygon[index].vertex,
                    polygon[index + 1].vertex,
                ];
                let (p0, p1, p2) = (
                    geometry.positions[corners[0]],
                    geometry.positions[corners[1]],
                    geometry.positions[corners[2]],
                );

                let face = cross(sub(p1, p0), sub(p2, p0));
                for corner in corners {
                    normals[corner] = [
                        normals[corner][0] + face[0],
                        normals[corner][1] + face[1],
                        normals[corner][2] + face[2],
                    ];
                }

                // divergence theorem: the signed volume, positive when the surface faces outwards
                volume += dot(p0, cross(p1, p2)) / 6.0;
            }
        }
    }

    let reversed = volume < 0.0;
    for normal in &mut normals {
        if reversed {
            *normal = [-normal[0], -normal[1], -normal[2]];
        }
        let length = dot(*normal, *normal).sqrt();
        *normal = if length > 1e-12 {
            [normal[0] / length, normal[1] / length, normal[2] / length]
        } else {
            // a vertex no face referenced, or a fully degenerate one
            [0.0, 1.0, 0.0]
        };
    }

    (normals, reversed)
}

/// Splits vertices so each one carries a single UV, and fans the polygons into triangles.
pub fn split(geometry: &Geometry) -> Split {
    let (normals, reversed) = normals_and_winding(geometry);
    let mut split = Split {
        reversed,
        ..Default::default()
    };
    let mut lookup: HashMap<(usize, usize), u32> = HashMap::new();

    for group in &geometry.groups {
        let mut indices = Vec::new();
        for polygon in &group.polygons {
            for index in 1..polygon.len().saturating_sub(1) {
                let mut fan = [&polygon[0], &polygon[index], &polygon[index + 1]];
                if reversed {
                    fan.reverse();
                }

                for corner in fan {
                    // an absent UV is its own split key, and becomes (0, 0)
                    let key = (corner.vertex, corner.uv.unwrap_or(usize::MAX));
                    let vertex = match lookup.get(&key) {
                        Some(existing) => *existing,
                        None => {
                            let position = geometry.positions[corner.vertex];
                            let normal = normals[corner.vertex];
                            let uv = match corner.uv {
                                Some(uv) => geometry.uvs[uv],
                                None => [0.0, 0.0],
                            };
                            let bone = geometry.bones[corner.vertex] as u16;

                            split.positions.push([
                                position[0] as f32,
                                position[1] as f32,
                                position[2] as f32,
                            ]);
                            split
                                .normals
                                .push([normal[0] as f32, normal[1] as f32, normal[2] as f32]);
                            split.uvs.push([uv[0] as f32, uv[1] as f32]);
                            split.joints.push([bone, 0, 0, 0]);
                            split.weights.push([1.0, 0.0, 0.0, 0.0]);

                            let index = (split.positions.len() - 1) as u32;
                            lookup.insert(key, index);
                            index
                        }
                    };
                    indices.push(vertex);
                }
            }
        }
        split.groups.push(indices);
    }

    split
}

/// A byte range inside the binary buffer.
#[derive(Clone, Copy)]
struct Chunk {
    offset: usize,
    length: usize,
}

/// Appends `bytes` to the buffer, 4-byte aligned, and reports where they landed.
fn push_bytes(bin: &mut Vec<u8>, bytes: &[u8]) -> Chunk {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let offset = bin.len();
    bin.extend_from_slice(bytes);
    Chunk {
        offset,
        length: bytes.len(),
    }
}

macro_rules! float_bytes {
    ($name:ident, $shape:ty) => {
        fn $name(values: &[$shape]) -> Vec<u8> {
            let mut out = Vec::with_capacity(values.len() * mem::size_of::<$shape>());
            for value in values {
                for component in value.iter() {
                    out.extend_from_slice(&component.to_le_bytes());
                }
            }
            out
        }
    };
}

float_bytes!(bytes_f32x2, [f32; 2]);
float_bytes!(bytes_f32x3, [f32; 3]);
float_bytes!(bytes_f32x4, [f32; 4]);
float_bytes!(bytes_f32x16, [f32; 16]);

fn bytes_u16x4(values: &[[u16; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 8);
    for value in values {
        for component in value {
            out.extend_from_slice(&component.to_le_bytes());
        }
    }
    out
}

fn bytes_u32(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Encodes RGBA8 pixels as a PNG, which is what glTF embeds.
fn encode_png(image: &ImageSource<'_>) -> Result<Vec<u8>> {
    crate::texture::encode_png(image.width, image.height, image.pixels)
}

/// Creates a buffer view plus an accessor over one byte range.
#[allow(clippy::too_many_arguments)]
fn accessor(
    root: &mut json::Root,
    buffer: Index<json::Buffer>,
    chunk: Chunk,
    count: usize,
    component: json::accessor::ComponentType,
    kind: json::accessor::Type,
    bounds: Option<(json::Value, json::Value)>,
) -> Index<json::Accessor> {
    let view = root.push(json::buffer::View {
        buffer,
        byte_length: USize64::from(chunk.length),
        byte_offset: Some(USize64::from(chunk.offset)),
        byte_stride: None,
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        target: None,
    });

    root.push(json::Accessor {
        buffer_view: Some(view),
        byte_offset: Some(USize64(0)),
        count: USize64::from(count),
        component_type: Valid(json::accessor::GenericComponentType(component)),
        extensions: Default::default(),
        extras: Default::default(),
        type_: Valid(kind),
        min: bounds.as_ref().map(|(min, _)| min.clone()),
        max: bounds.as_ref().map(|(_, max)| max.clone()),
        name: None,
        normalized: false,
        sparse: None,
    })
}

/// Points an accessor's buffer view at the binding target glTF asks for on vertex and index data.
fn set_view_target(
    root: &mut json::Root,
    accessor: Index<json::Accessor>,
    target: json::buffer::Target,
) {
    if let Some(view) = root.accessors[accessor.value()].buffer_view {
        root.buffer_views[view.value()].target = Some(Valid(target));
    }
}

/// The axis-aligned bounds of a vertex list, as JSON numbers for the accessor.
fn position_bounds(positions: &[[f32; 3]]) -> (json::Value, json::Value) {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    (
        json::Value::from(Vec::from(min)),
        json::Value::from(Vec::from(max)),
    )
}

/// One clip to embed: the name to give the glTF animation, and its decoded keyframes.
pub struct Clip<'a> {
    pub name: &'a str,
    pub animation: &'a anim::Animation,
}

/// One image to embed, already decoded to RGBA8.
pub struct ImageSource<'a> {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row by row.
    pub pixels: &'a [u8],
    /// Whether alpha should be honoured. A DS texture marks its colour-keyed background transparent,
    /// and glTF would draw it as a solid colour unless the material masks it out.
    pub alpha_mask: bool,
}

/// One material to write, in the same order as `geometry.groups`.
pub struct MaterialSource<'a> {
    /// The material's name, e.g. `din030_a`.
    pub name: &'a str,
    /// The base colour texture, if the group's palette base named an image.
    pub image: Option<ImageSource<'a>>,
}

/// Where a clip's sample data landed in the binary buffer.
struct ClipChunks {
    times: Chunk,
    frames: usize,
    /// Per bone: translation, rotation and scale sample ranges.
    bones: Vec<(Chunk, Chunk, Chunk)>,
}

fn bytes_f32(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn vec3(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

fn vec4(v: [f64; 4]) -> [f32; 4] {
    [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]
}

/// Builds a `.glb` holding the creature's bind-pose mesh, its skeleton, and any clips given.
///
/// Returns the file bytes and the split that produced them, so the caller can report what was
/// written without doing the work twice.
pub fn build(
    decoded: &Mesh,
    geometry: &Geometry,
    name: &str,
    clips: &[Clip<'_>],
    material_sources: &[MaterialSource<'_>],
) -> Result<(Vec<u8>, Split)> {
    let split = split(geometry);

    if decoded.bones.is_empty() {
        bail!("the mesh has no bones, so there is no skeleton to write");
    }
    if split.positions.is_empty() || split.groups.iter().all(|group| group.is_empty()) {
        bail!("nothing to export: the geometry has no triangles");
    }

    let mut inverse_bind: Vec<[f32; 16]> = Vec::with_capacity(decoded.bones.len());
    for bone in &decoded.bones {
        let inverse = bone
            .matrix
            .inverse()
            .ok_or_else(|| anyhow::anyhow!("bone `{}` has a degenerate bind matrix", bone.name))?;
        inverse_bind.push(inverse.to_gltf_matrix());
    }

    // one buffer, one view per accessor
    let mut bin = Vec::new();
    let positions_chunk = push_bytes(&mut bin, &bytes_f32x3(&split.positions));
    let normals_chunk = push_bytes(&mut bin, &bytes_f32x3(&split.normals));
    let uvs_chunk = push_bytes(&mut bin, &bytes_f32x2(&split.uvs));
    let joints_chunk = push_bytes(&mut bin, &bytes_u16x4(&split.joints));
    let weights_chunk = push_bytes(&mut bin, &bytes_f32x4(&split.weights));
    let inverse_bind_chunk = push_bytes(&mut bin, &bytes_f32x16(&inverse_bind));
    let index_chunks: Vec<Chunk> = split
        .groups
        .iter()
        .map(|indices| push_bytes(&mut bin, &bytes_u32(indices)))
        .collect();

    // Per clip: one shared time array, then translation/rotation/scale samples per bone. Every
    // frame's matrix is decomposed, because glTF animates TRS and has no matrix channels.
    let mut clip_chunks: Vec<ClipChunks> = Vec::with_capacity(clips.len());
    for clip in clips {
        if clip.animation.bone_count != decoded.bones.len() {
            bail!(
                "clip `{}` animates {} bones but the mesh has {}",
                clip.name,
                clip.animation.bone_count,
                decoded.bones.len()
            );
        }

        let frames = clip.animation.frame_count;
        let times_chunk = push_bytes(&mut bin, &bytes_f32(&clip.animation.times()));

        let mut bones = Vec::with_capacity(clip.animation.bone_count);
        for bone in 0..clip.animation.bone_count {
            let mut translations = Vec::with_capacity(frames);
            let mut rotations = Vec::with_capacity(frames);
            let mut scales = Vec::with_capacity(frames);

            for frame in 0..frames {
                let matrix = clip
                    .animation
                    .transform(bone, frame)
                    .expect("the frame is within range");
                let pose = matrix.decompose();

                translations.push([
                    pose.translation[0] as f32,
                    pose.translation[1] as f32,
                    pose.translation[2] as f32,
                ]);
                rotations.push([
                    pose.rotation[0] as f32,
                    pose.rotation[1] as f32,
                    pose.rotation[2] as f32,
                    pose.rotation[3] as f32,
                ]);
                scales.push([
                    pose.scale[0] as f32,
                    pose.scale[1] as f32,
                    pose.scale[2] as f32,
                ]);
            }

            bones.push((
                push_bytes(&mut bin, &bytes_f32x3(&translations)),
                push_bytes(&mut bin, &bytes_f32x4(&rotations)),
                push_bytes(&mut bin, &bytes_f32x3(&scales)),
            ));
        }

        clip_chunks.push(ClipChunks {
            times: times_chunk,
            frames,
            bones,
        });
    }

    // Textures share the buffer with the geometry, so they have to be placed before the buffer is
    // described.
    let mut image_chunks: Vec<Option<Chunk>> = Vec::with_capacity(material_sources.len());
    for source in material_sources {
        match source.image.as_ref() {
            Some(image) => {
                let png = encode_png(image).with_context(|| format!("texture `{}`", source.name))?;
                image_chunks.push(Some(push_bytes(&mut bin, &png)));
            }
            None => image_chunks.push(None),
        }
    }

    while bin.len() % 4 != 0 {
        bin.push(0);
    }

    let mut root = json::Root::default();
    root.asset = json::Asset {
        generator: Some("ff-export".to_string()),
        version: "2.0".to_string(),
        ..Default::default()
    };

    let buffer = root.push(json::Buffer {
        byte_length: USize64::from(bin.len()),
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        uri: None,
    });

    let count = split.positions.len();
    let positions = accessor(
        &mut root,
        buffer,
        positions_chunk,
        count,
        json::accessor::ComponentType::F32,
        json::accessor::Type::Vec3,
        Some(position_bounds(&split.positions)),
    );
    let normals = accessor(
        &mut root,
        buffer,
        normals_chunk,
        count,
        json::accessor::ComponentType::F32,
        json::accessor::Type::Vec3,
        None,
    );
    let uvs = accessor(
        &mut root,
        buffer,
        uvs_chunk,
        count,
        json::accessor::ComponentType::F32,
        json::accessor::Type::Vec2,
        None,
    );
    let joints = accessor(
        &mut root,
        buffer,
        joints_chunk,
        count,
        json::accessor::ComponentType::U16,
        json::accessor::Type::Vec4,
        None,
    );
    let weights = accessor(
        &mut root,
        buffer,
        weights_chunk,
        count,
        json::accessor::ComponentType::F32,
        json::accessor::Type::Vec4,
        None,
    );
    let inverse_bind_accessor = accessor(
        &mut root,
        buffer,
        inverse_bind_chunk,
        decoded.bones.len(),
        json::accessor::ComponentType::F32,
        json::accessor::Type::Mat4,
        None,
    );
    // --- primitives, nodes, skin and scene ---

    // One material per group. A group whose palette base named an image gets that image as its base
    // colour; a group that named nothing visible keeps a placeholder so the geometry stays complete.
    let mut sampler: Option<Index<json::texture::Sampler>> = None;
    let mut materials: Vec<Index<json::Material>> = Vec::with_capacity(geometry.groups.len());
    for (index, group) in geometry.groups.iter().enumerate() {
        let source = material_sources.get(index);
        let name = match source {
            Some(source) => source.name.to_string(),
            None => match group.palette_base {
                Some(value) => format!("material_{value}"),
                None => "material_none".to_string(),
            },
        };

        let mut base_color_texture = None;
        let mut alpha_mask = false;
        if let Some(chunk) = image_chunks.get(index).copied().flatten() {
            alpha_mask = source
                .and_then(|source| source.image.as_ref())
                .is_some_and(|image| image.alpha_mask);

            let view = root.push(json::buffer::View {
                buffer,
                byte_length: USize64::from(chunk.length),
                byte_offset: Some(USize64::from(chunk.offset)),
                byte_stride: None,
                extensions: Default::default(),
                extras: Default::default(),
                name: None,
                // an image is not vertex data, so it declares no target
                target: None,
            });
            let image = root.push(json::Image {
                buffer_view: Some(view),
                mime_type: Some(json::image::MimeType("image/png".to_string())),
                uri: None,
                name: Some(name.clone()),
                extensions: Default::default(),
                extras: Default::default(),
            });
            let sampler = match sampler {
                Some(existing) => existing,
                None => {
                    let created = root.push(json::texture::Sampler {
                        // The DS GPU has no bilinear filtering: both its mag and min filters are
                        // point samples, so nearest is the faithful choice, and the art is drawn to
                        // rely on it.
                        mag_filter: Some(Valid(json::texture::MagFilter::Nearest)),
                        min_filter: Some(Valid(json::texture::MinFilter::Nearest)),
                        wrap_s: Valid(json::texture::WrappingMode::Repeat),
                        wrap_t: Valid(json::texture::WrappingMode::Repeat),
                        name: Some("texture_sampler".to_string()),
                        extensions: Default::default(),
                        extras: Default::default(),
                    });
                    sampler = Some(created);
                    created
                }
            };
            let texture = root.push(json::Texture {
                sampler: Some(sampler),
                source: image,
                name: Some(name.clone()),
                extensions: Default::default(),
                extras: Default::default(),
            });
            base_color_texture = Some(json::texture::Info {
                index: texture,
                tex_coord: 0,
                extensions: Default::default(),
                extras: Default::default(),
            });
        }

        materials.push(root.push(json::Material {
            name: Some(name),
            alpha_cutoff: alpha_mask.then_some(json::material::AlphaCutoff(0.5)),
            alpha_mode: Valid(if alpha_mask {
                json::material::AlphaMode::Mask
            } else {
                json::material::AlphaMode::Opaque
            }),
            pbr_metallic_roughness: json::material::PbrMetallicRoughness {
                base_color_factor: json::material::PbrBaseColorFactor([1.0, 1.0, 1.0, 1.0]),
                base_color_texture,
                metallic_factor: json::material::StrengthFactor(0.0),
                roughness_factor: json::material::StrengthFactor(1.0),
                ..Default::default()
            },
            // the winding is corrected for the whole mesh, so faces are single sided
            double_sided: false,
            ..Default::default()
        }));
    }

    let mut primitives = Vec::new();
    for (index, chunk) in index_chunks.iter().enumerate() {
        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert(Valid(json::mesh::Semantic::Positions), positions);
        attributes.insert(Valid(json::mesh::Semantic::Normals), normals);
        attributes.insert(Valid(json::mesh::Semantic::TexCoords(0)), uvs);
        attributes.insert(Valid(json::mesh::Semantic::Joints(0)), joints);
        attributes.insert(Valid(json::mesh::Semantic::Weights(0)), weights);

        primitives.push(json::mesh::Primitive {
            attributes,
            indices: Some(accessor(
                &mut root,
                buffer,
                *chunk,
                split.groups[index].len(),
                json::accessor::ComponentType::U32,
                json::accessor::Type::Scalar,
                None,
            )),
            material: Some(materials[index]),
            mode: Valid(json::mesh::Mode::Triangles),
            targets: None,
            extensions: Default::default(),
            extras: Default::default(),
        });
    }

    let mesh = root.push(json::Mesh {
        extensions: Default::default(),
        extras: Default::default(),
        name: Some(format!("{name}_mesh")),
        primitives,
        weights: None,
    });

    // glTF wants a target on the buffer views used for vertex and index data.
    let mesh_index = mesh.value();
    let mut vertex_accessors: Vec<Index<json::Accessor>> = Vec::new();
    let mut index_accessors: Vec<Index<json::Accessor>> = Vec::new();
    for primitive in &root.meshes[mesh_index].primitives {
        vertex_accessors.extend(primitive.attributes.values().copied());
        if let Some(indices) = primitive.indices {
            index_accessors.push(indices);
        }
    }
    for accessor in vertex_accessors {
        set_view_target(&mut root, accessor, json::buffer::Target::ArrayBuffer);
    }
    for accessor in index_accessors {
        set_view_target(&mut root, accessor, json::buffer::Target::ElementArrayBuffer);
    }

    // A single node above the joints so that they have a common root, which glTF requires of a skin.
    let skeleton_root = root.push(json::Node {
        name: Some(format!("{name}_skeleton")),
        ..Default::default()
    });

    // The skeleton is flat - every joint is a direct child of that root - which is how the reference
    // models it too. The bind pose is written as TRS rather than as a matrix, because glTF forbids
    // animating a node that carries a matrix, and these are exactly the values the channels write.
    let joint_nodes: Vec<Index<json::Node>> = decoded
        .bones
        .iter()
        .map(|bone| {
            let pose = bone.matrix.decompose();
            root.push(json::Node {
                name: Some(bone.name.clone()),
                translation: Some(vec3(pose.translation)),
                rotation: Some(json::scene::UnitQuaternion(vec4(pose.rotation))),
                scale: Some(vec3(pose.scale)),
                ..Default::default()
            })
        })
        .collect();

    root.nodes[skeleton_root.value()].children = Some(joint_nodes.clone());

    // One glTF animation per clip: a sampler and a channel for each of a bone's three paths. Every
    // sampler in a clip shares the same time accessor.
    for (clip, chunks) in clips.iter().zip(&clip_chunks) {
        let last_time = if chunks.frames > 1 {
            (chunks.frames - 1) as f32 / 60.0
        } else {
            0.0
        };
        let input = accessor(
            &mut root,
            buffer,
            chunks.times,
            chunks.frames,
            json::accessor::ComponentType::F32,
            json::accessor::Type::Scalar,
            // a SCALAR accessor's bounds have to be one-element arrays
            Some((
                json::Value::from(vec![0.0f32]),
                json::Value::from(vec![last_time]),
            )),
        );

        let mut samplers = Vec::new();
        let mut channels = Vec::new();

        for (bone, (translation, rotation, scale)) in chunks.bones.iter().enumerate() {
            let node = joint_nodes[bone];
            let paths = [
                (
                    json::animation::Property::Translation,
                    *translation,
                    json::accessor::Type::Vec3,
                ),
                (
                    json::animation::Property::Rotation,
                    *rotation,
                    json::accessor::Type::Vec4,
                ),
                (
                    json::animation::Property::Scale,
                    *scale,
                    json::accessor::Type::Vec3,
                ),
            ];

            for (property, chunk, kind) in paths {
                let output = accessor(
                    &mut root,
                    buffer,
                    chunk,
                    chunks.frames,
                    json::accessor::ComponentType::F32,
                    kind,
                    None,
                );

                samplers.push(json::animation::Sampler {
                    extensions: Default::default(),
                    extras: Default::default(),
                    input,
                    interpolation: Valid(json::animation::Interpolation::Linear),
                    output,
                });

                channels.push(json::animation::Channel {
                    extensions: Default::default(),
                    extras: Default::default(),
                    sampler: Index::new((samplers.len() - 1) as u32),
                    target: json::animation::Target {
                        extensions: Default::default(),
                        extras: Default::default(),
                        node,
                        path: Valid(property),
                    },
                });
            }
        }

        root.push(json::Animation {
            channels,
            extensions: Default::default(),
            extras: Default::default(),
            name: Some(clip.name.to_string()),
            samplers,
        });
    }

    let skin = root.push(json::Skin {
        inverse_bind_matrices: Some(inverse_bind_accessor),
        joints: joint_nodes.clone(),
        skeleton: Some(skeleton_root),
        name: Some(format!("{name}_skeleton")),
        extensions: Default::default(),
        extras: Default::default(),
    });

    let mesh_node = root.push(json::Node {
        mesh: Some(mesh),
        skin: Some(skin),
        name: Some(name.to_string()),
        ..Default::default()
    });

    // the skeleton root carries the joints, so the scene needs only the mesh and that root
    let scene_nodes = vec![mesh_node, skeleton_root];
    let scene = root.push(json::Scene {
        nodes: scene_nodes,
        name: Some("scene".to_string()),
        extensions: Default::default(),
        extras: Default::default(),
    });
    root.scene = Some(scene);

    let json_text = json::serialize::to_string(&root)?;
    let glb = gltf::binary::Glb {
        // `to_writer` recomputes the length and handles the chunk padding itself
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: Cow::Owned(json_text.into_bytes()),
        bin: Some(Cow::Owned(bin)),
    };

    Ok((glb.to_vec()?, split))
}
