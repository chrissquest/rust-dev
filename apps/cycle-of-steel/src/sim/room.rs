//! A room: a grid of cells that block movement, and the data to draw it.
//!
//! A dungeon room is 16 by 9 world units on a half-unit grid, the same as the
//! Unity project (`RoomInfo.width/height`, tilemap cell size 0.5); the hub is
//! larger. The room's bottom-left corner is the world origin for now; the dungeon
//! will place rooms side by side later.

use crystal_engine::glam::Vec2;

use crate::data::RoomFile;

/// World size of one tile in every room so far. A room file states its own; this
/// is what the stand-in test room uses.
pub const CELL: f32 = 0.5;

/// What stands in a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Free,
    /// Blocks walkers; bullets and flyers pass over.
    Ledge,
    /// Blocks everything.
    Wall,
}

pub struct Room {
    pub columns: u32,
    pub rows: u32,
    /// World size of one cell.
    pub cell: f32,
    /// `columns * rows` cells, bottom row first.
    cells: Vec<Cell>,
    /// Where the player starts, in world units.
    pub spawn: Vec2,
    /// The file this room came from, for drawing. `None` for the test room.
    pub file: Option<RoomFile>,
}

impl Room {
    /// Builds the collision grid from a room file's enabled layers.
    pub fn from_file(file: RoomFile) -> Self {
        let [columns, rows] = file.size;
        let mut cells = vec![Cell::Free; (columns * rows) as usize];

        for layer in file.layers.iter().filter(|layer| layer.enabled) {
            let Some(solid) = &layer.solid else {
                continue;
            };
            for (row_from_top, line) in solid.iter().enumerate() {
                let y = rows as usize - 1 - row_from_top;
                for (x, symbol) in line.bytes().enumerate() {
                    if symbol != b'#' {
                        continue;
                    }
                    let cell = &mut cells[y * columns as usize + x];
                    // A wall on any layer wins over a ledge on another.
                    *cell = match (*cell, layer.ledge) {
                        (Cell::Wall, _) | (_, false) => Cell::Wall,
                        (_, true) => Cell::Ledge,
                    };
                }
            }
        }

        let mut room = Self {
            columns,
            rows,
            cell: file.cell,
            cells,
            spawn: Vec2::ZERO,
            file: None,
        };
        room.spawn = room.pick_spawn(&file);
        room.file = Some(file);
        room
    }

    /// The hub marks where the player appears; elsewhere it is the middle of the
    /// room. Either way the point is nudged to the nearest open floor.
    fn pick_spawn(&self, file: &RoomFile) -> Vec2 {
        let wanted = file
            .markers
            .iter()
            .find(|marker| marker.scripts.iter().any(|script| script == "HubRespawn"))
            // The marker is the rack on the wall; the player steps out below it.
            .map(|marker| Vec2::from(marker.position) - Vec2::new(0.0, 1.0))
            .unwrap_or(Vec2::new(16.0, 9.0) * 0.5);
        self.nearest_open(wanted)
    }

    /// The centre of the walkable cell nearest to `point` that also has walkable
    /// cells all round it, so a character placed there is not touching a wall.
    fn nearest_open(&self, point: Vec2) -> Vec2 {
        let is_open = |x: i32, y: i32| {
            (-1..=1).all(|dy| (-1..=1).all(|dx| self.cell_at(x + dx, y + dy) == Cell::Free))
        };
        let centre = |x: i32, y: i32| Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * self.cell;

        let mut best: Option<(f32, Vec2)> = None;
        for y in 0..self.rows as i32 {
            for x in 0..self.columns as i32 {
                if !is_open(x, y) {
                    continue;
                }
                let candidate = centre(x, y);
                let distance = candidate.distance_squared(point);
                if best.is_none_or(|(nearest, _)| distance < nearest) {
                    best = Some((distance, candidate));
                }
            }
        }
        best.map(|(_, position)| position).unwrap_or(point)
    }

    /// A walled room with a few pillars, for exercising movement and collision.
    /// It has no file, so it cannot be drawn.
    pub fn test_room() -> Self {
        // Top row first, as it appears on screen. '#' wall, '.' floor.
        const MAP: [&str; 18] = [
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
        let (columns, rows) = (MAP[0].len() as u32, MAP.len() as u32);

        let mut cells = vec![Cell::Free; (columns * rows) as usize];
        for (row_from_top, line) in MAP.iter().enumerate() {
            let y = rows as usize - 1 - row_from_top;
            for (x, symbol) in line.bytes().enumerate() {
                if symbol == b'#' {
                    cells[y * columns as usize + x] = Cell::Wall;
                }
            }
        }

        Self {
            columns,
            rows,
            cell: CELL,
            cells,
            spawn: Vec2::new(columns as f32, rows as f32) * CELL * 0.5,
            file: None,
        }
    }

    /// The room's size in world units.
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.columns as f32, self.rows as f32) * self.cell
    }

    /// What is at column `x`, row `y` (from the bottom). Outside the room is wall.
    pub fn cell_at(&self, x: i32, y: i32) -> Cell {
        if x < 0 || y < 0 || x >= self.columns as i32 || y >= self.rows as i32 {
            return Cell::Wall;
        }
        self.cells[(y as u32 * self.columns) as usize + x as usize]
    }

    /// Whether something walking is blocked by the cell at column `x`, row `y`.
    pub fn blocks_walker(&self, x: i32, y: i32) -> bool {
        self.cell_at(x, y) != Cell::Free
    }

    /// Whether a walker's box centred on `center` overlaps any blocking cell.
    pub fn box_hits_solid(&self, center: Vec2, half: Vec2) -> bool {
        let (min, max) = self.cell_range(center, half);
        (min.1..=max.1).any(|y| (min.0..=max.0).any(|x| self.blocks_walker(x, y)))
    }

    /// How far a ray from `origin` along the unit vector `direction` travels before
    /// it enters a wall, if it does within `max_distance`. Ledges do not stop it.
    /// A ray that starts inside a wall stops at once.
    pub fn ray_hits_wall(&self, origin: Vec2, direction: Vec2, max_distance: f32) -> Option<f32> {
        // Walk the grid one cell border at a time: `next` holds how far along the
        // ray the next border is on each axis, `per_cell` how far apart borders are.
        let scaled = origin / self.cell;
        let mut cell = (scaled.x.floor() as i32, scaled.y.floor() as i32);
        let step = (direction.x.signum() as i32, direction.y.signum() as i32);
        let per_cell = Vec2::new(self.cell / direction.x.abs(), self.cell / direction.y.abs());
        let to_border = |position: f32, direction: f32| {
            let inside = position - position.floor();
            if direction > 0.0 { 1.0 - inside } else { inside }
        };
        let mut next = Vec2::new(
            if direction.x == 0.0 { f32::INFINITY } else { to_border(scaled.x, direction.x) * per_cell.x },
            if direction.y == 0.0 { f32::INFINITY } else { to_border(scaled.y, direction.y) * per_cell.y },
        );
        let mut distance = 0.0;
        while distance <= max_distance {
            if self.cell_at(cell.0, cell.1) == Cell::Wall {
                return Some(distance);
            }
            if next.x < next.y {
                distance = next.x;
                next.x += per_cell.x;
                cell.0 += step.0;
            } else {
                distance = next.y;
                next.y += per_cell.y;
                cell.1 += step.1;
            }
        }
        None
    }

    /// The inclusive range of cells a box touches, as `((min_x, min_y), (max_x, max_y))`.
    /// A box edge lying exactly on a cell border does not count as touching the far cell.
    pub(crate) fn cell_range(&self, center: Vec2, half: Vec2) -> ((i32, i32), (i32, i32)) {
        const EDGE: f32 = 1e-4;
        let low = (center - half + EDGE) / self.cell;
        let high = (center + half - EDGE) / self.cell;
        (
            (low.x.floor() as i32, low.y.floor() as i32),
            (high.x.floor() as i32, high.y.floor() as i32),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_is_walled_and_spawn_is_free() {
        let room = Room::test_room();
        assert_eq!(room.cell_at(0, 0), Cell::Wall);
        assert_eq!(room.cell_at(room.columns as i32 - 1, room.rows as i32 - 1), Cell::Wall);
        assert_eq!(room.cell_at(1, 1), Cell::Free);
        assert_eq!(room.cell_at(-1, 5), Cell::Wall, "outside the room is wall");
        assert!(!room.box_hits_solid(room.spawn, Vec2::splat(0.25)));
        assert_eq!(room.size(), Vec2::new(16.0, 9.0));
    }

    #[test]
    fn a_box_flush_against_a_wall_does_not_hit_it() {
        let room = Room::test_room();
        let half = Vec2::splat(0.2);
        // Touching the left wall's inner face (x = CELL) exactly.
        assert!(!room.box_hits_solid(Vec2::new(CELL + half.x, 2.0), half));
        assert!(room.box_hits_solid(Vec2::new(CELL + half.x - 0.01, 2.0), half));
    }

    fn file(layers: &str, markers: &str) -> RoomFile {
        serde_json::from_str(&format!(
            r#"{{"name": "t", "size": [4, 3], "cell": 0.5, "palette": [],
                "layers": [{layers}], "markers": [{markers}]}}"#
        ))
        .unwrap()
    }

    fn layer(enabled: bool, ledge: bool, solid: [&str; 3]) -> String {
        format!(
            r#"{{"name": "l", "sorting_layer": "Default", "order": 0, "enabled": {enabled}, "ledge": {ledge},
                "tiles": [[-1,-1,-1,-1],[-1,-1,-1,-1],[-1,-1,-1,-1]],
                "solid": ["{}", "{}", "{}"]}}"#,
            solid[0], solid[1], solid[2]
        )
    }

    #[test]
    fn collision_comes_from_enabled_layers_with_walls_over_ledges() {
        let layers = [
            layer(true, false, ["#...", "....", "...."]),
            layer(true, true, ["##..", "....", "...."]),
            layer(false, false, ["....", "....", "####"]),
        ]
        .join(",");
        let room = Room::from_file(file(&layers, ""));
        // Rows are written top first: the top row is y = 2.
        assert_eq!(room.cell_at(0, 2), Cell::Wall, "wall and ledge together is a wall");
        assert_eq!(room.cell_at(1, 2), Cell::Ledge);
        assert_eq!(room.cell_at(2, 2), Cell::Free);
        assert_eq!(room.cell_at(0, 0), Cell::Free, "a switched-off layer does not collide");
        assert!(room.blocks_walker(1, 2));
    }

    #[test]
    fn spawn_is_moved_off_walls_onto_open_floor() {
        // In an empty 4x3 room only (1, 1) and (2, 1) have open floor all round,
        // because everything outside the room is wall. The default spawn, the
        // middle of a 16x9 room, is far to the upper right: (2, 1) is nearest.
        let layers = layer(true, false, ["....", "....", "...."]);
        let room = Room::from_file(file(&layers, ""));
        assert_eq!(room.spawn, Vec2::new(1.25, 0.75));
        assert!(!room.box_hits_solid(room.spawn, Vec2::splat(0.2)));
    }
    #[test]
    fn a_ray_stops_at_the_first_wall() {
        let room = Room::test_room();
        // From the middle of the room the right-hand wall starts at x = 15.5.
        let hit = room.ray_hits_wall(Vec2::new(8.0, 4.4), Vec2::X, 20.0).unwrap();
        assert!((hit - 7.5).abs() < 1e-4, "{hit}");
        // Straight up from there is the block in the middle of the room, at y = 5.
        let hit = room.ray_hits_wall(Vec2::new(8.0, 4.4), Vec2::Y, 20.0).unwrap();
        assert!((hit - 0.6).abs() < 1e-4, "{hit}");
        // Diagonally down-left from further left it slips past the corner block
        // and reaches the left-hand wall (x = 0.5).
        let direction = Vec2::new(-1.0, -1.0).normalize();
        let hit = room.ray_hits_wall(Vec2::new(4.0, 4.4), direction, 20.0).unwrap();
        assert!((hit - 3.5 * 2f32.sqrt()).abs() < 1e-3, "{hit}");
    }

    #[test]
    fn a_ray_gives_up_at_its_range_and_stops_at_once_inside_a_wall() {
        let room = Room::test_room();
        assert_eq!(room.ray_hits_wall(Vec2::new(8.0, 4.4), Vec2::X, 5.0), None);
        assert_eq!(room.ray_hits_wall(Vec2::new(0.2, 0.2), Vec2::X, 5.0), Some(0.0));
    }

    #[test]
    fn a_ray_passes_over_ledges() {
        let mut room = Room::test_room();
        let ledge = (20, 8);
        room.cells[(ledge.1 * room.columns + ledge.0) as usize] = Cell::Ledge;
        let hit = room.ray_hits_wall(Vec2::new(8.0, 4.4), Vec2::X, 20.0).unwrap();
        assert!((hit - 7.5).abs() < 1e-4, "{hit}");
    }
}
