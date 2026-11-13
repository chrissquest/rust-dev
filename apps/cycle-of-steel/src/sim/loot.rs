//! What a chest leaves when it breaks, and the drops that fly to the player.
//!
//! In Unity a chest is a `Killable` whose `lootPool` is rolled in `Killable.Die`.
//! Its lines name `Spawner` prefabs, the loot pools, which roll again on start
//! and spawn guns or drops. Drops (`DataDrop`, `HealDrop`) are pulled towards the
//! player once they are near and are collected on touching them.

use crystal_engine::glam::Vec2;

use super::DT;
use super::combat::{Pickup, UNITY_STEP, circle_overlaps_box, drag_kept};
use crate::data::DropKind;
use super::defs::{ChestId, Defs, DropId, EnemyId, Loot, LootEntry, PoolDef};
use super::guns::Gun;
use super::rng::Rng;

/// Room for this many drops before the oldest makes way.
pub const DROP_CAPACITY: usize = 128;
/// Room for this many guns on the floor before the list would grow.
pub const PICKUP_CAPACITY: usize = 64;

/// `DataDrop.Update` and `HealDrop.Update` push with `pickupSpeed` every rendered
/// frame, without scaling by the frame time, so Unity's pull depends on the frame
/// rate. As for damage numbers, one Unity frame is taken to be 1/60 s.
const UNITY_FRAME_RATE: f32 = 60.0;

/// A data cell or a health pack lying in the room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drop {
    pub def: DropId,
    /// What it is worth: its definition's `value`, unless the room overrides it.
    pub value: u32,
    pub position: Vec2,
    /// Position one step ago, for the view to blend from.
    pub previous_position: Vec2,
    pub velocity: Vec2,
}

impl Drop {
    pub fn new(def: DropId, value: u32, position: Vec2) -> Self {
        Self { def, value, position, previous_position: position, velocity: Vec2::ZERO }
    }
}

/// What a broken chest or a dead enemy leaves on the floor (`Killable.deathObject`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Remains {
    pub kind: RemainsKind,
    pub position: Vec2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RemainsKind {
    Chest(ChestId),
    /// An enemy's body: which of its `dead_sprites` (`RandomBodyScript`).
    Body(EnemyId, u8),
}

/// Where spawned loot goes.
pub struct Spawned<'a> {
    pub pickups: &'a mut Vec<Pickup>,
    pub drops: &'a mut Vec<Drop>,
}

/// Rolls a chest's loot at `position`, as `Killable.Die` does: each line spawns
/// when its weight is at least a roll of 0 to 100, so a weight of 100 always does.
pub fn spill(loot: &[LootEntry], defs: &Defs, position: Vec2, rng: &mut Rng, out: &mut Spawned) {
    for entry in loot {
        if entry.weight >= rng.range(0.0, 100.0) {
            spawn(entry.loot, defs, position, rng, out);
        }
    }
}

fn spawn(loot: Loot, defs: &Defs, position: Vec2, rng: &mut Rng, out: &mut Spawned) {
    match loot {
        Loot::Gun(gun) => out.pickups.push(Pickup { gun: Gun::new(gun), position }),
        Loot::Drop(drop) => {
            if out.drops.len() == out.drops.capacity() {
                out.drops.remove(0);
            }
            out.drops.push(Drop::new(drop, defs.drop_def(drop).value, position));
        }
        Loot::Pool(pool) => roll_pool(defs.pool(pool), defs, position, rng, out),
    }
}

/// `Spawner.Start`. With `multiple`, every line spawns when its weight beats its
/// own roll. Otherwise the lines are tried in order, again and again, until one
/// does; only that one spawns, so earlier lines are likelier than their weight
/// says. A pool whose weights are all zero would make Unity loop forever; here it
/// spawns nothing.
fn roll_pool(pool: &PoolDef, defs: &Defs, position: Vec2, rng: &mut Rng, out: &mut Spawned) {
    if pool.multiple {
        for entry in &pool.entries {
            if entry.weight > rng.range(0.0, 100.0) {
                spawn(entry.loot, defs, position, rng, out);
            }
        }
        return;
    }
    if !pool.entries.iter().any(|entry| entry.weight > 0.0) {
        return;
    }
    loop {
        for entry in &pool.entries {
            if entry.weight > rng.range(0.0, 100.0) {
                spawn(entry.loot, defs, position, rng, out);
                return;
            }
        }
    }
}

/// Pulls drops within reach towards the player and hands the kind and value of
/// each one the player touches to `collect`, removing it.
///
/// Unity measures reach between the two objects' positions and collects on its
/// trigger circle touching the player's collider. Drops have only that trigger,
/// so walls do not stop them.
pub fn step_drops(
    drops: &mut Vec<Drop>,
    defs: &Defs,
    player: Vec2,
    player_half: Vec2,
    mut collect: impl FnMut(DropKind, u32),
) {
    drops.retain_mut(|drop| {
        let def = defs.drop_def(drop.def);
        drop.previous_position = drop.position;
        let toward = player - drop.position;
        if toward.length() < def.pickup_range {
            drop.velocity += toward.normalize_or_zero() * pull_acceleration(def.pickup_speed) * DT;
        }
        drop.velocity *= drag_kept(def.drag);
        drop.position += drop.velocity * DT;

        let touching = circle_overlaps_box(drop.position + Vec2::from(def.offset), def.radius, player, player_half);
        if touching {
            collect(def.kind, drop.value);
        }
        !touching
    });
}

/// The acceleration, in units/s^2, of a drop pulled with `pickup_speed`: a force
/// of that much each frame, [`UNITY_FRAME_RATE`] frames a second, on a body of
/// mass 1, integrated over Unity's physics steps. 7 gives 8.4 units/s^2, which
/// against the drops' drag of 3 tops out near 2.8 units/s, faster than the player
/// runs.
fn pull_acceleration(pickup_speed: f32) -> f32 {
    pickup_speed * UNITY_FRAME_RATE * UNITY_STEP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DropKind;
    use crate::sim::defs::tests::test_defs;

    fn spill_chest(seed: u32) -> (Vec<Pickup>, Vec<Drop>) {
        let defs = test_defs();
        let chest = defs.chest(defs.chest_named("chest").unwrap());
        let (mut pickups, mut drops) = (Vec::new(), Vec::with_capacity(DROP_CAPACITY));
        let mut rng = Rng::new(seed);
        spill(&chest.loot, &defs, Vec2::new(3.0, 2.0), &mut rng, &mut Spawned { pickups: &mut pickups, drops: &mut drops });
        (pickups, drops)
    }

    #[test]
    fn a_chest_leaves_exactly_one_gun_and_what_its_other_pool_rolls() {
        let defs = test_defs();
        let (pistol, rifle) = (defs.gun_named("pistol").unwrap(), defs.gun_named("rifle").unwrap());
        let mut seen = [false, false];
        for seed in 1..200 {
            let (pickups, drops) = spill_chest(seed);
            assert_eq!(pickups.len(), 1, "a single-drop pool spawns exactly one thing");
            assert!(pickups[0].gun.def == pistol || pickups[0].gun.def == rifle);
            seen[(pickups[0].gun.def == rifle) as usize] = true;
            assert_eq!(pickups[0].position, Vec2::new(3.0, 2.0));
            // Weight 100 always beats the roll; weight 0 never does.
            assert_eq!(drops.len(), 1);
            assert_eq!(defs.drop_def(drops[0].def).kind, DropKind::Data);
        }
        assert_eq!(seen, [true, true], "either gun can come out");
    }

    #[test]
    fn the_first_line_of_a_single_drop_pool_is_favoured() {
        // Two lines of weight 20: the first wins 20 / (20 + 0.8 * 20) = 5/9 of the time.
        let defs = test_defs();
        let pistol = defs.gun_named("pistol").unwrap();
        let pistols = (1..2001).filter(|&seed| spill_chest(seed).0[0].gun.def == pistol).count();
        assert!((1000..1230).contains(&pistols), "{pistols} of 2000");
    }

    #[test]
    fn a_drop_out_of_reach_stays_and_one_in_reach_flies_to_the_player() {
        let defs = test_defs();
        let cell = defs.drop_named("cell").unwrap();
        let half = Vec2::new(0.1, 0.2);
        let mut drops = vec![Drop::new(cell, 3, Vec2::new(5.0, 0.0)), Drop::new(cell, 3, Vec2::new(1.5, 0.0))];
        let mut collected = 0;
        for _ in 0..60 {
            step_drops(&mut drops, &defs, Vec2::ZERO, half, |_, value| collected += value);
        }
        assert_eq!(drops.len(), 1, "the near one is collected within a second");
        assert_eq!(drops[0].position, Vec2::new(5.0, 0.0), "the far one has not moved");
        assert_eq!(collected, 3);
    }

    #[test]
    fn the_pull_tops_out_faster_than_the_player_runs() {
        let defs = test_defs();
        let cell = defs.drop_named("cell").unwrap();
        // Far from the player but in reach for a long time: keep it at the edge.
        let mut drop = vec![Drop::new(cell, 3, Vec2::new(1.9, 0.0))];
        let mut top = 0.0_f32;
        for _ in 0..600 {
            drop[0].position = Vec2::new(1.9, 0.0);
            step_drops(&mut drop, &defs, Vec2::ZERO, Vec2::splat(0.01), |_, _| {});
            top = top.max(drop[0].velocity.length());
        }
        // Unity: v = (v + 0.168) / 1.06 settles at 2.8.
        assert!((2.7..2.9).contains(&top), "top speed {top}");
        assert!(top > crate::sim::player::TOP_SPEED);
    }
}
