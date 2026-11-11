//! The viewer's in-memory scene: plain data, no GPU types and no glTF types.
//!
//! This is the seam that keeps the loader, the animation sampler and the
//! renderer independent of each other. Whatever eventually feeds it - glTF
//! today, `ff-export`'s format readers later - produces exactly these structs.

use crate::math::{Aabb, Mat4, Quat, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

impl Transform {
    pub fn to_matrix(self) -> Mat4 {
        Mat4::from_trs(self.translation, self.rotation, self.scale)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub local: Transform,
    /// Index into [`Scene::meshes`].
    pub mesh: Option<usize>,
    /// Index into [`Scene::skins`].
    pub skin: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFilter {
    Linear,
    Nearest,
}

#[derive(Clone, Debug)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, one row per scanline.
    pub rgba: Vec<u8>,
    pub filter: TextureFilter,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    /// Index into [`Scene::textures`].
    pub base_color_texture: Option<usize>,
    /// glTF `alphaMode: MASK`: discard fragments below half alpha.
    pub alpha_mask: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: String::from("default"),
            base_color: [1.0, 1.0, 1.0, 1.0],
            base_color_texture: None,
            alpha_mask: false,
        }
    }
}

/// One draw call's worth of geometry, in bind-pose mesh space.
#[derive(Clone, Debug, Default)]
pub struct Primitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Up to four joints per vertex, as indices into the owning skin.
    pub joints: Vec<[f32; 4]>,
    /// Weights matching `joints`, normalised to sum to one.
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Index into [`Scene::materials`]; always `Some` once loading is done.
    pub material: Option<usize>,
}

impl Primitive {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub name: String,
    pub primitives: Vec<Primitive>,
}

#[derive(Clone, Debug)]
pub struct Skin {
    /// Node indices, in the order the inverse bind matrices are given.
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub nodes: Vec<Node>,
    pub roots: Vec<usize>,
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub textures: Vec<Texture>,
    pub skins: Vec<Skin>,
    pub animations: Vec<crate::anim::Animation>,
    pub bounds: Aabb,
    /// Where it came from, for the window title.
    pub source: String,
}

impl Scene {
    pub fn vertex_count(&self) -> usize {
        self.meshes
            .iter()
            .flat_map(|mesh| &mesh.primitives)
            .map(Primitive::vertex_count)
            .sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.meshes
            .iter()
            .flat_map(|mesh| &mesh.primitives)
            .map(Primitive::triangle_count)
            .sum()
    }

    pub fn primitive_count(&self) -> usize {
        self.meshes.iter().map(|mesh| mesh.primitives.len()).sum()
    }

    /// The rest pose: every node at its authored local transform.
    pub fn default_pose(&self) -> Vec<Transform> {
        self.nodes.iter().map(|node| node.local).collect()
    }

    /// World matrix per node, walking down from the scene roots.
    ///
    /// Nodes nobody owns are also treated as roots. Some exporters (Blender's
    /// among them) emit a skin's joints as free-standing nodes that appear in
    /// no scene graph, and referencing them only from `skin.joints`; without
    /// this they would all sit at the identity and every skinned mesh would
    /// collapse.
    pub fn world_transforms(&self, pose: &[Transform]) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.nodes.len()];
        let mut visited = vec![false; self.nodes.len()];

        let roots = self.roots.iter().copied().chain(
            self.nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| node.parent.is_none())
                .map(|(index, _)| index),
        );

        for root in roots {
            let mut stack = vec![(root, Mat4::IDENTITY)];
            while let Some((index, parent)) = stack.pop() {
                if index >= self.nodes.len() || visited[index] {
                    continue;
                }
                visited[index] = true;

                let node = &self.nodes[index];
                let local = pose.get(index).copied().unwrap_or(node.local).to_matrix();
                let matrix = parent * local;
                world[index] = matrix;

                for &child in &node.children {
                    stack.push((child, matrix));
                }
            }
        }

        world
    }

    /// Per-joint skinning matrices for one mesh instance.
    ///
    /// glTF defines these in the mesh node's space, so the mesh's own world
    /// transform has to be divided out before the renderer applies it again:
    /// `inverse(mesh_world) * joint_world * inverse_bind`.
    pub fn joint_matrices(&self, world: &[Mat4], skin: usize, mesh_node: usize) -> Vec<Mat4> {
        let Some(skin) = self.skins.get(skin) else {
            return Vec::new();
        };
        let inverse_mesh = world
            .get(mesh_node)
            .copied()
            .unwrap_or(Mat4::IDENTITY)
            .inverse();

        skin.joints
            .iter()
            .zip(&skin.inverse_bind)
            .map(|(&joint, inverse_bind)| {
                let joint_world = world.get(joint).copied().unwrap_or(Mat4::IDENTITY);
                inverse_mesh * joint_world * *inverse_bind
            })
            .collect()
    }

    /// Recomputes `bounds` from the rest pose. Called once after loading.
    pub fn recompute_bounds(&mut self) {
        let world = self.world_transforms(&self.default_pose());
        self.bounds = self.bounds_for(&world);
    }

    /// The extents of every mesh vertex with skinning applied, for a given set of node
    /// world transforms.
    ///
    /// This has to match what the shader does. A skinned vertex is placed by its joint
    /// matrices, not by the node that references the mesh - and those two spaces can
    /// differ by a scale factor, which is exactly how a camera ends up framing a box
    /// several times smaller than the model on screen.
    pub fn bounds_for(&self, world: &[Mat4]) -> Aabb {
        let mut bounds = Aabb::empty();

        for (index, node) in self.nodes.iter().enumerate() {
            let Some(mesh_index) = node.mesh else {
                continue;
            };
            let Some(mesh) = self.meshes.get(mesh_index) else {
                continue;
            };

            let joints = node
                .skin
                .map(|skin| self.joint_matrices(world, skin, index))
                .unwrap_or_default();
            let model = world.get(index).copied().unwrap_or(Mat4::IDENTITY);

            for primitive in &mesh.primitives {
                for vertex in 0..primitive.vertex_count() {
                    let position = Vec3::from_array(primitive.positions[vertex]);
                    let point = if joints.is_empty() {
                        model.transform_point(position)
                    } else {
                        skinned_position(&joints, primitive, vertex, position)
                    };
                    bounds.expand(point);
                }
            }
        }

        bounds
    }

    /// Everything the `--info` report needs, as label/value rows.
    pub fn summary(&self) -> Vec<(String, String)> {
        let bounds = if self.bounds.is_empty() {
            String::from("empty")
        } else {
            let size = self.bounds.size();
            let center = self.bounds.center();
            format!(
                "size {:.2} x {:.2} x {:.2}, center ({:.2}, {:.2}, {:.2})",
                size.x, size.y, size.z, center.x, center.y, center.z
            )
        };

        vec![
            (String::from("source"), self.source.clone()),
            (String::from("nodes"), self.nodes.len().to_string()),
            (
                String::from("roots"),
                listing(
                    self.roots.len(),
                    self.roots
                        .iter()
                        .filter_map(|&index| self.nodes.get(index))
                        .map(|node| node.name.as_str()),
                ),
            ),
            (
                String::from("meshes"),
                listing(
                    self.meshes.len(),
                    self.meshes.iter().map(|mesh| mesh.name.as_str()),
                ),
            ),
            (
                String::from("primitives"),
                self.primitive_count().to_string(),
            ),
            (String::from("vertices"), self.vertex_count().to_string()),
            (String::from("triangles"), self.triangle_count().to_string()),
            (
                String::from("materials"),
                listing(
                    self.materials.len(),
                    self.materials.iter().map(|material| material.name.as_str()),
                ),
            ),
            (String::from("textures"), self.textures.len().to_string()),
            (
                String::from("skins"),
                self.skins
                    .iter()
                    .map(|skin| format!("{} joints", skin.joints.len()))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            (
                String::from("animations"),
                self.animations.len().to_string(),
            ),
            (
                String::from("clips"),
                self.animations
                    .iter()
                    .map(|animation| format!("{} ({:.2}s)", animation.name, animation.duration))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            (String::from("bounds"), bounds),
        ]
    }
}

/// "3 - body, eyes" or just "0" when nothing is named.
fn listing<'a>(total: usize, names: impl Iterator<Item = &'a str>) -> String {
    let names: Vec<&str> = names.filter(|name| !name.is_empty()).take(6).collect();
    if names.is_empty() {
        total.to_string()
    } else {
        format!("{total} - {}", names.join(", "))
    }
}

/// The same four-joint weighted sum the vertex shader performs.
fn skinned_position(
    joints: &[Mat4],
    primitive: &Primitive,
    vertex: usize,
    position: Vec3,
) -> Vec3 {
    let indices = primitive.joints.get(vertex).copied().unwrap_or([0.0; 4]);
    let weights = primitive
        .weights
        .get(vertex)
        .copied()
        .unwrap_or([1.0, 0.0, 0.0, 0.0]);

    let mut point = Vec3::ZERO;
    let mut total = 0.0;
    for slot in 0..4 {
        let weight = weights[slot];
        if weight == 0.0 {
            continue;
        }
        let joint = joints
            .get(indices[slot] as usize)
            .copied()
            .unwrap_or(Mat4::IDENTITY);
        point += joint.transform_point(position) * weight;
        total += weight;
    }

    if total > 0.0 {
        point / total
    } else {
        position
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader;

    fn node(name: &str, translation: Vec3) -> Node {
        Node {
            name: name.to_string(),
            local: Transform {
                translation,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn children_inherit_parent_transforms() {
        let mut scene = Scene::default();
        scene.nodes.push(node("root", Vec3::new(0.0, 1.0, 0.0)));
        scene.nodes.push(node("child", Vec3::new(0.0, 2.0, 0.0)));
        scene.nodes[0].children.push(1);
        scene.nodes[1].parent = Some(0);
        scene.roots.push(0);

        let world = scene.world_transforms(&scene.default_pose());
        assert_eq!(
            world[1].transform_point(Vec3::ZERO),
            Vec3::new(0.0, 3.0, 0.0)
        );
    }

    /// Blender exports skin joints that appear in no scene graph; they must be
    /// treated as roots or every skinned mesh collapses to the origin.
    #[test]
    fn orphaned_nodes_act_as_roots() {
        let mut scene = Scene::default();
        scene
            .nodes
            .push(node("scene_root", Vec3::new(0.0, 1.0, 0.0)));
        scene
            .nodes
            .push(node("orphan_joint", Vec3::new(0.0, 7.0, 0.0)));
        scene.roots.push(0);

        let world = scene.world_transforms(&scene.default_pose());
        assert_eq!(
            world[1].transform_point(Vec3::ZERO),
            Vec3::new(0.0, 7.0, 0.0)
        );
    }

    /// Diagnostic rather than an assertion: the skinning formula must collapse
    /// to identity at the rest pose. Run it against a rigged model with
    ///
    /// ```text
    /// $env:FF_VIEW_TEST_MODEL = "path\to\rigged.glb"
    /// cargo test -p ff-view bind_pose -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs FF_VIEW_TEST_MODEL pointing at a rigged glTF"]
    fn bind_pose_skinning_is_identity() {
        let Ok(path) = std::env::var("FF_VIEW_TEST_MODEL") else {
            return;
        };
        let scene = loader::load(std::path::Path::new(&path)).expect("load test model");
        let world = scene.world_transforms(&scene.default_pose());

        for (mesh_node, node) in scene.nodes.iter().enumerate() {
            let (Some(_), Some(skin_index)) = (node.mesh, node.skin) else {
                continue;
            };
            let skin = &scene.skins[skin_index];
            let inverse_mesh = world[mesh_node].inverse();

            let mut worst = 0.0f32;
            for (joint, inverse_bind) in skin.joints.iter().zip(&skin.inverse_bind) {
                let matrix = inverse_mesh * world[*joint] * *inverse_bind;
                for (value, identity) in matrix.0.iter().zip(Mat4::IDENTITY.0.iter()) {
                    worst = worst.max((value - identity).abs());
                }
            }

            println!(
                "mesh node {mesh_node} ({}) skin {skin_index}: {} joints, worst deviation from identity {worst:.5}",
                node.name,
                skin.joints.len()
            );
        }
    }
}
