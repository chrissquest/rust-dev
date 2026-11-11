# ff-view

A low-poly model viewer: glTF/GLB in, wgpu out. Skinning and animation clips
work; the point is to look at exported Fossil Fighters creatures without
bouncing through Blender. The renderer it grew is now
[`crystal-engine`](../../crates/crystal-engine); this app is the window around it.

Deliberately **not** Fossil-Fighters-specific. It consumes glTF, so anything
`ff-export` produces - or any other exporter - opens the same way. The coupling
between the two projects is a file format, which is why neither depends on the
other even though both now live in this repository.

The package builds two binaries on one library, and they are the same window:

| Binary | What it is |
| --- | --- |
| `ff-view` | one file full-window; with a folder - passed, remembered or dropped - the same window grows the creature/clip panel. `--info` / `--screenshot` live here |
| `ff-browser` | the same window with the panel always on, starting on the last folder |

## The picker

![ff-view with an export folder open](docs/picker.png)

An export folder makes the window list its creatures and clips. A folder can
arrive three ways: as the path, as a drop on any `ff-view` window, or as the
remembered folder on the next launch.

```powershell
cargo run -p ff-view -- .\out\ff1_usa
cargo run -p ff-browser
```

Either way it reads `manifest.json`, lists the creatures, and renders the
selected clip with the same renderer - textures, skinning and animation included.

```text
left drag            orbit             ↑ ↓   previous / next creature
middle / right drag  pan               ← →   previous / next clip
wheel                zoom              space pause
```

Each entry in the manifest is a clip, not a creature, because every clip carries
its own mesh and bind pose; the window therefore loads one `.glb` per selection.

Creatures open on **idle** by default (`--clip` overrides that), and switching
creatures keeps the clip you were watching, matched by name: pick **idle** on one
creature and the next opens on its own idle, at whatever position that clip sits
in its list - or on its first clip if it has no idle at all. Pause and speed carry
over with it, and so do the camera angle and zoom: switching re-points the orbit
target at the new creature instead of reframing it. **↑/↓** repeat while held and
stop at the ends of the list rather than wrapping.

The camera frames the rest pose and then follows the animated centre, so a lunge
does not walk out of frame. **T**, or the *Follow animation* checkbox in the
panel, turns that follow off (it is on by default). Near and far planes track the
zoom level, so zooming a long way out never clips the model away.

A dropped folder only needs `manifest.json` with `creatures[].clips[].glb`; the
paths are resolved relative to the folder. Dropping a loose `.glb` also works and
shows that file alone. `ff-browser` opens on the last folder `ff-view` opened, or
on `apps\ff-export\externals` when run from the repository root.

### Why egui here and Slint in `ff-export`

The 3D scene is drawn straight to the swapchain inside the panel's rectangle and
egui draws over it in a second pass on the same encoder - one window, one device,
no read-back, and the panel is plain Rust in this crate.

Slint could host it too, but only through its wgpu interop: with the
`unstable-wgpu-30` feature, `BackendSelector::require_wgpu_30` shares the device
and queue with the app, and `slint::Image::try_from(wgpu::Texture)` imports a
texture the viewer rendered into, so the viewport could live in the UI without a
read-back. That path is feature-flagged as unstable and versioned per wgpu major,
and it would move the panel into `.slint` files plus a build step. For a panel
with one list that is not yet worth it - and Slint's licensing (GPLv3, a
royalty-free desktop licence, or commercial) is a different trade from egui's
MIT/Apache-2.0. `ff-export`'s window has no 3D in it, so it stays Slint.

## Starting it with no arguments

```powershell
cargo run -p ff-view
```

opens the picker window with nothing loaded: **Folder...**, or a drop of one
`.glb`/`.gltf` or a whole export folder. A folder is remembered the moment it is
opened, before any clip loads, so closing the window cannot lose it - the next
bare launch reopens it on its first creature and clip.

It is written to `%APPDATA%\ff-view\settings.json` (`$XDG_CONFIG_HOME/ff-view/settings.json`
elsewhere). If that location cannot be written - a locked-down profile, an
antivirus rule, or a terminal that confines writes to the repository - saving
falls back to local app data, then to `~/.ff-view\settings.json`, and finally to
`ff-view-settings.json` beside the executable (which is `target\debug` for
`cargo run`). The file actually used is printed on open. `FF_VIEW_CONFIG` pins
the location outright, which is the way to keep it somewhere specific, for
example inside the repository:

```powershell
$env:FF_VIEW_CONFIG = ".\ff-view-settings.json"
```

To see what a launch would reopen, and where it comes from:

```powershell
cargo run -p ff-view -- --settings
```

A folder passed as the path is the same thing without the dragging, and so is a
folder dropped on a window that is already showing a model - the panel appears
either way:

```powershell
cargo run -p ff-view -- .\out\ff1_usa
cargo run -p ff-view -- .\out\ff1_usa --info
```

Inside a folder, **up/down** walk creatures and **left/right** walk that
creature's clips, so the list is reachable without the mouse. A folder with no
`manifest.json` still works - each `.glb` in it becomes a one-clip entry - which
is the fastest way to look at a handful of files someone sent you.

## Usage

```powershell
cargo run -p ff-view -- ".\out\ff1_usa\glb\breme_030_attack.glb"

# what is in the file, without opening a window
cargo run -p ff-view -- path\to\model.glb --info

# pick a clip, start in wireframe
cargo run -p ff-view -- model.glb --clip roar --wireframe

# render five frames and exit: a smoke test for CI or a GPU-less shell
cargo run -p ff-view -- model.glb --frames 5

# one frame to a PNG, off-screen: for asset diffs, no window appears
cargo run -p ff-view -- model.glb --screenshot shot.png

# the same, from a fixed camera: yaw, pitch (degrees), distance, target x,y,z
cargo run -p ff-view -- model.glb --view "15,14,9,0.5,0.6,-3" --screenshot shot.png

# a terrain folder (one holding terrain.json): splat-blended ground plus its props
cargo run -p ff-view -- path\to\terrain\meadow
```

A folder with a `terrain.json` opens as a crystal-engine terrain rather than as a
model listing, whether it is passed, or dropped on the window. See the
[engine README](../../crates/crystal-engine/README.md#terrain) for the format.

```
--info                  print the scene's (or folder's) contents and exit
--settings              print where the last folder is remembered and exit
--clip <index|name>     start on a specific animation clip
--wireframe             start in wireframe mode (needs POLYGON_MODE_LINE)
--bind-pose             ignore animation clips and show the rest pose
--backend <name>        gl | vulkan | dx12 | metal | all (default: all)
--frames <n>            render n frames, then exit
--screenshot <file>     render one frame off-screen, save it, and exit
--view <y,p,d,x,y,z>    start the camera here instead of framing the scene
```

`--info` never touches wgpu, and `--screenshot` renders into an off-screen
texture, so both work on a machine with no visible desktop.

## Controls

| Input | Action |
| --- | --- |
| left drag | orbit |
| middle drag, right drag, shift + left drag | pan |
| wheel | zoom |
| drop a file or folder | load it (a folder opens the panel and is remembered) |

Default keys (all rebindable, see below):

| Key | Action (`settings id`) |
| --- | --- |
| F | orbit around the point under the cursor (`focus_cursor`) |
| R | reset camera and restart the clip (`reset_camera`) |
| T | toggle camera follow, on by default (`toggle_follow`) |
| W | toggle wireframe (`toggle_wireframe`) |
| Space | pause / resume the clip (`toggle_pause`) |
| N / P | next / previous clip in this file (`next_clip_in_file`, `previous_clip_in_file`) |
| Up / Down | previous / next creature, repeats while held (`previous_creature`, `next_creature`) |
| Left / Right | previous / next clip in the folder panel (`previous_clip`, `next_clip`) |
| Esc | quit (`quit`) |

**F** casts a ray from the cursor into the scene (meshes as currently posed, and
terrain) and makes the hit point the orbit centre *without moving the eye*: the
view turns to it, and from then on orbit and zoom pivot there. Pointing at empty
sky does nothing.

### Rebinding keys

Keys live in `input.rs` as `Action`s; the window only ever asks the bindings which
action a key press means, so nothing else needs to change when keys do. The
settings file (`ff-view --settings` prints where) stores only what differs from
the defaults:

```json
{
  "key_bindings": {
    "focus_cursor": ["KeyF", "Ctrl+KeyF"],
    "toggle_follow": ["KeyL"],
    "toggle_wireframe": []
  }
}
```

Key names are winit's physical `KeyCode` names (`KeyA`, `Digit1`, `F5`, `Space`,
`ArrowUp`, ...), so a binding stays in the same place on any keyboard layout;
`Ctrl+`, `Shift+` and `Alt+` prefixes must match exactly. An empty list unbinds an
action, an action left out keeps its default (so new actions arrive bound), and a
bad entry is skipped with a warning. `ff-view --help` lists the keys as currently
bound. A rebinding popup only needs `Bindings::set`, `Bindings::conflicts` and
`settings::save_key_bindings`.

The 3D area takes pointer input wherever the cursor happens to be over it - no
click to give it focus first - and the edge between it and the panel is a hard
boundary: the wheel over the panel scrolls the creature and clip lists instead of
zooming, and a drag that starts on a slider stays on the slider.

The camera frames the model's bind-pose bounds on load, so assets authored in
centimetres and assets authored in metres both fill the window. With a folder
open it also follows the animated centre so a lunge does not walk out of frame;
panning still works, because only the *change* in that follow offset is applied
each frame.

## How it is put together

The scene, glTF loading, animation, camera and drawing all come from
[`crystal-engine`](../../crates/crystal-engine), whose README covers skinning,
the renderer's frame API and the current rendering limits. What is left here is
the app:

```text
src/
  lib.rs         the library both binaries use
  main.rs        `ff-view`: the CLI, then `ui::run`
  bin/browser.rs `ff-browser`: the same window, panel always on
  ui.rs          the window: scene, folder panel, input, title
  library.rs     an export folder: manifest.json, or a scan for .glb files
  settings.rs    the last folder, remembered between launches
```

Nothing below `ui` knows about a window. Both binaries are thin: they decide what
to open and hand it to `ui::run`, which puts egui over the engine's 3D by
inserting a pass between `Renderer::encode_scene` and `end_frame`.

Fossil Fighters exports use linear keyframes and no morph targets, so the
engine's limits on cubic-spline clips and morph targets do not affect them.

## Workspace

This crate is a member of the `rust-dev` workspace at the repository root,
alongside `ff-export` and `crystal-engine`; see the root README.
