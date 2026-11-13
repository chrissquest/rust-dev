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
drag); `effects` holds the splash or explosion, an explosion with its `damage`
and `radius`. Only bullets and effects that some gun reaches are written.

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
  json.rs      JSON output that keeps a row of tiles on one line
```

Not converted yet: animations, enemy stats, and audio.
