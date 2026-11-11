//! glTF 2.0 -> [`Scene`].
//!
//! Everything in this module is CPU-only and GPU-free on purpose: `--info`
//! must work on a machine with no display, and the loader is the piece most
//! worth testing on its own.

use std::path::Path;

use anyhow::{Context, Result};
use gltf::animation::Interpolation as GltfInterpolation;
use gltf::animation::util::ReadOutputs;
use gltf::mesh::Mode;

use crate::anim::{Animation, Channel, Interpolation, Values};
use crate::math::{Aabb, Mat4, Quat, Vec3};
use crate::scene::{
    Material, Mesh, Node, Primitive, Scene, Skin, Texture, TextureFilter, Transform,
};

pub fn load(path: &Path) -> Result<Scene> {
    let (document, buffers, images) =
        gltf::import(path).with_context(|| format!("could not import {}", path.display()))?;

    let textures = load_textures(&document, &images);
    let materials = load_materials(&document);
    let meshes = load_meshes(&document, &buffers);
    let skins = load_skins(&document, &buffers);
    let (nodes, roots) = load_nodes(&document);
    let animations = load_animations(&document, &buffers);

    let mut scene = Scene {
        nodes,
        roots,
        meshes,
        materials,
        textures,
        skins,
        animations,
        bounds: Aabb::empty(),
        source: path.display().to_string(),
    };
    scene.recompute_bounds();
    Ok(scene)
}

fn load_textures(document: &gltf::Document, images: &[gltf::image::Data]) -> Vec<Texture> {
    let names: Vec<String> = document
        .images()
        .map(|image| image.name().unwrap_or_default().to_string())
        .collect();

    // A glTF image can be referenced by several textures with different
    // samplers; keeping the first one's filtering is close enough, and the
    // alternative is duplicating the upload.
    let mut filters = vec![TextureFilter::Linear; images.len()];
    for texture in document.textures() {
        let image = texture.source().index();
        if let Some(slot) = filters.get_mut(image) {
            *slot = match texture.sampler().mag_filter() {
                Some(gltf::texture::MagFilter::Nearest) => TextureFilter::Nearest,
                _ => TextureFilter::Linear,
            };
        }
    }

    images
        .iter()
        .enumerate()
        .map(|(index, image)| Texture {
            name: names.get(index).cloned().unwrap_or_default(),
            width: image.width,
            height: image.height,
            rgba: to_rgba8(image),
            filter: filters[index],
        })
        .collect()
}

/// Expands whatever channel layout the decoder produced into tight RGBA8.
fn to_rgba8(image: &gltf::image::Data) -> Vec<u8> {
    use gltf::image::Format;

    let pixels = &image.pixels;
    match image.format {
        Format::R8G8B8A8 => pixels.clone(),
        Format::R8G8B8 => pixels
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        Format::R8G8 => pixels
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[1], 0, 255])
            .collect(),
        Format::R8 => pixels
            .iter()
            .flat_map(|&value| [value, value, value, 255])
            .collect(),
        // 16-bit and float images are rare in game assets and would need a
        // tone-mapping decision; render them white instead of guessing.
        other => {
            log::warn!(
                "image {}x{} uses {other:?}, which this viewer does not convert; using white",
                image.width,
                image.height
            );
            vec![255; 4]
        }
    }
}

fn load_materials(document: &gltf::Document) -> Vec<Material> {
    document
        .materials()
        .map(|material| {
            let pbr = material.pbr_metallic_roughness();
            Material {
                name: material.name().unwrap_or_default().to_string(),
                base_color: pbr.base_color_factor(),
                base_color_texture: pbr
                    .base_color_texture()
                    .map(|info| info.texture().source().index()),
                alpha_mask: matches!(material.alpha_mode(), gltf::material::AlphaMode::Mask),
            }
        })
        .collect()
}

fn load_meshes(document: &gltf::Document, buffers: &[gltf::buffer::Data]) -> Vec<Mesh> {
    document
        .meshes()
        .map(|mesh| Mesh {
            name: mesh.name().unwrap_or_default().to_string(),
            primitives: mesh
                .primitives()
                .filter_map(|primitive| match load_primitive(&primitive, buffers) {
                    Ok(loaded) => Some(loaded),
                    Err(error) => {
                        log::warn!(
                            "mesh {:?}: skipping primitive: {error:#}",
                            mesh.name().unwrap_or("<unnamed>")
                        );
                        None
                    }
                })
                .collect(),
        })
        .collect()
}

fn load_primitive(
    primitive: &gltf::Primitive<'_>,
    buffers: &[gltf::buffer::Data],
) -> Result<Primitive> {
    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()].0[..]));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .context("primitive has no POSITION attribute")?
        .collect();
    if positions.is_empty() {
        anyhow::bail!("primitive has no vertices");
    }

    let mut indices: Vec<u32> = match reader.read_indices() {
        Some(indices) => indices.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };

    indices = match primitive.mode() {
        Mode::Triangles => indices,
        Mode::TriangleStrip => strip_to_triangles(&indices),
        Mode::TriangleFan => fan_to_triangles(&indices),
        Mode::Points | Mode::Lines | Mode::LineStrip | Mode::LineLoop => {
            anyhow::bail!("only triangle primitives are drawn");
        }
    };

    let vertex_count = positions.len();
    if indices.len() < 3 {
        anyhow::bail!("primitive has fewer than three indices");
    }
    if let Some(bad) = indices
        .iter()
        .find(|&&index| index as usize >= vertex_count)
    {
        anyhow::bail!("index {bad} is outside the {vertex_count} vertices");
    }

    let mut normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|normals| normals.collect())
        .unwrap_or_default();
    if normals.len() != vertex_count {
        normals = smooth_normals(&positions, &indices);
    }

    let mut uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|uvs| uvs.into_f32().collect())
        .unwrap_or_default();
    uvs.resize(vertex_count, [0.0, 0.0]);

    let mut joints: Vec<[f32; 4]> = reader
        .read_joints(0)
        .map(|joints| {
            joints
                .into_u16()
                .map(|joint| {
                    [
                        joint[0] as f32,
                        joint[1] as f32,
                        joint[2] as f32,
                        joint[3] as f32,
                    ]
                })
                .collect()
        })
        .unwrap_or_default();

    let mut weights: Vec<[f32; 4]> = reader
        .read_weights(0)
        .map(|weights| weights.into_f32().collect())
        .unwrap_or_default();

    normalise_skinning(vertex_count, &mut joints, &mut weights);

    Ok(Primitive {
        positions,
        normals,
        uvs,
        joints,
        weights,
        indices,
        material: primitive.material().index(),
    })
}

/// Makes joints/weights the same length as the vertex list and picks a
/// fallback for vertices the exporter left unweighted. Weight 1 on joint 0 is
/// identity for unskinned draws, because the renderer binds an identity matrix
/// there.
fn normalise_skinning(
    vertex_count: usize,
    joints: &mut Vec<[f32; 4]>,
    weights: &mut Vec<[f32; 4]>,
) {
    joints.resize(vertex_count, [0.0; 4]);
    weights.resize(vertex_count, [0.0; 4]);

    for (joint, weight) in joints.iter_mut().zip(weights.iter_mut()) {
        let sum = weight[0] + weight[1] + weight[2] + weight[3];
        if sum > 1e-6 {
            for value in weight.iter_mut() {
                *value /= sum;
            }
        } else {
            *joint = [0.0; 4];
            *weight = [1.0, 0.0, 0.0, 0.0];
        }
    }
}

/// Area-weighted vertex normals, for primitives that ship without them.
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![Vec3::ZERO; positions.len()];

    for triangle in indices.chunks_exact(3) {
        let a = Vec3::from_array(positions[triangle[0] as usize]);
        let b = Vec3::from_array(positions[triangle[1] as usize]);
        let c = Vec3::from_array(positions[triangle[2] as usize]);
        let face = (b - a).cross(c - a);

        for &index in triangle {
            normals[index as usize] += face;
        }
    }

    normals
        .iter()
        .map(|normal| normal.normalize_or_zero().to_array())
        .collect()
}

fn strip_to_triangles(indices: &[u32]) -> Vec<u32> {
    if indices.len() < 3 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity((indices.len() - 2) * 3);
    for i in 0..indices.len() - 2 {
        // Winding alternates with parity, per the glTF spec.
        if i % 2 == 0 {
            out.extend_from_slice(&[indices[i], indices[i + 1], indices[i + 2]]);
        } else {
            out.extend_from_slice(&[indices[i + 1], indices[i], indices[i + 2]]);
        }
    }
    out
}

fn fan_to_triangles(indices: &[u32]) -> Vec<u32> {
    if indices.len() < 3 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity((indices.len() - 2) * 3);
    for i in 1..indices.len() - 1 {
        out.extend_from_slice(&[indices[0], indices[i], indices[i + 1]]);
    }
    out
}

fn load_skins(document: &gltf::Document, buffers: &[gltf::buffer::Data]) -> Vec<Skin> {
    document
        .skins()
        .map(|skin| {
            let joints: Vec<usize> = skin.joints().map(|joint| joint.index()).collect();
            let inverse_bind = skin
                .reader(|buffer| Some(&buffers[buffer.index()].0[..]))
                .read_inverse_bind_matrices()
                .map(|matrices| matrices.map(|m| Mat4::from_cols_array_2d(&m)).collect::<Vec<_>>())
                .unwrap_or_else(|| vec![Mat4::IDENTITY; joints.len()]);

            Skin {
                joints,
                inverse_bind,
            }
        })
        .collect()
}

fn load_nodes(document: &gltf::Document) -> (Vec<Node>, Vec<usize>) {
    let mut nodes: Vec<Node> = document
        .nodes()
        .map(|node| {
            let (translation, rotation, scale) = node.transform().decomposed();
            Node {
                name: node.name().unwrap_or_default().to_string(),
                parent: None,
                children: node.children().map(|child| child.index()).collect(),
                local: Transform {
                    translation: Vec3::from_array(translation),
                    rotation: Quat::from_array(rotation),
                    scale: Vec3::from_array(scale),
                },
                mesh: node.mesh().map(|mesh| mesh.index()),
                skin: node.skin().map(|skin| skin.index()),
            }
        })
        .collect();

    // Second pass: children carry the parent link, the JSON only stores it one
    // way round.
    for index in 0..nodes.len() {
        for child in nodes[index].children.clone() {
            if let Some(node) = nodes.get_mut(child) {
                node.parent = Some(index);
            }
        }
    }

    let roots: Vec<usize> = match document
        .default_scene()
        .or_else(|| document.scenes().next())
    {
        Some(scene) => scene.nodes().map(|node| node.index()).collect(),
        None => nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.parent.is_none())
            .map(|(index, _)| index)
            .collect(),
    };

    (nodes, roots)
}

fn load_animations(document: &gltf::Document, buffers: &[gltf::buffer::Data]) -> Vec<Animation> {
    document
        .animations()
        .enumerate()
        .map(|(clip_index, animation)| {
            let mut channels = Vec::new();
            let mut duration: f32 = 0.0;

            for channel in animation.channels() {
                let node = channel.target().node().index();
                let sample = channel.sampler();
                let cubic = matches!(sample.interpolation(), GltfInterpolation::CubicSpline);
                let interpolation = match sample.interpolation() {
                    GltfInterpolation::Step => Interpolation::Step,
                    // Cubic splines are approximated by their key values: the
                    // tangents are dropped rather than spending a page on
                    // Hermite evaluation for a viewer.
                    GltfInterpolation::Linear | GltfInterpolation::CubicSpline => {
                        Interpolation::Linear
                    }
                };

                let reader = channel.reader(|buffer| Some(&buffers[buffer.index()].0[..]));

                let Some(times) = reader
                    .read_inputs()
                    .map(|times| times.collect::<Vec<f32>>())
                else {
                    continue;
                };
                if times.is_empty() {
                    continue;
                }

                let Some(outputs) = reader.read_outputs() else {
                    continue;
                };

                let values = match outputs {
                    ReadOutputs::Translations(values) => {
                        Values::Translation(finish_vec3(values.collect(), cubic))
                    }
                    ReadOutputs::Scales(values) => {
                        Values::Scale(finish_vec3(values.collect(), cubic))
                    }
                    ReadOutputs::Rotations(rotations) => {
                        let mut values: Vec<[f32; 4]> = rotations.into_f32().collect::<Vec<_>>();
                        if cubic {
                            values = decimate_cubic(values);
                        }
                        Values::Rotation(values.into_iter().map(Quat::from_array).collect())
                    }
                    ReadOutputs::MorphTargetWeights(_) => continue,
                };

                if let Some(&last) = times.last() {
                    duration = duration.max(last);
                }

                channels.push(Channel {
                    node,
                    interpolation,
                    times,
                    values,
                });
            }

            Animation {
                name: animation
                    .name()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("clip {clip_index}")),
                channels,
                duration,
            }
        })
        .collect()
}

fn finish_vec3(values: Vec<[f32; 3]>, cubic: bool) -> Vec<Vec3> {
    let values = if cubic {
        decimate_cubic(values)
    } else {
        values
    };
    values.into_iter().map(Vec3::from_array).collect()
}

/// Cubic-spline output stores three values per key (in-tangent, value,
/// out-tangent); keep the middle one.
fn decimate_cubic<T>(values: Vec<T>) -> Vec<T> {
    values.into_iter().skip(1).step_by(3).collect()
}
