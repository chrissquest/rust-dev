# rust-dev: notes for Claude

One Cargo workspace. The active project is the port of **Cycle of Steel** (a Unity
top-down roguelike shooter) to Rust. Read `apps/cycle-of-steel/README.md` and
`apps/cos-convert/README.md` before changing either; they describe the layout and
formats and are kept current.

## The Cycle of Steel port

| Where | What |
| --- | --- |
| `crates/crystal-engine` | Rendering engine. The 2D layer is `sprite`, `tilemap`, `camera2d`, `render2d`, `surface`. |
| `apps/cycle-of-steel` | The game. `src/sim/` is the simulation: no window, GPU or clock, fixed 60 Hz step. |
| `apps/cos-convert` | One-off converter from the Unity project to `apps/cycle-of-steel/assets`. |
| `chrissquest/CycleOfSteel` | The Unity project, the reference for behaviour. Attach it with `add_repo` when a step needs Unity data; clone beside this repo. |

The plan and status live in a Claude doc: https://claude.ai/artifact/YDve6QfTC7BHiE1mXo5GQe
(tabs: Overview with a Status section, Port plan, Engine, Lighting, Assets and data,
Phase 2, Android). Update its Status section when a step lands.

### Where the port stands

Phase 1 steps: 1 foundation (done), 2 asset conversion (rooms and guns done), 3
combat (half done), 4 enemies, 5 dungeon, 6 lighting and audio, 7 full loop (hub,
upgrades, save, menus), 8 parity. Android steps 1-4 are done and confirmed on the
owner's phone.

Step 3 so far: guns picked up into a four-slot inventory, single and spray guns
with Unity's heat mechanic, lasers (`Combat::fire_laser`, `Room::ray_hits_wall`),
explosions (`explode` in `sim/combat.rs`: the bazooka's `Explosion 1`), homing
bullets (steered in `Combat::step` at the target nearest the player),
bullets, splashes, targets, camera shake, placeholder HUD, twin touch sticks, the
hub's hidden gun room. Waiting for enemies (step 4): knockback from lasers and
explosions, explosions hurting the player and whoever walks into them while they
last. Waiting for audio (step 6): the laser's loop and the explosion's sound.

**Next, the rest of step 3:** burn and freeze (`ElementalEffectHandler.cs`,
`Bullet_Burn`, `Bullet_Freeze`; bullets already carry `burn`/`freeze` in `guns.json`
but `data.rs` does not read them yet), damage numbers (needs text drawing: bake the
Thaleah pixel font, `Assets/Thaleah_PixelFont`, into an atlas), chests and drops
(`Scripts/Drops`, `Prefabs/Room Objects`, `Prefabs/Drops`). Convert each new kind of
data in `cos-convert` when the step needs it, not before.

### How to work here

- **Match Unity, and say where numbers come from.** Read the C# script and the prefab
  before porting a behaviour; constants that reproduce Unity carry a comment naming
  their source. Deliberate differences are stated in comments and READMEs.
- **Keep `sim` pure.** No winit, wgpu, wall-clock time or system randomness in
  `src/sim/`. Randomness comes from `sim::rng` only. One-shot inputs reach exactly one
  step (`InputState::take_sim_input`, `Input::held_only`).
- **No allocation while playing**: bullets and effects live in reserved vectors.
- **Tests with every change.** Unit tests sit beside the code; `app.rs` has tests that
  load the real hub from `assets/`. Run:
  `cargo test -p cycle-of-steel -p crystal-engine -p cos-convert` and
  `cargo clippy -p cycle-of-steel -p crystal-engine -p cos-convert --all-targets`
  (both must be clean).
- **Look at what you draw.** There is no display here, but a frame can be rendered
  off-screen with the software Vulkan driver that ships with the pre-installed Chromium:

  ```sh
  export VK_ICD_FILENAMES=/opt/pw-browsers/chromium_headless_shell-*/chrome-headless-shell-linux64/vk_swiftshader_icd.json
  cargo run -p cycle-of-steel -- --backend vulkan --screenshot out.png \
      --give shotgun_1 --aim 1,0 --ticks 8        # see --help for --room, --whole-room, --move, --touch
  ```

  Then read the PNG. The on-screen window path cannot run in this container; say so
  rather than claiming it was tested.
- **Re-running the converter:** `cargo run -p cos-convert -- ../CycleOfSteel apps/cycle-of-steel/assets`.
  It overwrites `rooms/`, `guns.json`, `sprites.json` and `sprites/`.
- **Code style:** follow the surrounding files. Module docs explain why a thing exists;
  comments say why, not what. Plain names, no abbreviations.
- **Commits:** descriptive subject, a body that explains the change, on branch
  `claude/game-engine-migration-7uionz`. Push after each working piece. No pull request
  unless asked.

### Android

Every push that touches the game runs `.github/workflows/android.yml`, which builds
the APK and replaces the `android-latest` pre-release:
https://github.com/chrissquest/rust-dev/releases/tag/android-latest. The owner tests on
their phone from that page, so after pushing, check that the run succeeded (GitHub MCP
`actions_list`) and tell them. Building the APK locally is optional; the steps are in
`apps/cycle-of-steel/README.md` and need the SDK from `dl.google.com`.

Code that differs on Android is behind `cfg(target_os = "android")`: the entry point in
`lib.rs`, `AssetSource::Apk`, and gamepads (gilrs has no Android backend). Check it
compiles with `cargo check -p cycle-of-steel --lib --target aarch64-linux-android`
(`rustup target add aarch64-linux-android` first).

### Working with the owner

- They follow from a phone and test Android builds by hand. Reports should say what
  to try, what was checked here and what was not.
- Usage is limited. Keep tool output short (pipe through `tail`/`grep`, never dump
  whole files or build logs), avoid re-reading files already seen, and finish a piece
  and stop rather than idling mid-task.
- Decisions already made: Rust on crystal-engine; port first, improve second; top-down
  camera for now with a 2.5D look later; UI is placeholder until a UI pass; old saves
  are not carried over; game assets are not under the repo's MIT/Apache licence.
