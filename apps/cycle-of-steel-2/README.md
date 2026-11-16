# Cycle of Steel 2: Parts Collector

A new top-down, tile-based pixel-art shooter in Cycle of Steel's vein. It isn't a
port, and nothing here has to match the Unity game. The core of it is building
weapons from parts and finding combinations that break the game.

The design, decisions and roadmap live in the design doc:
https://claude.ai/artifact/LeeojA3ET6neKbz8HJwr1R

## Layout

| Path | What |
| --- | --- |
| `src/sim/` | The game: world state and one fixed 60 Hz `step`. No window, GPU, clock or system randomness (`sim::rng` only). |
| `src/view.rs` | World to sprites, and the camera. Placeholder squares for now. |
| `src/app.rs` | Window, fixed-step frame loop, Android suspend and resume, `--screenshot`. |
| `src/main.rs` | Desktop command line (`--help`). |
| `src/lib.rs` | Module map and the Android entry point. |

It shares `crates/crystal-engine` with Cycle of Steel 1. Engine changes must keep
Cycle of Steel 1's tests passing.

## Running

```sh
cargo run -p cycle-of-steel-2                 # WASD or arrows to move, Escape to quit
cargo test -p cycle-of-steel-2
```

Without a display, render a frame off-screen (software Vulkan, as for Cycle of Steel 1):

```sh
export VK_ICD_FILENAMES=/opt/pw-browsers/chromium_headless_shell-*/chrome-headless-shell-linux64/vk_swiftshader_icd.json
cargo run -p cycle-of-steel-2 -- --backend vulkan --screenshot target/cos2.png --ticks 30 --move 1,0.5
```

## Android

The package is `com.chrissquest.cycleofsteel2`, so it installs beside Cycle of Steel 1, and it
uses the same committed test key. Check that it compiles with
`cargo check -p cycle-of-steel-2 --lib --target aarch64-linux-android`.

## Builds

As for Cycle of Steel 1, builds run only on request, with their own tags and releases:

- Android: `.github/workflows/cos2-android.yml`, `[cos2-android]` in a commit message or
  "Run workflow" in the Actions tab. It replaces the `cos2-android-latest` pre-release.
- Windows and Linux: `.github/workflows/cos2-desktop.yml`, `[cos2-desktop]`. It replaces
  `cos2-desktop-latest`.

## Assets

None yet. Cycle of Steel 1's sprites and sounds will be the placeholders until an art pass;
they aren't under the repository's code licence.
