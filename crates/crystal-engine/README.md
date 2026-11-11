# crystal-engine

A general-purpose rendering engine on [`wgpu`](https://wgpu.rs/): scenes, skinned
meshes, animation clips and glTF loading. It started as a triangle experiment
(see [blog post](https://garstka.dev/2026/09/05/rust-wgpu-exploration/)) and
took over the renderer that [`ff-view`](../../apps/ff-view) grew.

It is a library: it never opens a window or owns an event loop. An app does
that and hands the engine a window to draw into; `ff-view` is the first such app.

## Build and run

```powershell
cargo test -p crystal-engine                       # unit tests, no GPU needed
cargo run -p crystal-engine --example triangle     # the original experiment: a window, one triangle
cargo run -p ff-view -- path\to\model.glb          # the engine inside a real app
```

## How it is put together

```text
src/
  lib.rs         module list and layering
  math.rs        glam (Vec3 / Quat / Mat4) plus Aabb and NaN-safe helpers
  scene.rs       CPU scene: nodes, meshes, materials, textures, skins
  anim.rs        keyframe sampling and clip playback
  camera.rs      orbit camera, fitted from bounds
  loader.rs      glTF 2.0 -> scene (the only file that knows about `gltf`)
  renderer.rs    wgpu: pipelines, buffers, bind groups, draw loop
  shaders/
    model.wgsl   skinned vertex stage, two-light fragment stage
examples/
  triangle/      winit + wgpu from scratch, kept as the minimal reference
```

The dependency direction is one-way: `loader` -> `scene` -> `renderer`. Nothing
above `renderer` knows about wgpu, which is what makes an app's `--info` mode and
the unit tests possible without a GPU.

### Math

[`glam`](https://docs.rs/glam) provides the vector, quaternion and matrix types.
It is column-major like glTF and WGSL, so `Mat4::to_cols_array` goes straight
into a uniform buffer and `Mat4::from_cols_array_2d` reads glTF's matrices
unchanged. `math.rs` adds what glam lacks: `Aabb`, and helpers that fall back to
something drawable where glam would return NaN (`normalize_quat`,
`inverse_or_identity`, `trs`). The engine re-exports `glam` so apps use the same
version.

### Drawing a frame

`Renderer` exposes the frame in three calls - `begin_frame`, `encode_scene`
(which takes the viewport rectangle and the camera), `end_frame` - so a caller
that wants to draw something else on top can insert a pass between them. That is
all `ff-view` needs to put egui over the 3D, and to shrink the 3D to the area its
panel leaves free.

### Skinning

Joint matrices are computed on the CPU once per frame
(`inverse(mesh_node_world) * joint_world * inverse_bind`, per the glTF spec) and
uploaded as a uniform array of 128 `mat4`s; the vertex shader does the four-way
weighted sum. That is the wrong call for thousands of characters, and the right
call for one model at a time: the sampling code stays unit-testable and the
shader stays short. To scale up, joint matrices move to a storage buffer and the
CPU pass becomes an optional fallback.

The formula collapses to identity at the rest pose, which is the cheapest way to
tell whether a rig is being read correctly. Point it at a real file to check:

```powershell
$env:CRYSTAL_TEST_MODEL = "path\to\rigged.glb"
cargo test -p crystal-engine bind_pose -- --ignored --nocapture
```

### Flat rigs

Some exporters - Blender's glTF exporter among them - emit a skin's joints as
nodes that appear in no scene graph at all, referenced only from `skin.joints`.
Those nodes are treated as roots, because the alternative is leaving them at the
identity and having every skinned mesh collapse to a point. One of the assets
used to test this renderer had 62 of its 71 joints in exactly that state.
`scene::tests::orphaned_nodes_act_as_roots` covers it.

## Current limits

- **No mipmaps.** Textures upload at full size and alias at distance. This is
  the first thing that will look wrong on a big sheet.
- **Alpha is cut-out only.** `alphaMode: MASK` discards, everything else is
  opaque; there is no blending pipeline yet.
- **Cubic-spline clips are approximated.** Tangents are dropped and the key
  values are interpolated linearly.
- **Morph targets are skipped.**
- **glTF only.** No OBJ, no FBX. Adding a second format means writing another
  `-> Scene` function, not touching the renderer.
- **One model at a time.** The renderer draws one `Scene`; there is no world or
  entity layer above it yet.
