# cycle-of-steel

Cycle of Steel, a top-down roguelike shooter, being ported from its Unity project
onto [`crystal-engine`](../../crates/crystal-engine). The Unity project stays the
reference for behaviour while the port catches up.

## State of the port

Step 1 of the port, the 2D foundation, is in: a room drawn from tiles, the player
walking in it with their run animations, collision against walls, a camera that
follows inside the room, and keyboard and gamepad input. It also builds for
Android, with an on-screen stick for movement.

The room is a stand-in built in code (`sim::room::Room::test_room`) from the real
tileset. Real rooms, guns, enemies, the dungeon, lighting, audio and menus come in
later steps.

![The stand-in room with the player in the middle](docs/step1-room.png)

## Run

```powershell
cargo run -p cycle-of-steel                 # the game
cargo run -p cycle-of-steel -- --help       # every option
cargo test -p cycle-of-steel                # simulation and input tests, no GPU needed
```

Keys: WASD or the arrows to move (or a gamepad's left stick or d-pad), F11 for
fullscreen, Escape to quit.

`--screenshot` runs the game with no window, steps the simulation, draws one frame
off-screen and saves it. It needs a GPU or a software rasteriser but no display:

```powershell
cargo run -p cycle-of-steel -- --screenshot room.png --ticks 120 --move 1,0
```

On Linux, building needs libudev's headers for gamepad support
(`apt-get install libudev-dev`).

## Android

The same game builds as an APK for 64-bit ARM phones, Android 8.0 and up. It runs
in landscape and is played with an on-screen stick: touch the left half of the
screen and drag.

**Getting a build onto a phone.** Every push that touches the game runs the
`Android APK` workflow, which replaces the `android-latest` pre-release on GitHub.
On the phone, open the repository's Releases page, download `cycle-of-steel.apk`
from "Android test build" and open it. Android asks once to let the browser install
apps. Later builds install over earlier ones.

**Building it yourself** needs the Android SDK and NDK and `cargo-apk`:

```sh
rustup target add aarch64-linux-android
cargo install cargo-apk --version 0.10.0 --locked
sdkmanager "platforms;android-33" "build-tools;34.0.0" "ndk;25.2.9519653"
export ANDROID_NDK_ROOT=$ANDROID_HOME/ndk/25.2.9519653
cargo apk build -p cycle-of-steel --lib --release     # target/release/apk/cycle-of-steel.apk
cargo apk run -p cycle-of-steel --lib --release       # build, install over USB, start
adb logcat -s cycle-of-steel                          # the game's log
```

The NDK is pinned to r25c because that is what `cargo-apk` 0.10 supports; its
highest platform is 33, hence `target_sdk_version = 33`.

How Android differs from desktop, and where that lives:

- **Entry point.** There is no `main`: the system loads the library and calls
  `android_main` (`src/lib.rs`). The APK holds no Java code (`NativeActivity`).
- **Surface.** Android takes the drawing surface away whenever the app leaves the
  screen. `app.rs` drops everything built on it in `suspended` and rebuilds it in
  `resumed`; the world is untouched, so the game is where you left it.
- **Assets.** They are entries inside the APK, read through `AssetSource::Apk`
  (`src/assets.rs`). Nothing else in the game knows where bytes come from.
- **Input.** Touch events drive `input::TouchStick`. Controllers are not wired up
  on Android yet: the desktop gamepad library has no Android backend.

**The signing key in `android/` is not a secret.** It is committed, with its
password in `Cargo.toml`, so that builds from any machine can install over each
other. It must never sign a store release; that needs a private key kept out of
the repository.

## How it is put together

```text
src/
  main.rs        command line
  app.rs         window, fixed-timestep frame loop, screenshot mode
  input.rs       keys, gamepads and the touch stick -> sim::Input
  view.rs        world state -> sprites; the camera
  assets.rs      loads assets from a folder, or from inside the APK on Android
  sim/           the game itself: no window, GPU or clock
    mod.rs       World, Input, the 60 Hz step
    room.rs      tile layers and the solid grid
    collide.rs   moving a box through solid cells
    player.rs    movement, facing, animation
assets/
  sprites/       PNGs copied from the Unity project
android/
  test-signing.keystore   signs test APKs; not a secret, see "Android"
```

`sim` advances only through `World::step`, 60 times a second, whatever the frame
rate. The frame loop feeds real time into an accumulator, steps while it holds a
whole step, and passes the leftover fraction to the view, which blends between the
last two steps. Because `sim` touches no window, GPU or clock, tests step it
directly and the same inputs always give the same world.

## Matching Unity

Constants that reproduce the Unity project say where they come from. The player's
movement, for example, is derived from the old force, mass and drag values and
tested against them (`sim/player.rs`): top speed about 1.82 units per second.

Units are Unity's: sprites at 64 pixels per unit, tiles half a unit, rooms 16 by 9.

## Licence

The code is MIT or Apache-2.0 like the rest of the repository. The files under
`assets/` are not: see [`assets/LICENSE.md`](assets/LICENSE.md).
