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
combat (done), 4 enemies (done), 5 dungeon (done), 6 lighting and audio (pieces 1 to 3 of 4), 7 full loop (hub,
upgrades, save, menus), 8 parity. Android steps 1-4 are done and confirmed on the
owner's phone.

Step 3 is done: guns picked up into a four-slot inventory, single and spray guns
with Unity's heat mechanic, lasers (`Combat::fire_laser`, `Room::ray_hits_wall`),
explosions (`explode` in `sim/combat.rs`: the bazooka's `Explosion 1`), homing
bullets (steered in `Combat::step` at the target nearest the player),
bullets, splashes, targets, camera shake, placeholder HUD, twin touch sticks, the
hub's hidden gun room, freezing (`sim/elemental.rs`; only targets whose Unity
object has an `ElementalEffectHandler` freeze, so not the hub's dummy). Burning is
deliberately left out: no Unity prefab uses `Bullet_Burn` and the handler's burn
never deals damage. Waiting for enemies (step 4): knockback from lasers and
explosions, explosions hurting the player and whoever walks into them while they
last, a frozen enemy's slowdown (`Elemental::added_drag`) and cyan tint. Waiting for audio (step 6): the laser's loop and the explosion's sound.

Damage numbers are in too (`Combat::damage_numbers`, drawn by `View::draw_text`
with the Thaleah font from `assets/font.json`; the font lacks `" % - = ^ _ ~ ¨`,
which Unity's atlas stores rotated).

Chests and drops (`assets/loot.json`, `sim/loot.rs`): chests are `Target`s with a
`chest` id, solid to the player (`Combat::solids`, `collide::move_box`), that spill
loot (`loot::spill`, Unity's `Killable.Die` and `Spawner`) and leave remains. Drops
fly to the player and add to `World::data_cells` or `Player::health` (1000, from
`Player.prefab`; nothing hurts the player yet). The HUD shows health and data cells.
Waiting for lighting and audio: the chests' and drops' glow, pickup sounds. Waiting
for the UI pass: chests' health bars.

Step 4 (enemies) is done. All seven enemy prefabs run their Unity AI in
`sim/enemy.rs` (`Ai::Warrior` for the Warrior and EnemyTank, `Ranged` Crab,
`Turret`, `Laser` LASERCrab, `Drone`, `Boss` Priest), with bites (`TouchDef`),
enemy shots (`Combat::shots`, homing ones too), explosive deaths, lasting blasts
(`Combat::blasts`) that hurt the player too, water and lava grates
(`Combat::grates`, `Elemental`), knockback, freezing, loot and bodies. The player
can be hurt (`Player::hurt`, 0.5 s invulnerable) and restarts the room at 0
health (placeholder until step 7). The HUD shows health, data cells and a boss
bar. Not yet: enemies and the player pushing each other, door barriers, the
player's hurt flash, LASERCrab sparks; health bars over enemies (UI pass).

Drag follows Unity 2D (Box2D): velocity divided by `1 + drag * dt` per physics
step (`combat::drag_kept`, the player's `RESPONSE`), switched in step 4 at the
owner's request; the player now tops out at 2.22 units/s, not 1.82.

Step 5 (the dungeon) is done. A run is the hub with a floor built one empty
block to its right (`World::new_run`, `sim/dungeon.rs` running Unity's
`DungeonGenerator` on `assets/dungeon.json`), the default start; `--room dungeon`
starts in the floor. Rooms seal while their counted enemies live
(`Floor::seal`, `SealTile`: door tiles, or the boss room's wall layers, which
unlike Unity reopen when the boss dies); `ExitHub` enters, `ExitDungeon` leaves
and, once all but two rooms are cleared, makes a new floor at difficulty 2
(`World::new_floor`, `room_version` makes the view re-upload); death respawns in
the hub on a new floor. The minimap (`Floor::map`, Tab / Select / MAP button)
is `MapMenu`. Not yet: the exit dialogue and open-door arrows (UI pass), the
death screen and saves (step 7).

Step 6 (lighting and audio) is split into four pieces, agreed with the owner:
(1) the light buffer and the rooms' lights, (2) wall shadows, (3) lights on
things (chests, drops, enemies, bullets, explosions), (4) audio. The plan's
depth cues (bloom, vignette...) wait until after parity: port first.

Piece 1 is done. `crystal-engine` has 2D lights: `Sprite::lit(layer)`, a
half-resolution light buffer with `LIGHT_LAYERS` layers filled by
`Renderer2d::draw_lights` from a `LightList` (point lights, polygon lights by
layer mask), multiplied into lit sprites. A layer no light reaches stays white
(Unity draws it unlit). The game maps Unity's sorting layers to light layers
(`view::light_layer`) and gathers the room's `lights` near the camera each
frame (`View::gather_lights`).

Piece 2 is done: lights with `shadow` (the converter already wrote Unity's
`m_ShadowIntensity` when shadows are on) go through `LightList::shadowed_point`
and `shadowed_polygon`; `Renderer2d::draw_lights` draws each one's shadow mask
from the occluder edges (`LightList::set_occluders`, the room's
`Room::wall_edges`) and then the light through it, on the occluders' layers
(Background only, as Unity's casters).

Piece 3 is done: bullets, effects, chests, drops and enemies carry their
prefabs' `Light2D`s (`lights` in guns.json, loot.json, enemies.json, written by
`room::prefab_lights`), added each frame by `View::gather_lights` at the
thing's place and rotation. The guns' own lights (glow, muzzle flash) are not
ported yet.

**Next: step 6 piece 4, audio.**

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

  On the owner's Windows machine there is a real GPU: drop the export and
  `--backend vulkan`. The sandbox there may refuse writes outside the repository,
  so write screenshots under `target/` (git-ignored). The same sandbox makes
  crystal-engine's `terrain::tests::loads_a_folder_with_16_bit_heights` fail with
  "Access is denied" (it writes to the system temp folder); that is the sandbox,
  not the code. The local clippy there is newer and also warns about
  `chunks_exact` in older code; those warnings predate step 3's last piece.
- **Re-running the converter:** `cargo run -p cos-convert -- ../CycleOfSteel apps/cycle-of-steel/assets`.
  It overwrites `rooms/`, `guns.json`, `loot.json`, `enemies.json`, `dungeon.json`, `font.json`,
  `sprites.json` and `sprites/`.
  On the owner's Windows machine the Unity project is at `../../Unity/CycleOfSteel`
  (relative to this repo), and a run there changes a few room rotations in the last
  digit; revert those rather than commit them.
- **Code style:** follow the surrounding files. Module docs explain why a thing exists;
  comments say why, not what. Plain names, no abbreviations.
- **Commits:** descriptive subject, a body that explains the change, on branch
  `claude/game-engine-migration-7uionz`. Push after each working piece. No pull request
  unless asked.

### Android

Builds are opt-in, every time (the owner's choice). `.github/workflows/android.yml`
builds the APK only when started from the Actions tab or when a pushed commit's
message contains `[android]`; it replaces the `android-latest` pre-release:
https://github.com/chrissquest/rust-dev/releases/tag/android-latest. The owner tests on
their phone from that page. Building the APK locally is optional; the steps are in
`apps/cycle-of-steel/README.md` and need the SDK from `dl.google.com`.

Put `[android]` in a commit message only when the owner asks for a phone build,
and do not wait for the run: push and report.

Windows and Linux builds run only on request (`.github/workflows/desktop.yml`):
from the Actions tab ("Desktop builds" -> "Run workflow"), or by putting
`[desktop]` in a commit message. They replace the `desktop-latest` pre-release.

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
