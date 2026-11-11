//! A room: tile layers to draw and a grid of solid cells to collide with.
//!
//! Rooms are 16 by 9 world units on a half-unit grid, the same as the Unity project
//! (`RoomInfo.width/height`, tilemap cell size 0.5). The room's bottom-left corner
//! is the world origin for now; the dungeon will place rooms side by side later.
//!
//! The only room so far is built in code as a stand-in. Real rooms arrive with the
//! converter, which is also when the room file format gets defined.

use crystal_engine::glam::Vec2;
use crystal_engine::tilemap::TileLayer;

/// World size of one tile.
pub const CELL: f32 = 0.5;
/// Room size in cells.
pub const COLUMNS: u32 = 32;
pub const ROWS: u32 = 18;
/// Room size in world units.
pub const WIDTH: f32 = COLUMNS as f32 * CELL;
pub const HEIGHT: f32 = ROWS as f32 * CELL;

/// Cells of `main_dark_compound.png` (8 columns of 32 px) used by the test room.
const FLOOR_TILE: u16 = 42;
const WALL_TILE: u16 = 14;

pub struct Layer {
    pub name: &'static str,
    pub tiles: TileLayer,
}

pub struct Room {
    /// Drawn in order, first at the bottom.
    pub layers: Vec<Layer>,
    /// `COLUMNS * ROWS` flags, bottom row first: `true` blocks movement.
    solid: Vec<bool>,
    /// Where the player starts, in world units.
    pub spawn: Vec2,
}

impl Room {
    /// A walled room with a few pillars, for exercising movement and collision.
    pub fn test_room() -> Self {
        // Top row first, as it appears on screen. '#' wall, '.' floor.
        const MAP: [&str; ROWS as usize] = [
            "################################",
            "#..............................#",
            "#..............................#",
            "#....##..................##....#",
            "#....##..................##....#",
            "#..............................#",
            "#..............................#",
            "#.............####.............#",
            "#..............................#",
            "#..............................#",
            "#.............####.............#",
            "#..............................#",
            "#..............................#",
            "#....##..................##....#",
            "#....##..................##....#",
            "#..............................#",
            "#..............................#",
            "################################",
        ];

        let mut floor = TileLayer::empty(COLUMNS, ROWS);
        let mut walls = TileLayer::empty(COLUMNS, ROWS);
        let mut solid = vec![false; (COLUMNS * ROWS) as usize];

        for (row_from_top, line) in MAP.iter().enumerate() {
            let y = ROWS as i32 - 1 - row_from_top as i32;
            for (x, symbol) in line.bytes().enumerate() {
                let x = x as i32;
                floor.set(x, y, FLOOR_TILE);
                if symbol == b'#' {
                    walls.set(x, y, WALL_TILE);
                    solid[(y as u32 * COLUMNS) as usize + x as usize] = true;
                }
            }
        }

        Self {
            layers: vec![
                Layer { name: "Background", tiles: floor },
                Layer { name: "Walls", tiles: walls },
            ],
            solid,
            spawn: Vec2::new(WIDTH * 0.5, HEIGHT * 0.5),
        }
    }

    /// Whether the cell at column `x`, row `y` (from the bottom) blocks movement.
    /// Everything outside the room does.
    pub fn is_solid(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= COLUMNS as i32 || y >= ROWS as i32 {
            return true;
        }
        self.solid[(y as u32 * COLUMNS) as usize + x as usize]
    }

    /// Whether a box centred on `center` overlaps any solid cell.
    pub fn box_hits_solid(&self, center: Vec2, half: Vec2) -> bool {
        let (min, max) = cell_range(center, half);
        (min.1..=max.1).any(|y| (min.0..=max.0).any(|x| self.is_solid(x, y)))
    }
}

/// The inclusive range of cells a box touches, as `((min_x, min_y), (max_x, max_y))`.
/// A box edge lying exactly on a cell border does not count as touching the far cell.
pub(crate) fn cell_range(center: Vec2, half: Vec2) -> ((i32, i32), (i32, i32)) {
    const EDGE: f32 = 1e-4;
    let low = (center - half + EDGE) / CELL;
    let high = (center + half - EDGE) / CELL;
    (
        (low.x.floor() as i32, low.y.floor() as i32),
        (high.x.floor() as i32, high.y.floor() as i32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_is_walled_and_spawn_is_free() {
        let room = Room::test_room();
        assert!(room.is_solid(0, 0));
        assert!(room.is_solid(COLUMNS as i32 - 1, ROWS as i32 - 1));
        assert!(!room.is_solid(1, 1));
        assert!(room.is_solid(-1, 5), "outside the room is solid");
        assert!(!room.box_hits_solid(room.spawn, Vec2::splat(0.25)));
    }

    #[test]
    fn a_box_flush_against_a_wall_does_not_hit_it() {
        let room = Room::test_room();
        let half = Vec2::splat(0.2);
        // Touching the left wall's inner face (x = CELL) exactly.
        assert!(!room.box_hits_solid(Vec2::new(CELL + half.x, 2.0), half));
        assert!(room.box_hits_solid(Vec2::new(CELL + half.x - 0.01, 2.0), half));
    }
}
