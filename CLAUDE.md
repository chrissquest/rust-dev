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
combat (done), 4 enemies (pieces 1 to 3 done), 5 dungeon, 6 lighting and audio, 7 full loop (hub,
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

Step 4, enemies, is split into five pieces, agreed with the owner: (1) enemy
basics and the Warrior, (2) ranged enemies (Crab, the turret, LASERCrab) and their
shots hurting the player (done as 2a, Crab and turret; 2b is LASERCrab's
sweeping beam), (3) dashers (Drone and the dashing AI), (4) explosions
hurting the player and enemies walking into them, plus EffectGrate, (5) the boss
(Priest) and its bar.

Piece 1 is done: `sim/enemy.rs` runs Unity's `Warrior` AI for the Warrior and the
EnemyTank (`assets/enemies.json`, with animations converted from Unity clips).
An enemy is a `Target` with `enemy: Some(Enemy)`; knockback, freezing slowdown and
tint, loot and bodies (`RemainsKind::Body`) work. `Player::hurt` has Unity's 0.5 s
invulnerability; at 0 health the player restarts the room (placeholder). The hub's
gun room has a practice EnemyTank, so homing shots fired from the spawn chase it.
Not yet: enemies and player pushing each other, door barriers, hurt flash.

Piece 2a is done: `Ai::Ranged` (Crab) and `Ai::Turret` in `sim/enemy.rs`, bites
on touch (`TouchDef`, Unity's `BasicAIAttack`), and enemy bullets
(`Combat::shots`, `enemy::step_shots`), defined in `guns.json` beside the players'.

Piece 2b is done: `Ai::Laser` (LASERCrab) with its sight, warning and firing
lines (`Enemy::beam`, drawn by `View::draw_enemy_beams`).

Piece 3 is done: `Ai::Drone` (rises, drifts, flies over ledges via
`collide::move_flyer`), `ExplosionTrigger` (`EnemyDef::detonate`) and explosive
deaths. Explosions now hurt and push the player (`combat::explode`), which was
part of piece 4. `DashAI.cs` is used by no prefab and is not ported.

Drag follows Unity 2D (Box2D): velocity divided by `1 + drag * dt` per physics
step (`combat::drag_kept`, the player's `RESPONSE`), switched in step 4 at the
owner's request; the player now tops out at 2.22 units/s, not 1.82.

**Next: step 4 piece 4**: explosions lasting their 0.3 s (hurting what walks in)
and EffectGrate hazards. Then piece 5, the boss. Convert each new kind of data in
`cos-convert` when the step needs it, not before.

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
  It overwrites `rooms/`, `guns.json`, `loot.json`, `font.json`, `sprites.json` and `sprites/`.
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
