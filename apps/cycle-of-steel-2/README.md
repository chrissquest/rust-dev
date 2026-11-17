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
| `src/sim/gun.rs` | The feel prototype's one gun and its bullets: fire rate, spread, damage range, crits, recoil. |
| `src/sim/target.rs` | Training dummies on springs, with a running total per burst. |
| `src/sim/feedback.rs` | Damage numbers, sparks, screen shake and hit-stop, in fixed pools. |
| `src/view.rs` | World to sprites, and the camera (follows the player, leads the aim, carries the shake). |
| `src/touch.rs` | Floating twin touch sticks: left half moves, right half aims and fires past a dead zone. |
| `src/art.rs` | Placeholder pixel art drawn in code, packed into one atlas with white silhouettes for hit flashes, and a 3x5 digit font. |
| `src/app.rs` | Window, fixed-step frame loop, Android suspend and resume, `--screenshot`. |
| `src/main.rs` | Desktop command line (`--help`). |
| `src/lib.rs` | Module map and the Android entry point. |

It shares `crates/crystal-engine` with Cycle of Steel 1. Engine changes must keep
Cycle of Steel 1's tests passing.

## Running

```sh
cargo run -p cycle-of-steel-2   # WASD move, mouse aims, left button or Space fires,
                                # arrow keys aim and fire, Escape quits. On a phone: left thumb moves,
                                # right thumb aims and fires (floating twin sticks, `src/touch.rs`)
cargo test -p cycle-of-steel-2
```

Without a display, render a frame off-screen (software Vulkan, as for Cycle of Steel 1):

```sh
export VK_ICD_FILENAMES=/opt/pw-browsers/chromium_headless_shell-*/chrome-headless-shell-linux64/vk_swiftshader_icd.json
cargo run -p cycle-of-steel-2 -- --backend vulkan --screenshot target/cos2.png --ticks 47 --aim 1,0 --fire
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

None yet: the feel prototype's sprites are drawn in code (`src/art.rs`). Cycle of Steel 1's
sprites and sounds can stand in where code-drawn art won't do; they aren't under the
repository's code licence.

## Feel tuning

The numbers to tune by playing are constants with comments: `sim/gun.rs` (fire rate,
spread, damage, crit chance and multiplier, recoil), `sim/feedback.rs` (number life,
shake per hit and per crit, hit-stop steps), `sim/target.rs` (knockback spring, burst
window) and `view.rs` (number sizes and pop, camera lead).
