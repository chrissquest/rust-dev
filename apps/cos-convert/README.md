# cos-convert

Turns the Cycle of Steel Unity project into the asset files the Rust port loads.
It reads Unity's text files directly, so Unity does not need to be installed.

It is a one-off tool. Once an asset kind has been converted, the files it wrote are
the source of truth: they are edited by hand, or later by the room editor, and the
converter is only run again if a Unity original changes.

## Run

```powershell
cargo run -p cos-convert -- path\to\CycleOfSteel apps\cycle-of-steel\assets
```

The first argument is the Unity project (the folder holding `Assets/` and
`ProjectSettings/`), the second is where to write. It prints one line per room and
any warnings, and overwrites what is there.

## What it writes

```text
rooms/<room>.json     one per prefab under Assets/Prefabs/Generatable Rooms
guns.json             every gun, the bullets they fire and the effects those leave
loot.json             chests, loot pools and drops
enemies.json          enemies the port has an AI for
dungeon.json          the dungeon generator's settings
sprites.json          every sprite the above use: texture, rectangle, pivot, scale
sprites/<name>.png    the textures themselves, copied unchanged
```

A room file is in room space: x right, y up, origin at the room's bottom-left
corner, in world units (half a unit per tile).

| Key | Holds |
| --- | --- |
| `size`, `cell` | Columns and rows of tiles, and a tile's world size. Dungeon rooms are 32 by 18. |
| `doors`, `room_type` | Which sides have doors, and spawn, random, exit or boss. |
| `palette` | The sprite names the room's tiles use. |
| `layers` | Tile layers, bottom first: sorting layer, order, whether it is switched on, its tiles and which cells are solid. |
| `decorations` | Loose sprites such as racks and wrecked robots, in draw order. |
| `objects` | Nested prefabs to spawn: enemies, chests, hazards, pickups, with any overridden settings. |
| `lights` | Point, freeform and global lights with colour, intensity, range and the sorting layers they light. |
| `barriers` | Boxes at the doors that stop everything except the player. |
| `markers` | Other objects that carried a game script (triggers, the upgrade bench), with their settings. |

A tile is one number: -1 for empty, otherwise `palette index * 8 + transform`. The
transform's low two bits are quarter turns counter-clockwise; adding 4 mirrors the
tile left-to-right first. Each layer's tiles are written one row per line, top row
first, so a room reads the way it looks.

`guns.json` has three tables keyed by Unity prefab name. `guns` holds each gun's
numbers (kind, damage, fire delay, heat capacity and cooldown, spread, camera
shake) and names its bullet; `bullets` holds a bullet's sprite, colour, size,
lifetime and whether it pierces, and names the effect it leaves (homing bullets
also carry `homing`, the steering force, and `drag`, their rigidbody's linear
drag; a freezing bullet carries `freeze`, its strength); `effects` holds the splash or explosion, an explosion with its `damage`
and `radius`. Only bullets and effects that some gun reaches are written.

`loot.json` has three tables keyed by Unity prefab name. `chests` holds each
chest under `Prefabs/Room Objects` (a `Killable`): health, its whole, damaged and
broken sprites (the last from its `deathObject` prefab), its solid box (`size`,
`offset`) and its `loot`, lines of `drop` and `weight` out of 100. `pools` holds
the `Spawner` prefabs those lines name: `multiple` (every line rolls) or not
(exactly one spawns) and their `entries`. `drops` holds every `DataDrop` or
`HealDrop` prefab under `Prefabs/Drops`: `kind` (`data` or `heal`), `value`,
`pickup_range`, `pickup_speed`, the rigidbody's `drag`, and its trigger circle
(`radius`, `offset`). A loot line names a pool, a drop, or a gun in `guns.json`.

`enemies.json` holds, by prefab name, every enemy under `Prefabs/Enemies` whose AI
is ported (`Warrior`, which the EnemyTank also runs, `RangedAI`,
`RangedAITurret`, `RangedAI_Laser`, `DashAIDrone` and `BossAI`): its `Killable`
numbers and `loot`, the bodies its `deathObject` can show (`dead_sprites`), its
sprite, `scale`, `color`, solid box, rigidbody `mass` and `drag`, whether it can
be frozen, the AI's numbers under `warrior` (with its `reach` and four `swings`
boxes), `ranged`, `turret` (with its `muzzles`: offset and angle), `laser` (with the
beam's `width` from its `LineRenderer`) or `drone`, `flying` for the
`FlyingEnemy` layer, `explosion` (the effect an explosive `Killable` leaves, in
`guns.json`), `detonate` (an `ExplosionTrigger` box), `boss` (the Priest's numbers, its three
bullets, staff offset and laser: damage, delay and three beam sprites) with
`boss_bar` (its name), a bite on
touch under `touch` (`BasicAIAttack`), and its `animations` (`idle` is the
controller its Animator starts with). Ranged enemies name their bullet, a key of
`guns.json`'s `bullets`, where enemy bullets are written beside the players'. Boxes are in world units with the prefab's scale
applied. An animation is `{"fps", "frames"}`: the sprites the clip keys, one per
sample, read through the `AnimatorController` the script names.

Room objects carry `overrides`, the settings a placement changes; the game reads
the drops' `value` from them (the hub's drops are worth more than their prefabs).

`dungeon.json` holds the `DungeonGenerator` of `Scenes/Dungeon.unity`: the grid
size and room count (constants in the script), the spawn's place, the room files
it uses for grown, spawn, exit and boss rooms, `by_doors` (for each set of doors,
`N` to `NSEW`, the rooms that fit, in the order it tries them) and `distance`
(each room's `distanceFromSpawn`). Room names are the room files' names.

`font.json` is the pixel font, Thaleah Fat, from the project's 16-pixel bitmap
atlas (`ThaleahFat.fontsettings` and its PNG). `glyphs` maps a character to its
`sprite`, one of the `font_<code point>` entries the converter adds to
`sprites.json`, and its `advance`; both, and `line_height`, are in ems, so a glyph
sprite is one unit per em. The atlas holds its glyphs upside down and is copied as
it is: glyph sprites are drawn flipped top to bottom, and their pivot (the pen
position on the baseline) is given for the stored picture. Eight glyphs the atlas
stores rotated (`" % - = ^ _ ~ ¨`) are left out, with a warning.

## What it decides on the way

- **Room size.** A dungeon room is exactly 32 by 18 tiles, because the dungeon
  generator places rooms on that grid. Tiles painted outside are dropped with a
  warning. The hub is not placed by the generator and keeps its painted size.
- **Collision.** A tile is solid when its tilemap's collider is on and its Tile asset
  has a collider type. Unity's "Sprite" collider type (the sprite's outline) is
  treated as the whole cell. Layers on the `Ledges` physics layer are marked
  `ledge`: they stop walkers but not bullets or flyers.
- **Switched-off layers** are kept with `"enabled": false`. They are the wall sets a
  room swaps in to close a side.
- **Tiles with no picture** still count for collision and are simply not drawn.
- **Light prefabs.** A nested prefab that is only a light, such as the hub's
  spotlights, is written as a light with the room's overrides applied.
- **Shadow casters** are left out. The port derives shadows from the wall tiles.

## How it is put together

```text
src/
  main.rs      command line, the sprite catalogue, copying textures
  yaml.rs      a reader for the subset of YAML that Unity writes
  project.rs   GUID -> asset path, sprite rectangles from .meta files, tile colliders
  room.rs      one room prefab -> one room file
  guns.rs      gun, bullet and effect prefabs -> guns.json
  loot.rs      chest, loot pool and drop prefabs -> loot.json
  enemies.rs   enemy prefabs and their animations -> enemies.json
  dungeon.rs   the dungeon scene's generator -> dungeon.json
  json.rs      JSON output that keeps a row of tiles on one line
```

An explosion's `radius` is its circle's at the prefab's scale; a scaled one also
has `knockback_radius`, the unscaled radius Unity divides its push by.

An enemy whose picture is on a child named `Sprite` (the Priest) gets that
child's sprite, colour and animation, with `sprite_offset`. A `PolygonCollider2D`
is written as the box around its points.

The YAML reader understands a list inside a list (`- - item`), which polygons and
some rule tiles use. Before it did, seven tiles' sprites went unresolved and were
not drawn in seven rooms (the hub, the spawn and exit rooms, ns1, ns2, s1, s2).

Not converted yet: the player's animations (the game
slices its sheets itself), and audio.

Copied textures are left writable: some files in the Unity project are read-only,
and `fs::copy` would carry that over and stop the next run overwriting them.

Run on Windows, the converter writes a few room rotations one digit differently in
the last place (`155.2294` for `155.2295`): float formatting differs from Linux.
Those files need not be committed when nothing else in them changed.
