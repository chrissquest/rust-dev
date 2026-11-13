//! A dungeon floor: Unity's `DungeonGenerator`, and the rooms it picks joined
//! into one room the rest of the game plays in.
//!
//! The generator works on a grid of screen-sized rooms. From the spawn in the
//! middle it grows rooms outwards, a layer at a time, each new room touching only
//! the one it grew from, until it has `rooms` of them; the last room of the
//! outermost layer becomes the exit, and the boss room is put beside it. Each
//! grown room is then swapped for a room prefab whose doors match its
//! neighbours, picked from the generator's lists in order: the first whose
//! `distanceFromSpawn` the room's layer reaches. The spawn, exit and boss rooms
//! close the sides that lead nowhere with their wall layers (`SpawnRoom`).
//!
//! Joined, the floor is one [`RoomFile`] with each room's tiles, objects and
//! markers moved to its place on the grid, so collision, enemies and drawing
//! need nothing new. Each room is one camera block, as in Unity.

use std::collections::HashMap;

use anyhow::{Context, Result};
use crystal_engine::glam::Vec2;

use super::rng::Rng;
use super::room::{Cell, Room};
use crate::data::{DoorsFile, DungeonFile, LayerFile, RoomFile};

/// `DungeonGenerator`'s give-up count: rounds in a row that add no room.
const MAX_TRIES: u32 = 7;
/// Up, down, left, right: `Coord.cardinals`, in its order.
const CARDINALS: [(i32, i32); 4] = [(0, 1), (0, -1), (-1, 0), (1, 0)];
/// A room's size in cells and in world units (`RoomInfo`).
const ROOM_CELLS: [usize; 2] = [32, 18];
const ROOM_SIZE: Vec2 = Vec2::new(16.0, 9.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomKind {
    Spawn,
    Random,
    Exit,
    Boss,
}

/// A room on the generator's grid.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedRoom {
    pub coord: (i32, i32),
    pub kind: RoomKind,
    /// Which sides lead nowhere, as `RoomScript`'s top, bottom, left and right:
    /// true is closed.
    pub closed: [bool; 4],
    /// The layer it was grown in (`distanceFromSpawn`).
    pub distance: i32,
    /// The room file it becomes.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub rooms: Vec<PlannedRoom>,
}

impl Layout {
    pub fn room_at(&self, coord: (i32, i32)) -> Option<&PlannedRoom> {
        self.rooms.iter().find(|room| room.coord == coord)
    }
}

/// `DungeonGenerator.GenerateDungeon`, with the game's own random numbers.
pub fn generate(file: &DungeonFile, rng: &mut Rng) -> Layout {
    let size = file.grid_size as i32;
    let spawn = (file.spawn[0], file.spawn[1]);
    let mut grid: HashMap<(i32, i32), PlannedRoom> = HashMap::new();
    let in_grid = |(x, y): (i32, i32)| (0..size).contains(&x) && (0..size).contains(&y);
    let new_room = |coord, kind| PlannedRoom { coord, kind, closed: [true; 4], distance: 0, name: String::new() };

    grid.insert(spawn, new_room(spawn, RoomKind::Spawn));
    let mut generated = 1;
    let mut end_rooms = vec![spawn];
    let mut next_end_rooms: Vec<(i32, i32)> = Vec::new();
    let mut layer = 0;
    let mut tries = 0;
    while generated < file.rooms && tries < MAX_TRIES {
        // `GenerateRooms`.
        for &end in &end_rooms.clone() {
            for (index, &(dx, dy)) in CARDINALS.iter().enumerate() {
                let room = (end.0 + dx, end.1 + dy);
                let neighbours = CARDINALS
                    .iter()
                    .filter(|(nx, ny)| grid.contains_key(&(room.0 + nx, room.1 + ny)))
                    .count();
                let valid = in_grid(room) && !grid.contains_key(&room) && generated < file.rooms && neighbours <= 1;
                // `Random.Range(0, 2) == 0`: a coin toss for each place that could grow.
                if valid && rng.unit() < 0.5 {
                    let mut created = new_room(room, RoomKind::Random);
                    open(&mut created.closed, opposite(index));
                    if let Some(from) = grid.get_mut(&end) {
                        open(&mut from.closed, index);
                    }
                    grid.insert(room, created);
                    generated += 1;
                    next_end_rooms.push(room);
                    tries = 0;
                }
            }
        }
        if next_end_rooms.len() >= 2 {
            end_rooms = std::mem::take(&mut next_end_rooms);
            layer += 1;
            for end in &end_rooms {
                if let Some(room) = grid.get_mut(end) {
                    room.distance = layer;
                }
            }
        } else {
            // Unity appends them and keeps them queued for next time too.
            end_rooms.extend(next_end_rooms.iter().copied());
        }
        tries += 1;
    }

    let exit = *end_rooms.last().expect("the spawn at least");
    if let Some(room) = grid.get_mut(&exit) {
        room.kind = RoomKind::Exit;
    }
    for (index, &(dx, dy)) in CARDINALS.iter().enumerate() {
        let boss = (exit.0 + dx, exit.1 + dy);
        if in_grid(boss) && !grid.contains_key(&boss) {
            let mut room = new_room(boss, RoomKind::Boss);
            open(&mut room.closed, opposite(index));
            if let Some(exit_room) = grid.get_mut(&exit) {
                open(&mut exit_room.closed, index);
            }
            grid.insert(boss, room);
            break;
        }
    }

    let mut rooms: Vec<PlannedRoom> = grid.into_values().collect();
    rooms.sort_by_key(|room| (room.coord.1, room.coord.0));
    for room in &mut rooms {
        room.name = match room.kind {
            RoomKind::Spawn => file.spawn_room.clone(),
            RoomKind::Exit => file.exit_room.clone(),
            RoomKind::Boss => file.boss_room.clone(),
            RoomKind::Random => pick_room(file, room),
        };
    }
    Layout { rooms }
}

/// `spawnRandomRoom` and `spawnRoomFromArray`: the first room in the list for its
/// doors that may be this far from the spawn. A list left empty keeps the open
/// room it was grown as.
fn pick_room(file: &DungeonFile, room: &PlannedRoom) -> String {
    let [top, bottom, left, right] = room.closed;
    let mut doors = String::new();
    for (closed, letter) in [(top, 'N'), (bottom, 'S'), (right, 'E'), (left, 'W')] {
        if !closed {
            doors.push(letter);
        }
    }
    file.by_doors
        .get(&doors)
        .and_then(|names| {
            names.iter().find(|name| file.distance.get(*name).copied().unwrap_or(0) <= room.distance)
        })
        .cloned()
        .unwrap_or_else(|| file.open_room.clone())
}

/// `CardinalOpen`: the side facing `CARDINALS[index]` now leads somewhere.
fn open(closed: &mut [bool; 4], index: usize) {
    // Cardinals are up, down, left, right; sides are top, bottom, left, right.
    closed[index] = false;
}

/// The cardinal pointing the other way.
fn opposite(index: usize) -> usize {
    [1, 0, 3, 2][index]
}

/// What a run's floors are made from: the generator's settings, every room it
/// can pick, and the hub they are built beside.
#[derive(Clone, Debug)]
pub struct RunAssets {
    pub settings: DungeonFile,
    pub rooms: HashMap<String, RoomFile>,
    pub hub: RoomFile,
}

impl RunAssets {
    /// The room files a floor can use, by name.
    pub fn room_names(settings: &DungeonFile) -> Vec<String> {
        let mut names: Vec<String> = settings.by_doors.values().flatten().cloned().collect();
        names.extend([&settings.open_room, &settings.spawn_room, &settings.exit_room, &settings.boss_room].map(Clone::clone));
        names.sort();
        names.dedup();
        names
    }
}

/// A floor while it is played: its rooms, and which are sealed.
#[derive(Clone, Debug)]
pub struct Floor {
    pub rooms: Vec<FloorRoom>,
    /// Where the floor starts in the world: everything left of this is the hub.
    pub start_x: f32,
    /// Where entering the dungeon puts the player (`Player.TeleportDungeon`).
    pub entry: Vec2,
    /// Rooms whose enemies have all died (`DungeonGenerator.ClearedRoom`).
    pub rooms_cleared: usize,
}

/// A tile a sealed room shows, and the cell it closes.
#[derive(Clone, Debug, PartialEq)]
pub struct SealTile {
    pub cell: (i32, i32),
    pub sprite: String,
    /// A room tile's transform: quarter turns, plus 4 for mirrored.
    pub transform: u8,
    pub solid: bool,
    /// What the cell was before the room sealed.
    before: Cell,
}

#[derive(Clone, Debug)]
pub struct FloorRoom {
    /// Its screen-sized block in the world.
    pub block: (i32, i32),
    pub kind: RoomKind,
    /// What sealing it puts in place: door tiles in a picked room's doorways
    /// (`RoomScript.SealRoom`), wall layers on the boss room's open sides
    /// (`SpawnRoom.SealRoom`).
    pub seal: Vec<SealTile>,
    pub sealed: bool,
    /// Whether it had counted enemies when the floor was made, and whether they
    /// have all died since.
    pub had_enemies: bool,
    pub cleared: bool,
}

impl Floor {
    pub fn room_at(&self, block: (i32, i32)) -> Option<usize> {
        self.rooms.iter().position(|room| room.block == block)
    }

    /// Puts the room's seal in place; tiles that collide make their cells walls.
    pub fn seal(&mut self, index: usize, room: &mut Room) {
        let floor_room = &mut self.rooms[index];
        if floor_room.sealed {
            return;
        }
        for tile in &mut floor_room.seal {
            tile.before = room.cell_at(tile.cell.0, tile.cell.1);
            if tile.solid {
                room.set_cell(tile.cell.0, tile.cell.1, Cell::Wall);
            }
        }
        floor_room.sealed = true;
    }

    /// `RoomScript.UnsealRoom`: the seal goes and the cells are as they were.
    pub fn unseal(&mut self, index: usize, room: &mut Room) {
        let floor_room = &mut self.rooms[index];
        for tile in &floor_room.seal {
            room.set_cell(tile.cell.0, tile.cell.1, tile.before);
        }
        floor_room.sealed = false;
    }
}

/// Where a floor goes: on its own, or to the right of the hub, one empty block
/// beyond it, as Unity's scene keeps the hub apart from the dungeon.
pub struct Built {
    pub file: RoomFile,
    pub floor: Floor,
}

/// Joins the layout's rooms into one room, each at its place on the grid, after
/// `hub` if given. The world's spawn stays the hub's when there is one;
/// otherwise it is where entering the dungeon puts the player.
pub fn build(
    layout: &Layout,
    files: &HashMap<String, RoomFile>,
    settings: &DungeonFile,
    hub: Option<&RoomFile>,
) -> Result<Built> {
    let min_x = layout.rooms.iter().map(|room| room.coord.0).min().context("an empty layout")?;
    let min_y = layout.rooms.iter().map(|room| room.coord.1).min().context("an empty layout")?;
    let max_x = layout.rooms.iter().map(|room| room.coord.0).max().unwrap_or(min_x);
    let max_y = layout.rooms.iter().map(|room| room.coord.1).max().unwrap_or(min_y);
    // The floor's first block: past the hub and one empty block more.
    let start_block = match hub {
        Some(hub) => (hub.size[0] as usize).div_ceil(ROOM_CELLS[0]) as i32 + 1,
        None => 0,
    };
    let floor_columns = (max_x - min_x + 1) as usize * ROOM_CELLS[0];
    let floor_rows = (max_y - min_y + 1) as usize * ROOM_CELLS[1];
    let columns = start_block as usize * ROOM_CELLS[0] + floor_columns;
    let rows = floor_rows.max(hub.map_or(0, |hub| hub.size[1] as usize));

    let mut joined = Joined {
        file: RoomFile {
            name: "dungeon".to_string(),
            size: [columns as u32, rows as u32],
            cell: 0.5,
            palette: Vec::new(),
            layers: Vec::new(),
            decorations: Vec::new(),
            objects: Vec::new(),
            markers: Vec::new(),
            barriers: Vec::new(),
            doors: DoorsFile::default(),
        },
        palette_index: HashMap::new(),
        layer_index: HashMap::new(),
    };
    if let Some(hub) = hub {
        joined.add(hub, (0, 0), |layer| layer.enabled);
    }

    let mut floor = Floor {
        rooms: Vec::new(),
        start_x: start_block as f32 * ROOM_SIZE.x,
        entry: Vec2::ZERO,
        rooms_cleared: 0,
    };
    for room in &layout.rooms {
        let file = files.get(&room.name).with_context(|| format!("room `{}` was not loaded", room.name))?;
        let block = (start_block + room.coord.0 - min_x, room.coord.1 - min_y);
        let cells = (block.0 as usize * ROOM_CELLS[0], block.1 as usize * ROOM_CELLS[1]);
        if room.kind == RoomKind::Spawn {
            // `Player.TeleportDungeon`: 3 left of and 2 above the room's middle.
            floor.entry = Vec2::new(block.0 as f32, block.1 as f32) * ROOM_SIZE + ROOM_SIZE * 0.5 + Vec2::new(-3.0, 2.0);
        }
        joined.add(file, cells, |layer| layer_shown(layer, room));
        floor.rooms.push(FloorRoom {
            block,
            kind: room.kind,
            seal: seal_tiles(room, file, settings, cells),
            sealed: false,
            had_enemies: false,
            cleared: false,
        });
    }
    Ok(Built { file: joined.file, floor })
}

/// A room file being joined from others.
struct Joined {
    file: RoomFile,
    palette_index: HashMap<String, i32>,
    /// One joined layer per (sorting layer, order, ledge), in order of first sight.
    layer_index: HashMap<(String, i32, bool), usize>,
}

impl Joined {
    /// Copies `room` in with its bottom-left cell at `cells`, keeping the layers
    /// `shown` lets through.
    fn add(&mut self, room: &RoomFile, cells: (usize, usize), shown: impl Fn(&LayerFile) -> bool) {
        let [columns, rows] = [self.file.size[0] as usize, self.file.size[1] as usize];
        let offset = Vec2::new(cells.0 as f32, cells.1 as f32) * self.file.cell;
        let remap: Vec<i32> = room
            .palette
            .iter()
            .map(|name| {
                let next = self.palette_index.len() as i32;
                *self.palette_index.entry(name.clone()).or_insert_with(|| {
                    self.file.palette.push(name.clone());
                    next
                })
            })
            .collect();

        for layer in room.layers.iter().filter(|layer| shown(layer)) {
            let key = (layer.sorting_layer.clone(), layer.order, layer.ledge);
            let layers = &mut self.file.layers;
            let index = *self.layer_index.entry(key).or_insert_with(|| {
                layers.push(LayerFile {
                    name: layer.name.clone(),
                    sorting_layer: layer.sorting_layer.clone(),
                    order: layer.order,
                    enabled: true,
                    ledge: layer.ledge,
                    tiles: vec![vec![-1; columns]; rows],
                    solid: None,
                });
                layers.len() - 1
            });
            let merged = &mut self.file.layers[index];
            let room_rows = layer.tiles.len();
            for (row_from_top, row) in layer.tiles.iter().enumerate() {
                let floor_row = rows - 1 - (cells.1 + room_rows - 1 - row_from_top);
                for (x, &tile) in row.iter().enumerate() {
                    if tile >= 0 {
                        merged.tiles[floor_row][cells.0 + x] = remap[(tile / 8) as usize] * 8 + tile % 8;
                    }
                }
            }
            if let Some(solid) = &layer.solid {
                let merged_solid = merged.solid.get_or_insert_with(|| vec![".".repeat(columns); rows]);
                let solid_rows = solid.len();
                for (row_from_top, line) in solid.iter().enumerate() {
                    let floor_row = rows - 1 - (cells.1 + solid_rows - 1 - row_from_top);
                    let mut bytes = std::mem::take(&mut merged_solid[floor_row]).into_bytes();
                    for (x, symbol) in line.bytes().enumerate() {
                        if symbol == b'#' {
                            bytes[cells.0 + x] = b'#';
                        }
                    }
                    merged_solid[floor_row] = String::from_utf8(bytes).expect("ascii");
                }
            }
        }

        let moved = |position: [f32; 2]| (Vec2::from(position) + offset).to_array();
        for decoration in &room.decorations {
            let mut decoration = decoration.clone();
            decoration.position = moved(decoration.position);
            self.file.decorations.push(decoration);
        }
        for object in &room.objects {
            let mut object = object.clone();
            object.position = moved(object.position);
            self.file.objects.push(object);
        }
        for barrier in &room.barriers {
            let mut barrier = *barrier;
            barrier.center = moved(barrier.center);
            self.file.barriers.push(barrier);
        }
        for marker in &room.markers {
            let mut marker = marker.clone();
            marker.position = moved(marker.position);
            if let Some(bounds) = &mut marker.bounds {
                bounds.center = moved(bounds.center);
            }
            self.file.markers.push(marker);
        }
    }
}

/// What sealing a room puts in place. A picked room seals its doorways with
/// door tiles at the cells `RoomScript.SealRoom` names. The boss room is a
/// `SpawnRoom` whose sides are flipped on `Start` (`doorsInverseGenerated`), so
/// sealing it switches on the wall layers of the sides that lead somewhere,
/// shutting the player in with the boss. The spawn and exit rooms never seal:
/// they have no enemies.
fn seal_tiles(room: &PlannedRoom, file: &RoomFile, settings: &DungeonFile, cells: (usize, usize)) -> Vec<SealTile> {
    let base = (cells.0 as i32, cells.1 as i32);
    let mut tiles = Vec::new();
    match room.kind {
        RoomKind::Random => {
            let doors = file.doors;
            let tile = |x: i32, y: i32, door: &crate::data::DoorTileFile| SealTile {
                cell: (base.0 + x, base.1 + y),
                sprite: door.sprite.clone(),
                transform: 0,
                solid: door.solid,
                before: Cell::Free,
            };
            let door = &settings.door_tiles;
            if doors.top {
                tiles.extend([tile(15, 17, &door.top), tile(16, 17, &door.top)]);
                tiles.extend([tile(15, 16, &door.bottom), tile(16, 16, &door.bottom)]);
            }
            if doors.bottom {
                tiles.extend([tile(15, 0, &door.top), tile(16, 0, &door.top)]);
            }
            if doors.left {
                tiles.extend((8..=10).map(|y| tile(0, y, &door.vertical)));
            }
            if doors.right {
                tiles.extend((8..=10).map(|y| tile(31, y, &door.vertical)));
            }
        }
        RoomKind::Boss => {
            let [top, bottom, left, right] = room.closed;
            for layer in &file.layers {
                let open_side = match layer.name.as_str() {
                    "TopWall" => !top,
                    "BottomWall" => !bottom,
                    "LeftWall" => !left,
                    "RightWall" => !right,
                    _ => false,
                };
                if layer.enabled || !open_side {
                    continue;
                }
                let rows = layer.tiles.len();
                for (row_from_top, row) in layer.tiles.iter().enumerate() {
                    let y = (rows - 1 - row_from_top) as i32;
                    for (x, &value) in row.iter().enumerate() {
                        if value < 0 {
                            continue;
                        }
                        let solid = layer
                            .solid
                            .as_ref()
                            .and_then(|solid| solid.get(row_from_top))
                            .is_some_and(|line| line.as_bytes().get(x) == Some(&b'#'));
                        tiles.push(SealTile {
                            cell: (base.0 + x as i32, base.1 + y),
                            sprite: file.palette[(value / 8) as usize].clone(),
                            transform: (value % 8) as u8,
                            solid,
                            before: Cell::Free,
                        });
                    }
                }
            }
        }
        RoomKind::Spawn | RoomKind::Exit => {}
    }
    tiles
}

/// Whether a room's layer is drawn: enabled ones always, and the spawn, exit and
/// boss rooms' wall layers on the sides that lead nowhere (`SpawnRoom.SealRoom`).
fn layer_shown(layer: &LayerFile, room: &PlannedRoom) -> bool {
    if layer.enabled {
        return true;
    }
    if room.kind == RoomKind::Random {
        return false;
    }
    let [top, bottom, left, right] = room.closed;
    match layer.name.as_str() {
        "TopWall" => top,
        "BottomWall" => bottom,
        "LeftWall" => left,
        "RightWall" => right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> DungeonFile {
        serde_json::from_str(
            r#"{"grid_size": 10, "rooms": 8, "spawn": [5, 5], "open_room": "open", "spawn_room": "spawn",
                "door_tiles": {"top": {"sprite": "t", "solid": true}, "bottom": {"sprite": "b", "solid": false},
                               "vertical": {"sprite": "v", "solid": true}},
                "exit_room": "exit", "boss_room": "boss",
                "by_doors": {"N": ["n_far", "n"], "S": ["s"], "E": ["e"], "W": ["w"], "NS": ["ns"], "EW": ["ew"],
                             "NE": ["ne"], "NW": ["nw"], "SE": ["se"], "SW": ["sw"], "NSE": ["nse"],
                             "NSW": ["nsw"], "NEW": ["new"], "SEW": ["sew"], "NSEW": ["nsew"]},
                "distance": {"n_far": 3}}"#,
        )
        .unwrap()
    }

    #[test]
    fn a_floor_has_its_rooms_a_spawn_an_exit_and_a_boss_beside_it() {
        for seed in 1..50 {
            let layout = generate(&settings(), &mut Rng::new(seed));
            let count = |kind| layout.rooms.iter().filter(|room| room.kind == kind).count();
            assert_eq!((count(RoomKind::Spawn), count(RoomKind::Exit), count(RoomKind::Boss)), (1, 1, 1), "seed {seed}");
            assert!(layout.rooms.len() >= 3);
            let spawn = layout.room_at((5, 5)).unwrap();
            assert_eq!(spawn.kind, RoomKind::Spawn);
            let exit = layout.rooms.iter().find(|room| room.kind == RoomKind::Exit).unwrap();
            let boss = layout.rooms.iter().find(|room| room.kind == RoomKind::Boss).unwrap();
            assert_eq!((exit.coord.0 - boss.coord.0).abs() + (exit.coord.1 - boss.coord.1).abs(), 1);
        }
    }

    #[test]
    fn every_door_has_a_room_behind_it_and_every_room_is_reached() {
        for seed in 1..50 {
            let layout = generate(&settings(), &mut Rng::new(seed));
            for room in &layout.rooms {
                for (index, &(dx, dy)) in CARDINALS.iter().enumerate() {
                    let neighbour = layout.room_at((room.coord.0 + dx, room.coord.1 + dy));
                    if !room.closed[index] {
                        let neighbour = neighbour.expect("an open side leads to a room");
                        assert!(!neighbour.closed[opposite(index)], "doors meet, seed {seed}");
                    }
                }
            }
            // Walk the open sides from the spawn: every room is reached.
            let mut seen = vec![(5, 5)];
            let mut index = 0;
            while index < seen.len() {
                let room = layout.room_at(seen[index]).unwrap();
                for (side, &(dx, dy)) in CARDINALS.iter().enumerate() {
                    let next = (room.coord.0 + dx, room.coord.1 + dy);
                    if !room.closed[side] && !seen.contains(&next) {
                        seen.push(next);
                    }
                }
                index += 1;
            }
            assert_eq!(seen.len(), layout.rooms.len(), "seed {seed}");
        }
    }

    #[test]
    fn rooms_are_picked_by_their_doors_and_distance() {
        let file = settings();
        let near = PlannedRoom { coord: (0, 0), kind: RoomKind::Random, closed: [false, true, true, true], distance: 1, name: String::new() };
        assert_eq!(pick_room(&file, &near), "n", "the far room needs layer 3");
        let far = PlannedRoom { distance: 3, ..near.clone() };
        assert_eq!(pick_room(&file, &far), "n_far");
        let ew = PlannedRoom { closed: [true, true, false, false], ..near };
        assert_eq!(pick_room(&file, &ew), "ew");
    }
}
