# crystal-engine

A general-purpose rendering engine on [`wgpu`](https://wgpu.rs/): scenes, skinned
meshes, animation clips and glTF loading, plus a 2D layer of sprites and tiles. It started as a triangle experiment
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
  terrain.rs     terrain folder (terrain.json + height/splat PNGs) -> scene
  renderer.rs    wgpu: pipelines, buffers, bind groups, draw loop
  sprite.rs      2D: one textured quad as plain data, sprite sheets, PNG decoding
  tilemap.rs     2D: a grid of tileset cells -> sprites
  camera2d.rs    2D: straight-down orthographic camera, pixel-space camera for UI
  render2d.rs    wgpu: instanced sprite drawing into any target, off-screen readback
  surface.rs     wgpu: a window's swapchain and device, with no pipelines attached
  shaders/
    model.wgsl   skinned vertex stage, two-light fragment stage
    terrain.wgsl pixel-art splat blend over a heightmap grid
    sprite.wgsl  one quad per instance, built from the vertex index
examples/
  triangle/      winit + wgpu from scratch, kept as the minimal reference
```

The dependency direction is one-way: `loader` -> `scene` -> `renderer`, and
`sprite`/`tilemap`/`camera2d` -> `render2d`. Nothing above `renderer`, `render2d`
and `surface` knows about wgpu, which is what makes an app's `--info` mode and
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

### The 2D layer

A `Sprite` is one quad with a full 3D position: x right, y up, z height above the
ground. `Camera2d` looks straight down, so a larger z is nearer and wins the depth
test. Sprites are alpha-blended and depth-tested; add them back to front and
blending is right, and the depth buffer keeps the same scene working once a camera
stops looking straight down.

`Renderer2d` owns no window. It is given a device and a target format and records
passes into the caller's encoder:

```rust
renderer.begin_frame();
list.clear();
list.batch(room_tiles);              // uploaded once with add_batch
list.sprite(player_sheet, sprite);   // re-uploaded every frame
renderer.draw(&mut encoder, target, &Pass { view_projection, clear, viewport }, &list);
```

A run of sprites sharing a texture is one instanced draw call, and a static batch
(a room's tile layer) is one call however many tiles it holds. The target can be a
`surface::WindowSurface` frame or a `render2d::Offscreen` texture, which is how an
app takes screenshots with no window; `apps/cycle-of-steel` does both.

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

### Terrain

A terrain is a folder, not a mesh, so ground can be painted (and later changed
at runtime) instead of modelled:

```text
meadow/
  terrain.json   size, height range, layers, optional props .glb
  height.png     8- or 16-bit grey, one sample per grid vertex
  splat.png      RGBA weights for layers 0-3 (row 0 = -Z edge, column 0 = -X edge)
  splat2.png     optional, layers 4-7
```

```json
{
  "size": [32, 32],
  "height_range": [-0.96, 4.12],
  "splat2": "splat2.png",
  "layers": [
    { "name": "grass",  "texture": "../../textures/grass.png",  "gain": 0.45 },
    { "name": "dirt",   "texture": "../../textures/dirt.png",   "gain": 0.25 },
    { "name": "cobble", "texture": "../../textures/cobble.png", "gain": 0.55 },
    { "name": "rock",   "texture": "../../textures/rock.png",   "gain": 0.35, "slope": [0.16, 0.30] }
  ],
  "props": "../../models/nature_props.glb",
  "blend": { "texels_per_metre": 16, "tile_metres": 2, "wobble": 0.45,
             "wobble_period": 0.8, "grain": 0.1, "contact_shadow": 0.62 }
}
```

`blend` is optional (those are the defaults); so are `splat2`, `props` and each
layer's `gain` (0.35) and `slope`. `terrain::load` builds a grid mesh with smooth
normals and returns a `Scene` holding the terrain plus the props, so everything
else - bounds, camera framing, `--info` - works unchanged.

The shader never blends colours. Per **texel** (world XZ snapped to
`texels_per_metre`) it scores every layer:

```text
score = splat weight + slope term
      + gain   * (texel brightness - 0.5)     stones and blade tips stand proud
      + wobble * (value noise at ~0.8 m - 0.5)  borders meander, per-layer offset
      + grain  * (per-texel hash - 0.5)        no long straight runs
```

and draws the winner's texel as-is, so borders are crisp pixel steps and every
pixel stays on the palette. A texel whose +X or +Z neighbour belongs to a higher
layer is darkened by `contact_shadow`. Layer textures go into one
`texture_2d_array` (nearest), the splat maps into another (linear, clamped,
non-sRGB because weights are data). One draw call per terrain.

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
