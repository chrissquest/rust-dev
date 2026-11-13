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

/// Moves a box (centre `position`, half-size `half`) by `delta`, stopping at walls
/// and at `solids`, boxes given as (centre, half-size). A solid the box already
/// overlaps does not hold it, so nothing can get stuck inside one.
pub fn move_box(
    room: &Room,
    solids: impl Iterator<Item = (Vec2, Vec2)> + Clone,
    position: Vec2,
    half: Vec2,
    delta: Vec2,
) -> Moved {
    debug_assert!(
        delta.x.abs() <= room.cell && delta.y.abs() <= room.cell,
        "delta {delta} exceeds one cell"
    );

    let mut position = position;
    let hit_x = move_axis(room, solids.clone(), &mut position, half, delta.x, Axis::X);
    let hit_y = move_axis(room, solids, &mut position, half, delta.y, Axis::Y);
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

fn move_axis(
    room: &Room,
    solids: impl Iterator<Item = (Vec2, Vec2)>,
    position: &mut Vec2,
    half: Vec2,
    amount: f32,
    axis: Axis,
) -> bool {
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

    let index = if axis == Axis::X { 0 } else { 1 };
    for (center, solid_half) in solids {
        let reach = half + solid_half;
        let gap = target - center;
        let before = *position - center;
        let overlapping = gap.x.abs() < reach.x && gap.y.abs() < reach.y;
        // Only a solid the box was clear of along this axis can stop it.
        if !overlapping || before[index].abs() < reach[index] {
            continue;
        }
        hit = true;
        target[index] = if amount > 0.0 { center[index] - reach[index] } else { center[index] + reach[index] };
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
        let moved = move_box(&room, std::iter::empty(), Vec2::new(8.0, 3.0), HALF, Vec2::new(0.1, -0.05));
        assert_eq!(moved.position, Vec2::new(8.1, 2.95));
        assert!(!moved.hit_x && !moved.hit_y);
    }

    #[test]
    fn a_wall_stops_one_axis_and_the_other_slides() {
        let room = Room::test_room();
        // Just right of the left wall, pushing into it and upwards.
        let start = Vec2::new(CELL + HALF.x + 0.01, 3.0);
        let moved = move_box(&room, std::iter::empty(), start, HALF, Vec2::new(-0.2, 0.1));
        assert!(moved.hit_x && !moved.hit_y);
        assert!((moved.position.x - (CELL + HALF.x)).abs() < 1e-6);
        assert!((moved.position.y - 3.1).abs() < 1e-6);
    }

    #[test]
    fn stops_under_a_ceiling() {
        let room = Room::test_room();
        let top_inner = room.size().y - CELL;
        let start = Vec2::new(8.0, top_inner - HALF.y - 0.01);
        let moved = move_box(&room, std::iter::empty(), start, HALF, Vec2::new(0.0, 0.3));
        assert!(moved.hit_y);
        assert!((moved.position.y - (top_inner - HALF.y)).abs() < 1e-6);
    }

    #[test]
    fn a_solid_box_stops_one_axis_and_the_other_slides() {
        let room = Room::test_room();
        let chest = (Vec2::new(8.0, 3.0), Vec2::new(0.25, 0.14));
        let start = Vec2::new(8.0 - 0.25 - HALF.x - 0.01, 3.0);
        let moved = move_box(&room, std::iter::once(chest), start, HALF, Vec2::new(0.05, 0.02));
        assert!(moved.hit_x && !moved.hit_y);
        assert!((moved.position.x - (8.0 - 0.25 - HALF.x)).abs() < 1e-6);
        assert!((moved.position.y - 3.02).abs() < 1e-6);
    }

    #[test]
    fn a_box_already_inside_a_solid_can_walk_out() {
        let room = Room::test_room();
        let chest = (Vec2::new(8.0, 3.0), Vec2::new(0.25, 0.14));
        let moved = move_box(&room, std::iter::once(chest), Vec2::new(8.0, 3.0), HALF, Vec2::new(0.05, 0.0));
        assert!(!moved.hit_x);
        assert_eq!(moved.position, Vec2::new(8.05, 3.0));
    }
}
