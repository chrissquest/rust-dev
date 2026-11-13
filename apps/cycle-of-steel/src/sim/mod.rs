//! The game simulation: plain state advanced by a fixed step.
//!
//! Everything here runs at [`TICK_RATE`] steps per second regardless of how fast
//! frames are drawn. The frame loop calls [`World::step`] zero or more times per
//! frame and the view blends between the last two steps, so motion is smooth at
//! any refresh rate and identical on every machine.

pub mod collide;
pub mod combat;
pub mod defs;
pub mod elemental;
pub mod enemy;
pub mod guns;
pub mod loot;
pub mod player;
pub mod rng;
pub mod room;

use std::sync::Arc;

use crystal_engine::glam::Vec2;

use crate::data::{GunKind, ShakeFile};
use combat::{Combat, Interaction, Pickup, Target, Teleport};
use crate::data::DropKind;
use loot::Drop;
use elemental::Elemental;
use defs::{Defs, GunId};
use guns::Gun;
use player::Player;
use rng::Rng;
use room::Room;

/// Simulation steps per second.
pub const TICK_RATE: u32 = 60;
/// Seconds per simulation step.
pub const DT: f32 = 1.0 / TICK_RATE as f32;

/// How many guns the player can carry.
pub const INVENTORY_SLOTS: usize = 4;

/// What the player is asking for during one step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// Direction to move in, at most length 1. Zero means stand still.
    pub movement: Vec2,
    /// Direction to shoot in. `Some` means the trigger is held; any length.
    pub aim: Option<Vec2>,
    /// Pick up what is in reach. True for the one step the button went down.
    pub interact: bool,
    /// Switch to this inventory slot (0-based).
    pub select_slot: Option<usize>,
    /// Step this many slots along, wrapping: +1 next, -1 previous.
    pub cycle_slot: i32,
}

impl Input {
    /// The same input with its one-shot requests removed, for the second and later
    /// steps of a frame: a press must not act once per step.
    pub fn held_only(self) -> Self {
        Self {
            interact: false,
            select_slot: None,
            cycle_slot: 0,
            ..self
        }
    }
}

/// Something that happened in a step that the outside may want to show or play.
/// The simulation never depends on these being read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Shake(ShakeFile),
}

pub struct World {
    pub defs: Arc<Defs>,
    pub room: Room,
    pub player: Player,
    pub combat: Combat,
    /// The guns being carried, and which one is in hand.
    pub inventory: [Option<Gun>; INVENTORY_SLOTS],
    pub selected_slot: usize,
    /// The currency the player picks up (`PlayerData.dataCells`).
    pub data_cells: u32,
    /// Steps taken since the world was created.
    pub tick: u64,
    rng: Rng,
    /// Events since they were last taken. Bounded, in case nothing takes them.
    events: Vec<Event>,
}

impl World {
    /// A world in `room`, with the room's guns and drops on the floor and its
    /// targets and chests standing.
    pub fn new(room: Room, defs: Arc<Defs>, seed: u32) -> Self {
        let player = Player::new(room.spawn);
        let mut combat = Combat::new();

        if let Some(file) = &room.file {
            // The dummy comes first, so that it keeps the first place among targets.
            for marker in file.markers.iter().filter(|marker| marker.has_script("Killable")) {
                let Some(bounds) = marker.bounds else {
                    continue;
                };
                let health = marker.number("Killable.maxHealth").unwrap_or(1.0);
                combat.targets.push(Target {
                    id: combat.targets.len() as u32,
                    position: Vec2::from(bounds.center),
                    half: Vec2::from(bounds.size) * 0.5,
                    origin: Vec2::from(bounds.center),
                    chest: None,
                    enemy: None,
                    health,
                    max_health: health,
                    last_hit: f64::NEG_INFINITY,
                    // The hub's dummy has none, so nothing freezes it, as in Unity.
                    elemental: marker.has_script("ElementalEffectHandler").then(Elemental::default),
                });
            }
            for object in &file.objects {
                let position = Vec2::from(object.position);
                if let Some(gun) = defs.gun_named(&object.prefab) {
                    combat.pickups.push(Pickup { gun: Gun::new(gun), position });
                } else if let Some(drop) = defs.drop_named(&object.prefab) {
                    // Rooms set some drops' worth: the hub's are worth far more.
                    let field = match defs.drop_def(drop).kind {
                        DropKind::Data => "DataDrop.value",
                        DropKind::Heal => "HealDrop.value",
                    };
                    let value = object.overrides.get(field).and_then(|value| value.parse().ok());
                    combat.drops.push(Drop::new(drop, value.unwrap_or(defs.drop_def(drop).value), position));
                } else if let Some(enemy) = defs.enemy_named(&object.prefab) {
                    combat.targets.push(enemy::spawn(&defs, enemy, position, combat.targets.len() as u32));
                } else if let Some(chest) = defs.chest_named(&object.prefab) {
                    let def = defs.chest(chest);
                    combat.targets.push(Target {
                        id: combat.targets.len() as u32,
                        position: position + Vec2::from(def.offset),
                        half: Vec2::from(def.half),
                        origin: position,
                        chest: Some(chest),
                        enemy: None,
                        health: def.max_health,
                        max_health: def.max_health,
                        last_hit: f64::NEG_INFINITY,
                        // Chests have no `ElementalEffectHandler`.
                        elemental: None,
                    });
                }
            }
            // Unity's `DebugRoom` and `DebugRoomExit`: interact on the spot to be
            // moved by a fixed amount, into the gun room and back out of it.
            for (script, offset) in [("DebugRoom", Vec2::new(7.0, 0.0)), ("DebugRoomExit", Vec2::new(0.0, -2.5))] {
                for marker in file.markers.iter().filter(|marker| marker.has_script(script)) {
                    let (position, half) = match marker.bounds {
                        Some(bounds) => (Vec2::from(bounds.center), Vec2::from(bounds.size) * 0.5),
                        None => (Vec2::from(marker.position), Vec2::splat(0.1)),
                    };
                    combat.teleports.push(Teleport { position, half, offset });
                }
            }
        }

        Self {
            defs,
            room,
            player,
            combat,
            inventory: [None, None, None, None],
            selected_slot: 0,
            data_cells: 0,
            tick: 0,
            rng: Rng::new(seed),
            events: Vec::new(),
        }
    }

    /// Seconds since the world was created.
    pub fn time(&self) -> f64 {
        self.tick as f64 * DT as f64
    }

    /// The gun in hand, if the selected slot holds one.
    pub fn current_gun(&self) -> Option<&Gun> {
        self.inventory[self.selected_slot].as_ref()
    }

    /// What pressing interact would act on where the player stands, if anything.
    pub fn interaction(&self) -> Option<Interaction> {
        self.combat
            .interaction(&self.defs, self.player.position, player::HALF_SIZE)
    }

    pub fn can_interact(&self) -> bool {
        self.interaction().is_some()
    }

    /// Puts a gun straight into the first free slot. Returns false when full.
    pub fn give(&mut self, gun: GunId) -> bool {
        match self.inventory.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => {
                *slot = Some(Gun::new(gun));
                true
            }
            None => false,
        }
    }

    /// Takes the events of the steps since the last call.
    pub fn drain_events(&mut self) -> std::vec::Drain<'_, Event> {
        self.events.drain(..)
    }

    /// Advances the world by [`DT`].
    pub fn step(&mut self, input: &Input) {
        let now = self.time();

        self.player.step(input, &self.room, self.combat.solids());

        if let Some(slot) = input.select_slot {
            self.select_slot(slot);
        }
        if input.cycle_slot != 0 {
            let slots = INVENTORY_SLOTS as i32;
            self.select_slot((self.selected_slot as i32 + input.cycle_slot).rem_euclid(slots) as usize);
        }
        if input.interact {
            match self.interaction() {
                Some(Interaction::Pickup(index)) => self.pick_up(index),
                Some(Interaction::Teleport(index)) => {
                    self.player.position += self.combat.teleports[index].offset;
                    // No blending across the jump: the player is simply there.
                    self.player.previous_position = self.player.position;
                    self.player.velocity = Vec2::ZERO;
                }
                None => {}
            }
        }

        // Every gun cools, wherever it is.
        let defs = self.defs.clone();
        for (slot, gun) in self.inventory.iter_mut().enumerate() {
            if let Some(gun) = gun {
                gun.firing = slot == self.selected_slot && self.player.aim.is_some();
                gun.cool(defs.gun(gun.def), now);
            }
        }
        for pickup in &mut self.combat.pickups {
            pickup.gun.firing = false;
            pickup.gun.cool(defs.gun(pickup.gun.def), now);
        }

        // The beam lasts only as long as the laser keeps firing.
        self.combat.beam = None;
        if let (Some(aim), Some(gun)) = (self.player.aim, self.inventory[self.selected_slot].as_mut()) {
            let def = defs.gun(gun.def);
            if def.kind == GunKind::Laser {
                if !gun.is_locked(now) {
                    self.combat.fire_laser(
                        gun,
                        def,
                        &self.room,
                        self.player.muzzle(aim),
                        aim,
                        self.player.crit_chance,
                        now,
                        &mut self.rng,
                        &mut self.events,
                    );
                }
            } else if gun.try_fire(def, now) {
                self.combat.fire(
                    &defs,
                    def,
                    self.player.muzzle(aim),
                    aim,
                    self.player.velocity,
                    self.player.crit_chance,
                    &mut self.rng,
                    &mut self.events,
                );
            }
        }

        let combat = &mut self.combat;
        enemy::step_enemies(&mut combat.targets, &mut combat.shots, &defs, &self.room, &mut self.player, now, &mut self.rng);
        enemy::step_shots(&mut combat.shots, &mut combat.effects, &defs, &self.room, &mut self.player, now);
        self.combat.step(&defs, &self.room, self.player.position, now, &mut self.rng);
        if self.player.health <= 0.0 {
            // Placeholder until the full loop (step 7): Unity's `PlayerData.Die`
            // ends the run; here the player simply starts the room again.
            self.player = Player::new(self.room.spawn);
        }

        let (player, data_cells) = (&mut self.player, &mut self.data_cells);
        loot::step_drops(&mut self.combat.drops, &defs, player.position, player::HALF_SIZE, |kind, value| match kind {
            // `PlayerData.incrementDataCells`.
            DropKind::Data => *data_cells += value,
            // `PlayerData.HealPlayer`: a pack is used up even at full health.
            DropKind::Heal => player.health = (player.health + value as f32).min(player.max_health),
        });

        // Nothing has to read events; do not let them pile up if nothing does.
        if self.events.len() > 256 {
            self.events.drain(..self.events.len() - 256);
        }
        self.tick += 1;
    }

    fn select_slot(&mut self, slot: usize) {
        if slot < INVENTORY_SLOTS {
            self.selected_slot = slot;
        }
    }

    /// Unity's `PlayerInventory.AddInventory`: the gun goes into the first empty
    /// slot; with none empty it replaces the one in hand, which drops at the
    /// player's feet.
    fn pick_up(&mut self, index: usize) {
        let taken = self.combat.pickups.swap_remove(index).gun;

        match self.inventory.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(taken),
            None => {
                let dropped = self.inventory[self.selected_slot].replace(taken);
                if let Some(gun) = dropped {
                    self.combat.pickups.push(Pickup {
                        gun,
                        position: self.player.position - Vec2::new(0.0, player::HALF_SIZE.y - 0.1),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::defs::tests::test_defs;

    fn world() -> World {
        World::new(Room::test_room(), Arc::new(test_defs()), 1)
    }

    fn run(world: &mut World, input: Input, seconds: f32) {
        for _ in 0..(seconds * TICK_RATE as f32) as u32 {
            world.step(&input);
        }
    }

    fn moving(movement: Vec2) -> Input {
        Input {
            movement,
            ..Input::default()
        }
    }

    fn drop_gun(world: &mut World, name: &str, position: Vec2) {
        let gun = Gun::new(world.defs.gun_named(name).unwrap());
        world.combat.pickups.push(Pickup { gun, position });
    }

    #[test]
    fn the_same_inputs_give_the_same_world() {
        let mut a = world();
        let mut b = world();
        for world in [&mut a, &mut b] {
            let shotgun = world.defs.gun_named("shotgun").unwrap();
            world.give(shotgun);
            run(world, moving(Vec2::new(1.0, 0.0)), 0.7);
            let firing = Input {
                movement: Vec2::new(-0.6, 0.8),
                aim: Some(Vec2::new(1.0, 0.3)),
                ..Input::default()
            };
            run(world, firing, 1.3);
            run(world, Input::default(), 0.2);
        }
        assert_eq!(a.player.position, b.player.position);
        assert_eq!(a.tick, b.tick);
        // The spray's random angles came out the same too.
        let positions = |world: &World| world.combat.bullets.iter().map(|bullet| bullet.position).collect::<Vec<_>>();
        assert_eq!(positions(&a), positions(&b));
    }

    #[test]
    fn walking_into_a_wall_stops_at_the_wall() {
        let mut world = world();
        // Far longer than it takes to cross the room.
        run(&mut world, moving(Vec2::new(-1.0, 0.0)), 20.0);

        let left_edge = world.player.position.x - player::HALF_SIZE.x;
        // The test room's outer wall is one cell thick.
        assert!((left_edge - world.room.cell).abs() < 1e-3, "left edge at {left_edge}");
        assert!(!world.room.box_hits_solid(world.player.position, player::HALF_SIZE));
    }

    #[test]
    fn holding_the_trigger_fires_the_gun_in_hand() {
        let mut world = world();
        let aim = Input {
            aim: Some(Vec2::X),
            ..Input::default()
        };
        run(&mut world, aim, 0.5);
        assert!(world.combat.bullets.is_empty(), "nothing in hand, nothing fired");

        let pistol = world.defs.gun_named("pistol").unwrap();
        assert!(world.give(pistol));
        run(&mut world, aim, 0.5);
        assert_eq!(world.combat.bullets.len(), 1);
        // It left from in front of the player, going right.
        let bullet = &world.combat.bullets[0];
        assert!(bullet.position.x > world.player.position.x);
        assert!(bullet.velocity.x > 0.0 && bullet.velocity.y.abs() < 1e-6);
    }

    #[test]
    fn interact_takes_the_gun_underfoot_and_a_full_inventory_swaps() {
        let mut world = world();
        let here = world.player.position;
        assert!(!world.can_interact());
        drop_gun(&mut world, "pistol", here);
        assert!(world.can_interact());

        let interact = Input {
            interact: true,
            ..Input::default()
        };
        world.step(&interact);
        assert!(world.combat.pickups.is_empty());
        let pistol = world.defs.gun_named("pistol").unwrap();
        assert_eq!(world.current_gun().map(|gun| gun.def), Some(pistol));

        // Fill the other three slots, then pick up a shotgun: it replaces the
        // pistol in hand, which lands at the player's feet.
        for _ in 0..3 {
            assert!(world.give(pistol));
        }
        assert!(!world.give(pistol), "four slots only");
        drop_gun(&mut world, "shotgun", here);
        world.step(&interact);
        let shotgun = world.defs.gun_named("shotgun").unwrap();
        assert_eq!(world.current_gun().map(|gun| gun.def), Some(shotgun));
        assert_eq!(world.combat.pickups.len(), 1);
        assert_eq!(world.combat.pickups[0].gun.def, pistol);
        assert!(world.combat.pickups[0].position.y < here.y);
    }

    #[test]
    fn interacting_on_a_teleport_spot_moves_the_player() {
        let mut world = world();
        let here = world.player.position;
        world.combat.teleports.push(Teleport {
            position: here,
            half: Vec2::splat(0.1),
            offset: Vec2::new(3.0, 0.0),
        });
        assert!(world.can_interact());
        world.step(&Input {
            interact: true,
            ..Input::default()
        });
        assert_eq!(world.player.position, here + Vec2::new(3.0, 0.0));
        assert_eq!(world.player.previous_position, world.player.position, "no slide across the jump");
        assert!(!world.can_interact(), "the spot was left behind");
    }

    #[test]
    fn slots_select_directly_and_cycle_with_wrapping() {
        let mut world = world();
        world.step(&Input {
            select_slot: Some(2),
            ..Input::default()
        });
        assert_eq!(world.selected_slot, 2);
        world.step(&Input {
            cycle_slot: 2,
            ..Input::default()
        });
        assert_eq!(world.selected_slot, 0);
        world.step(&Input {
            cycle_slot: -1,
            ..Input::default()
        });
        assert_eq!(world.selected_slot, 3);
        world.step(&Input {
            select_slot: Some(9),
            ..Input::default()
        });
        assert_eq!(world.selected_slot, 3, "a slot that does not exist is ignored");
    }

    #[test]
    fn one_shot_requests_are_dropped_for_later_steps_of_a_frame() {
        let input = Input {
            movement: Vec2::X,
            aim: Some(Vec2::Y),
            interact: true,
            select_slot: Some(1),
            cycle_slot: 1,
        };
        let held = input.held_only();
        assert_eq!((held.movement, held.aim), (Vec2::X, Some(Vec2::Y)));
        assert!(!held.interact && held.select_slot.is_none() && held.cycle_slot == 0);
    }

    #[test]
    fn firing_a_gun_with_shake_reports_it_once() {
        let mut world = world();
        let pistol = world.defs.gun_named("pistol").unwrap();
        world.give(pistol);
        let mut defs = (*world.defs).clone();
        defs.guns[pistol.0 as usize].shake = ShakeFile { time: 0.1, amplitude: 0.5, frequency: 1.0 };
        world.defs = Arc::new(defs);
        world.step(&Input {
            aim: Some(Vec2::X),
            ..Input::default()
        });
        assert_eq!(world.drain_events().count(), 1);
        assert_eq!(world.drain_events().count(), 0);
    }
    fn aiming(aim: Vec2) -> Input {
        Input {
            aim: Some(aim),
            ..Input::default()
        }
    }

    /// A target `distance` to the right of the player, on the line of a shot.
    fn target_to_the_right(world: &mut World, distance: f32, health: f32) {
        world.combat.targets.push(Target {
            id: 7,
            position: world.player.pivot() + Vec2::new(distance, 0.0),
            half: Vec2::splat(0.25),
            health,
            max_health: health,
            origin: world.player.pivot() + Vec2::new(distance, 0.0),
            chest: None,
            enemy: None,
            last_hit: f64::NEG_INFINITY,
            elemental: None,
        });
    }

    #[test]
    fn a_laser_reaches_the_wall_and_stops_when_the_trigger_is_let_go() {
        let mut world = world();
        world.give(world.defs.gun_named("laser").unwrap());
        world.step(&aiming(Vec2::X));
        let beam = world.combat.beam.expect("the laser fires on the first step");
        assert_eq!(beam.origin, world.player.muzzle(Vec2::X));
        // The right-hand wall of the test room starts at x = 15.5.
        assert!((beam.origin.x + beam.length - 15.5).abs() < 1e-3, "{beam:?}");
        assert!(world.combat.bullets.is_empty(), "a laser fires no bullets");
        assert!(world.drain_events().any(|event| matches!(event, Event::Shake(_))));

        world.step(&Input::default());
        assert_eq!(world.combat.beam, None);
    }

    #[test]
    fn a_laser_stops_at_a_target_and_hurts_it_every_damage_delay() {
        let mut world = world();
        world.give(world.defs.gun_named("laser").unwrap());
        target_to_the_right(&mut world, 3.0, 1.0e6);
        // Damage 100 every 0.1 s: the first hit at once, then one every 7 steps,
        // the first step on which more than 0.1 s has passed.
        run(&mut world, aiming(Vec2::X), 1.0);
        let beam = world.combat.beam.unwrap();
        assert!((beam.length - (3.0 - 0.25 - player::MUZZLE_DISTANCE)).abs() < 1e-3, "{beam:?}");
        let hits = (1.0e6 - world.combat.targets[0].health) / 100.0;
        assert!((8.0..=10.0).contains(&hits), "{hits} hits in a second");
    }

    #[test]
    fn a_laser_passes_through_where_a_dead_target_stood() {
        let mut world = world();
        world.give(world.defs.gun_named("laser").unwrap());
        target_to_the_right(&mut world, 3.0, 150.0);
        run(&mut world, aiming(Vec2::X), 0.5);
        assert!(world.combat.targets.is_empty(), "two hits of 100 kill it");
        assert!(world.combat.beam.unwrap().length > 5.0);
    }

    #[test]
    fn a_laser_overheats_after_twenty_hits_and_comes_back_by_itself() {
        let mut world = world();
        world.give(world.defs.gun_named("laser").unwrap());
        // Heat comes from the beam touching anything, a wall included: 20 hits
        // 0.1 s apart overheat it in about two seconds.
        run(&mut world, aiming(Vec2::X), 1.5);
        assert!(world.combat.beam.is_some());
        assert!(world.current_gun().unwrap().heat >= 13.0);
        run(&mut world, aiming(Vec2::X), 1.0);
        assert_eq!(world.combat.beam, None, "overheated: no beam though the trigger is held");
        assert!(world.current_gun().unwrap().is_locked(world.time()));
        // The cooldown is 10 s; still holding the trigger, the beam returns.
        run(&mut world, aiming(Vec2::X), 9.0);
        assert_eq!(world.combat.beam, None);
        run(&mut world, aiming(Vec2::X), 1.5);
        assert!(world.combat.beam.is_some());
    }

    #[test]
    fn a_laser_without_heat_never_stops() {
        let mut world = world();
        world.give(world.defs.gun_named("endless_laser").unwrap());
        target_to_the_right(&mut world, 3.0, 1.0e6);
        for _ in 0..(5 * TICK_RATE) {
            world.step(&aiming(Vec2::X));
            assert!(world.combat.beam.is_some());
        }
        assert_eq!(world.current_gun().unwrap().heat, 0.0);
        // Its 0.02 s delay is longer than one step, so it hits every other step.
        let hits = (1.0e6 - world.combat.targets[0].health) / 100.0;
        assert!((140.0..=160.0).contains(&hits), "{hits} hits in five seconds");
    }

    #[test]
    fn every_hit_of_a_laser_is_a_crit_when_the_chance_is_certain() {
        let mut world = world();
        world.give(world.defs.gun_named("laser").unwrap());
        world.player.crit_chance = 1.0;
        target_to_the_right(&mut world, 3.0, 1000.0);
        world.step(&aiming(Vec2::X));
        assert_eq!(world.combat.targets[0].health, 800.0);
    }
}
