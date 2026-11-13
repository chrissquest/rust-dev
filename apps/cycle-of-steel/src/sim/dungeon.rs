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
use crate::data::{DungeonFile, LayerFile, RoomFile};

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

/// Joins the layout's rooms into one room, each at its place on the grid, and
/// says where the player starts: the middle of the spawn room.
pub fn build(layout: &Layout, files: &HashMap<String, RoomFile>) -> Result<(RoomFile, Vec2)> {
    let min_x = layout.rooms.iter().map(|room| room.coord.0).min().context("an empty layout")?;
    let min_y = layout.rooms.iter().map(|room| room.coord.1).min().context("an empty layout")?;
    let max_x = layout.rooms.iter().map(|room| room.coord.0).max().unwrap_or(min_x);
    let max_y = layout.rooms.iter().map(|room| room.coord.1).max().unwrap_or(min_y);
    let columns = (max_x - min_x + 1) as usize * ROOM_CELLS[0];
    let rows = (max_y - min_y + 1) as usize * ROOM_CELLS[1];

    let mut floor = RoomFile {
        name: "dungeon".to_string(),
        size: [columns as u32, rows as u32],
        cell: 0.5,
        palette: Vec::new(),
        layers: Vec::new(),
        decorations: Vec::new(),
        objects: Vec::new(),
        markers: Vec::new(),
    };
    let mut palette_index: HashMap<String, i32> = HashMap::new();
    // One floor layer per (sorting layer, order, ledge), in order of first sight.
    let mut layer_index: HashMap<(String, i32, bool), usize> = HashMap::new();
    let mut spawn = Vec2::ZERO;

    for room in &layout.rooms {
        let file = files.get(&room.name).with_context(|| format!("room `{}` was not loaded", room.name))?;
        let cells = (
            (room.coord.0 - min_x) as usize * ROOM_CELLS[0],
            (room.coord.1 - min_y) as usize * ROOM_CELLS[1],
        );
        let offset = Vec2::new(cells.0 as f32, cells.1 as f32) * floor.cell;
        if room.kind == RoomKind::Spawn {
            spawn = offset + ROOM_SIZE * 0.5;
        }
        let remap: Vec<i32> = file
            .palette
            .iter()
            .map(|name| {
                let next = palette_index.len() as i32;
                *palette_index.entry(name.clone()).or_insert_with(|| {
                    floor.palette.push(name.clone());
                    next
                })
            })
            .collect();

        for layer in &file.layers {
            if !layer_shown(layer, room) {
                continue;
            }
            let key = (layer.sorting_layer.clone(), layer.order, layer.ledge);
            let index = *layer_index.entry(key).or_insert_with(|| {
                floor.layers.push(LayerFile {
                    name: layer.name.clone(),
                    sorting_layer: layer.sorting_layer.clone(),
                    order: layer.order,
                    enabled: true,
                    ledge: layer.ledge,
                    tiles: vec![vec![-1; columns]; rows],
                    solid: None,
                });
                floor.layers.len() - 1
            });
            let merged = &mut floor.layers[index];
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
        for decoration in &file.decorations {
            let mut decoration = decoration.clone();
            decoration.position = moved(decoration.position);
            floor.decorations.push(decoration);
        }
        for object in &file.objects {
            let mut object = object.clone();
            object.position = moved(object.position);
            floor.objects.push(object);
        }
        for marker in &file.markers {
            let mut marker = marker.clone();
            marker.position = moved(marker.position);
            if let Some(bounds) = &mut marker.bounds {
                bounds.center = moved(bounds.center);
            }
            floor.markers.push(marker);
        }
    }
    Ok((floor, spawn))
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
