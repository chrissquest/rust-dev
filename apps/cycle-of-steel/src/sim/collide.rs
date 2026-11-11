//! Moving a box through a room's solid cells.
//!
//! Movement is resolved one axis at a time: move along x and stop at the first
//! solid cell, then the same along y. That gives sliding along walls for free. A
//! single call must not move further than one cell, or the box could step over a
//! wall; everything that walks is far slower than that at 60 steps per second.
//! Fast things (bullets) will use a ray walk over the same grid instead.

use crystal_engine::glam::Vec2;

use super::room::Room;

/// The result of [`move_box`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moved {
    pub position: Vec2,
    /// Whether a wall stopped the movement along each axis.
    pub hit_x: bool,
    pub hit_y: bool,
}

/// Moves a box (centre `position`, half-size `half`) by `delta`, stopping at walls.
pub fn move_box(room: &Room, position: Vec2, half: Vec2, delta: Vec2) -> Moved {
    debug_assert!(
        delta.x.abs() <= room.cell && delta.y.abs() <= room.cell,
        "delta {delta} exceeds one cell"
    );

    let mut position = position;
    let hit_x = move_axis(room, &mut position, half, delta.x, Axis::X);
    let hit_y = move_axis(room, &mut position, half, delta.y, Axis::Y);
    Moved {
        position,
        hit_x,
        hit_y,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    X,
    Y,
}

fn move_axis(room: &Room, position: &mut Vec2, half: Vec2, amount: f32, axis: Axis) -> bool {
    if amount == 0.0 {
        return false;
    }

    let mut target = *position;
    match axis {
        Axis::X => target.x += amount,
        Axis::Y => target.y += amount,
    }

    let cell = room.cell;
    let ((min_x, min_y), (max_x, max_y)) = room.cell_range(target, half);
    let mut hit = false;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if !room.blocks_walker(x, y) {
                continue;
            }
            hit = true;
            // Pull back to rest against the near face of this cell.
            match axis {
                Axis::X if amount > 0.0 => target.x = target.x.min(x as f32 * cell - half.x),
                Axis::X => target.x = target.x.max((x + 1) as f32 * cell + half.x),
                Axis::Y if amount > 0.0 => target.y = target.y.min(y as f32 * cell - half.y),
                Axis::Y => target.y = target.y.max((y + 1) as f32 * cell + half.y),
            }
        }
    }

    *position = target;
    hit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::room::CELL;

    const HALF: Vec2 = Vec2::new(0.1, 0.2);

    #[test]
    fn free_movement_is_not_changed() {
        let room = Room::test_room();
        let moved = move_box(&room, Vec2::new(8.0, 3.0), HALF, Vec2::new(0.1, -0.05));
        assert_eq!(moved.position, Vec2::new(8.1, 2.95));
        assert!(!moved.hit_x && !moved.hit_y);
    }

    #[test]
    fn a_wall_stops_one_axis_and_the_other_slides() {
        let room = Room::test_room();
        // Just right of the left wall, pushing into it and upwards.
        let start = Vec2::new(CELL + HALF.x + 0.01, 3.0);
        let moved = move_box(&room, start, HALF, Vec2::new(-0.2, 0.1));
        assert!(moved.hit_x && !moved.hit_y);
        assert!((moved.position.x - (CELL + HALF.x)).abs() < 1e-6);
        assert!((moved.position.y - 3.1).abs() < 1e-6);
    }

    #[test]
    fn stops_under_a_ceiling() {
        let room = Room::test_room();
        let top_inner = room.size().y - CELL;
        let start = Vec2::new(8.0, top_inner - HALF.y - 0.01);
        let moved = move_box(&room, start, HALF, Vec2::new(0.0, 0.3));
        assert!(moved.hit_y);
        assert!((moved.position.y - (top_inner - HALF.y)).abs() < 1e-6);
    }
}
