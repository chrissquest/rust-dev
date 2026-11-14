//! The game simulation: plain state advanced by a fixed step.
//!
//! Everything here runs at [`TICK_RATE`] steps per second regardless of how fast
//! frames are drawn. The frame loop calls [`World::step`] zero or more times per
//! frame and the view blends between the last two steps, so motion is smooth at
//! any refresh rate and identical on every machine.

pub mod collide;
pub mod combat;
pub mod defs;
pub mod dialogue;
pub mod dungeon;
pub mod elemental;
pub mod enemy;
pub mod guns;
pub mod loot;
pub mod player;
pub mod rng;
pub mod room;
pub mod save;

use std::sync::Arc;

use crystal_engine::glam::Vec2;

use crate::data::{GunKind, ShakeFile};
use combat::{Combat, Grate, GrateKind, Interaction, Pickup, Target, Teleport};
use crate::data::DropKind;
use loot::Drop;
use elemental::Elemental;
use defs::{Defs, GunId};
use guns::Gun;
use player::Player;
use rng::Rng;
use room::Room;
use save::{Save, Upgrade, Upgrades};
use dialogue::{Dialogue, DialogueTrigger, PlayerEvent};

/// Simulation steps per second.
pub const TICK_RATE: u32 = 60;
/// Seconds per simulation step.
pub const DT: f32 = 1.0 / TICK_RATE as f32;

/// Mixed into the world's seed for the first floor's layout, so it is not drawn
/// from the same numbers as the world's own.
const FLOOR_SEED_MIX: u32 = 0x9e37_79b9;
/// `DungeonGenerator.LeftDungeon` sets difficulty to this after a cleared floor.
const CLEARED_DIFFICULTY: f32 = 2.0;

/// How far further in `PlayerData.Update` moves a player entering a room that
/// seals, in units.
const ROOM_ENTRY_STEP: f32 = 0.75;

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
    Sound(Sound),
    /// The upgrade bench was used: the app shows its menu (`UpgradeBench`).
    OpenUpgrades,
}

/// A sound the world asks for. What each sounds like is in the definitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    /// A gun fired (`SingleBulletGun`, `SprayGun`: `fireSound.Play()`).
    Fire(GunId),
    /// A laser started and stopped firing (`LaserGun.startFiring`, `stopFiring`).
    LaserStart(GunId),
    LaserStop,
    /// An effect with a sound appeared, such as an explosion.
    Effect(defs::EffectId),
    /// An enemy died and left its body (`RandomBodyScript`).
    Death(defs::EnemyId),
    /// `PlayerData`: hurt, healed, a data cell picked up.
    Hurt,
    Heal,
    Pickup,
    /// `PlayerAnimations`: moving started and stopped.
    WalkStart,
    WalkStop,
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
    /// A dungeon floor's rooms, when the world has one.
    pub floor: Option<dungeon::Floor>,
    /// What a new floor is made from, when the world is a run: the hub with a
    /// floor beside it.
    run: Option<dungeon::RunAssets>,
    /// `PlayerData.difficulty`: 1 on a new save, 2 once a floor has been
    /// cleared and left. It scales how hard enemies push and how often they fire.
    pub difficulty: f32,
    /// Goes up whenever the room is rebuilt (a new floor), for the view to redraw it.
    pub room_version: u64,
    /// What the upgrade bench has sold.
    pub upgrades: Upgrades,
    /// The player's lines, showing and to come.
    pub dialogue: Dialogue,
    /// When the intro is said (`HubRespawn`), in world seconds.
    intro_at: Option<f64>,
    /// The last save made or loaded, and how many saves have been made, for
    /// the app to notice a new one and write it out.
    pub saved: Option<Save>,
    pub saves: u64,
    /// The laser whose sound is playing, and whether the walking sound is.
    laser_sounding: Option<GunId>,
    walking: bool,
    /// The block the player was in last step, to notice them entering a room.
    last_block: (i32, i32),
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
        let player_position = player.position;
        let mut combat = Combat::new();

        if let Some(file) = &room.file {
            populate(&mut combat, file, &defs, |_| true);
        }

        Self {
            defs,
            room,
            player,
            combat,
            inventory: [None, None, None, None],
            selected_slot: 0,
            data_cells: 0,
            floor: None,
            run: None,
            difficulty: 1.0,
            room_version: 0,
            upgrades: Upgrades::default(),
            dialogue: Dialogue::default(),
            intro_at: None,
            saved: None,
            saves: 0,
            laser_sounding: None,
            walking: false,
            last_block: enemy::block_of(player_position),
            tick: 0,
            rng: Rng::new(seed),
            events: Vec::new(),
        }
    }

    /// A run: the hub, with a floor generated beside it, as Unity's scene keeps
    /// the hub and the dungeon apart. The player starts in the hub.
    pub fn new_run(assets: dungeon::RunAssets, defs: Arc<Defs>, seed: u32) -> anyhow::Result<Self> {
        let mut layout_rng = Rng::new(seed ^ FLOOR_SEED_MIX);
        let layout = dungeon::generate(&assets.settings, &mut layout_rng);
        let built = dungeon::build(&layout, &assets.rooms, &assets.settings, Some(&assets.hub))?;
        let mut world = Self::new(Room::from_file(built.file), defs, seed);
        world.floor = Some(built.floor);
        world.mark_rooms();
        world.run = Some(assets);
        world.intro_at = Some(dialogue::INTRO_DELAY);
        Ok(world)
    }

    /// A new floor in place of the old one: the hub and whatever is in it stay;
    /// everything on the old floor goes (`DungeonGenerator.Init`).
    fn new_floor(&mut self) {
        let Some(run) = &self.run else {
            return;
        };
        let layout = dungeon::generate(&run.settings, &mut self.rng);
        let Ok(built) = dungeon::build(&layout, &run.rooms, &run.settings, Some(&run.hub)) else {
            return;
        };
        let start_x = built.floor.start_x;
        let in_hub = |position: Vec2| position.x < start_x;
        let combat = &mut self.combat;
        combat.targets.retain(|target| in_hub(target.origin));
        combat.pickups.retain(|pickup| in_hub(pickup.position));
        combat.drops.retain(|drop| in_hub(drop.position));
        combat.remains.retain(|remains| in_hub(remains.position));
        combat.grates.retain(|grate| in_hub(grate.position));
        combat.barriers.retain(|(center, _)| in_hub(*center));
        combat.teleports.retain(|teleport| in_hub(teleport.position));
        combat.exits.retain(|(center, _)| in_hub(*center));
        combat.entrances.retain(|(center, _)| in_hub(*center));
        combat.benches.retain(|(center, _)| in_hub(*center));
        combat.dialogues.retain(|trigger| in_hub(trigger.center));
        combat.bullets.clear();
        combat.shots.clear();
        combat.blasts.clear();
        combat.effects.clear();
        combat.damage_numbers.clear();
        self.room = Room::from_file(built.file);
        if let Some(file) = &self.room.file {
            populate(&mut self.combat, file, &self.defs, |position| !in_hub(position));
        }
        self.floor = Some(built.floor);
        self.mark_rooms();
        self.room_version += 1;
    }

    /// Notes which rooms of the floor start with enemies to clear.
    fn mark_rooms(&mut self) {
        let Some(floor) = &mut self.floor else {
            return;
        };
        for room in &mut floor.rooms {
            room.had_enemies = self
                .combat
                .targets
                .iter()
                .filter_map(|target| target.enemy.as_ref())
                .any(|enemy| enemy.counted && enemy.home == room.block);
        }
    }

    /// Puts the player somewhere at once: no blending across the jump, no speed
    /// carried, and no room entered on the way.
    pub fn teleport(&mut self, position: Vec2) {
        self.player.position = position;
        self.player.previous_position = position;
        self.player.velocity = Vec2::ZERO;
        self.last_block = enemy::block_of(position);
    }

    /// `ExitHub` then `Player.TeleportDungeon`: into the floor's spawn room.
    pub fn enter_dungeon(&mut self) {
        if let Some(entry) = self.floor.as_ref().map(|floor| floor.entry) {
            let entry = self.room.nearest_open_point(entry);
            self.teleport(entry);
        }
    }

    /// `ExitDungeon.OnInteract` and `DungeonGenerator.LeftDungeon`: healed and
    /// back in the hub; a floor with all but two of its rooms cleared is
    /// replaced by a new one, and enemies get harder.
    fn leave_dungeon(&mut self) {
        self.player.health = self.player.max_health;
        self.save(true);
        let spawn = self.room.spawn;
        self.teleport(spawn);
        let Some(floor) = &self.floor else {
            return;
        };
        if floor.rooms_cleared + 2 >= floor.rooms.len() {
            self.difficulty = CLEARED_DIFFICULTY;
            self.new_floor();
        }
    }

    /// `PlayerData.LoadData`: the player as a save left them. Guns whose
    /// names are no longer in `guns.json` are left out.
    pub fn load(&mut self, save: &Save) {
        let player = &mut self.player;
        player.max_health = save.max_health;
        // `SetHealth`: never above the maximum.
        player.health = save.health.min(save.max_health);
        player.move_speed = save.move_speed;
        player.crit_chance = save.crit_chance;
        self.data_cells = save.data_cells;
        self.select_slot(save.selected_slot);
        // Saves from before Unity kept a difficulty read it as 0.
        self.difficulty = if save.difficulty != 0.0 { save.difficulty } else { 1.0 };
        for (slot, name) in self.inventory.iter_mut().zip(&save.inventory) {
            *slot = name.as_deref().and_then(|name| self.defs.gun_named(name)).map(Gun::new);
        }
        self.upgrades = save.upgrades;
        for (said, event) in self.dialogue.said.iter_mut().zip(PlayerEvent::ALL) {
            *said = save.events.iter().any(|name| name == event.name());
        }
        self.saved = Some(save.clone());
    }

    /// `PlayerData.SaveData`. Without the inventory, the guns of the last save
    /// are kept (a new save's starting gun when there is none): dying and
    /// quitting to the menu do not keep what was picked up since.
    fn save(&mut self, with_inventory: bool) {
        let mut save = Save::new_game(self.player.max_health);
        save.health = self.player.health;
        save.move_speed = self.player.move_speed;
        save.crit_chance = self.player.crit_chance;
        save.data_cells = self.data_cells;
        save.selected_slot = self.selected_slot;
        save.upgrades = self.upgrades;
        save.difficulty = self.difficulty;
        save.events = PlayerEvent::ALL
            .into_iter()
            .filter(|&event| self.dialogue.said[event as usize])
            .map(|event| event.name().to_string())
            .collect();
        if with_inventory {
            save.inventory = std::array::from_fn(|slot| self.inventory[slot].as_ref().map(|gun| self.defs.gun(gun.def).name.clone()));
        } else if let Some(saved) = &self.saved {
            save.inventory = saved.inventory.clone();
        }
        self.saved = Some(save);
        self.saves += 1;
    }

    /// `UpgradeMenu.UpgradeCrit`, `UpgradeHp`, `UpgradeMove`: buys one if the
    /// data cells cover it. More health also heals fully (`AddMaxHealth`).
    pub fn buy(&mut self, upgrade: Upgrade) -> bool {
        let price = self.upgrades.price(upgrade);
        if self.data_cells < price {
            return false;
        }
        self.data_cells -= price;
        let gain = self.upgrades.gain(upgrade);
        match upgrade {
            Upgrade::Crit => self.player.crit_chance += gain,
            Upgrade::Health => {
                self.player.max_health += gain;
                self.player.health = self.player.max_health;
            }
            Upgrade::Move => self.player.move_speed += gain,
        }
        self.upgrades.bought(upgrade);
        true
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

    /// Whether interact does anything: reads on through a line, or acts on
    /// something in reach.
    pub fn can_interact(&self) -> bool {
        self.dialogue.is_showing() || self.interaction().is_some()
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

        // `PlayerInput`: while a line shows, interact moves the dialogue on
        // and the player neither moves nor shoots.
        let talking = self.dialogue.is_showing();
        if talking && input.interact {
            self.dialogue.advance();
        }
        let held = *input;
        let input = &if talking { Input { movement: Vec2::ZERO, aim: None, interact: false, ..held } } else { held };

        self.player.step(input, &self.room, self.combat.solids());
        self.say_what_is_due(now);
        if self.combat.entrances.iter().any(|&(center, half)| combat::boxes_overlap(self.player.position, player::HALF_SIZE, center, half)) {
            // `ExitHub`: saved, guns and all, on the way in.
            self.save(true);
            self.enter_dungeon();
        }
        self.update_rooms();

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
                Some(Interaction::Exit(_)) => self.leave_dungeon(),
                Some(Interaction::Bench(_)) => self.events.push(Event::OpenUpgrades),
                Some(Interaction::Teleport(index)) => {
                    let teleport = &self.combat.teleports[index];
                    if let Some((event, lines)) = teleport.says {
                        self.dialogue.event(event, dialogue::lines(lines));
                    }
                    self.player.position += teleport.offset;
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

        // `PlayerAnimations.AnimateOnMove`: the walking sound plays while moving
        // is held, even while a line stops the player.
        let walking = held.movement != Vec2::ZERO;
        if walking != self.walking {
            self.walking = walking;
            self.events.push(Event::Sound(if walking { Sound::WalkStart } else { Sound::WalkStop }));
        }
        // A laser sounds from when its trigger is pulled to when it is let go.
        let laser = match (self.player.aim, &self.inventory[self.selected_slot]) {
            (Some(_), Some(gun)) if defs.gun(gun.def).kind == GunKind::Laser => Some(gun.def),
            _ => None,
        };
        if laser != self.laser_sounding {
            if self.laser_sounding.is_some() {
                self.events.push(Event::Sound(Sound::LaserStop));
            }
            if let Some(gun) = laser {
                self.events.push(Event::Sound(Sound::LaserStart(gun)));
            }
            self.laser_sounding = laser;
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
                self.events.push(Event::Sound(Sound::Fire(gun.def)));
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
        enemy::step_enemies(&mut combat.targets, &combat.barriers, &mut combat.shots, self.difficulty, &defs, &self.room, &mut self.player, now, &mut self.rng);
        enemy::step_shots(&mut combat.shots, &mut combat.effects, &defs, &self.room, &mut self.player, now);
        let hurt_before = self.player.last_hurt;
        self.combat.step(&defs, &self.room, &mut self.player, now, &mut self.rng);
        self.events.extend(self.combat.sounds.drain(..).map(Event::Sound));
        if self.player.last_hurt != hurt_before {
            // `PlayerData.DamagePlayer` plays it when a blow lands.
            self.events.push(Event::Sound(Sound::Hurt));
        }
        if self.player.health <= 0.0 {
            // `PlayerData.Die` and `Respawn`: healed, saved without the guns and
            // reloaded, so what was picked up since the last save is lost; back
            // in the hub with a new floor. Unity's 5 second death screen waits
            // for the menus. A world that is one room simply starts it again.
            self.player.health = self.player.max_health;
            self.save(false);
            if let Some(saved) = self.saved.clone() {
                self.load(&saved);
            }
            // `HubRespawn.StartAnim`: waking in the hub, then the intro if unsaid.
            self.intro_at = self.run.is_some().then_some(now + dialogue::INTRO_DELAY);
            let spawn = self.room.spawn;
            self.teleport(spawn);
            self.new_floor();
        }

        let (player, data_cells, events) = (&mut self.player, &mut self.data_cells, &mut self.events);
        loot::step_drops(&mut self.combat.drops, &defs, player.position, player::HALF_SIZE, |kind, value| match kind {
            // `PlayerData.incrementDataCells`.
            DropKind::Data => {
                *data_cells += value;
                events.push(Event::Sound(Sound::Pickup));
            }
            // `PlayerData.HealPlayer`: a pack is used up even at full health.
            DropKind::Heal => {
                player.health = (player.health + value as f32).min(player.max_health);
                events.push(Event::Sound(Sound::Heal));
            }
        });

        // Nothing has to read events; do not let them pile up if nothing does.
        if self.events.len() > 256 {
            self.events.drain(..self.events.len() - 256);
        }
        self.tick += 1;
    }

    /// Says the intro once its time comes, and what the boxes the player
    /// stands in have to say (`DialogueEvent`, `ExitRoomDialogue`).
    fn say_what_is_due(&mut self, now: f64) {
        if self.intro_at.is_some_and(|at| now >= at) {
            self.intro_at = None;
            self.dialogue.event(PlayerEvent::Intro, dialogue::lines(&dialogue::INTRO));
        }
        let (position, dialogue) = (self.player.position, &mut self.dialogue);
        for trigger in self.combat.dialogues.iter_mut().filter(|trigger| !trigger.said) {
            if combat::boxes_overlap(position, player::HALF_SIZE, trigger.center, trigger.half) {
                trigger.said = true;
                let lines = trigger.lines.iter().cloned();
                match trigger.event {
                    Some(event) => dialogue.event(event, lines),
                    None => dialogue.queue(lines),
                }
            }
        }
    }

    /// `PlayerData.Update` and `RoomScript`: stepping into a room whose enemies
    /// still live seals its doors and moves the player 0.75 further in, so the
    /// door does not close on them; a sealed room opens once its last counted
    /// enemy dies.
    fn update_rooms(&mut self) {
        let Some(floor) = &mut self.floor else {
            return;
        };
        let block = enemy::block_of(self.player.position);
        if let Some(index) = floor.room_at(block) {
            floor.rooms[index].explored = true;
        }
        let enemies_left = |targets: &[Target], block: (i32, i32)| {
            targets
                .iter()
                .filter_map(|target| target.enemy.as_ref())
                .any(|enemy| enemy.counted && enemy.home == block)
        };
        if block != self.last_block {
            let from = self.last_block;
            self.last_block = block;
            if let Some(index) = floor.room_at(block)
                && !floor.rooms[index].sealed
                && enemies_left(&self.combat.targets, block)
            {
                floor.seal(index, &mut self.room);
                let entered = Vec2::new((block.0 - from.0) as f32, (block.1 - from.1) as f32);
                self.player.position += entered * ROOM_ENTRY_STEP;
                self.player.previous_position = self.player.position;
            }
        }
        for index in 0..floor.rooms.len() {
            let room = &floor.rooms[index];
            if (room.sealed || (room.had_enemies && !room.cleared)) && !enemies_left(&self.combat.targets, room.block) {
                // `RoomScript.SubEnemyCount`: the last one died. Unlike Unity,
                // the boss room opens too: there its walls stayed, shutting the
                // player in for good.
                if !room.cleared && room.had_enemies {
                    floor.rooms[index].cleared = true;
                    floor.rooms_cleared += 1;
                }
                floor.unseal(index, &mut self.room);
            }
        }
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

/// Adds what a room file holds to `combat`, keeping only what stands where
/// `keep` says: barriers, the dummy, guns, drops, enemies, chests, grates, and
/// the hub's and dungeon's ways in and out.
fn populate(combat: &mut Combat, file: &crate::data::RoomFile, defs: &Defs, keep: impl Fn(Vec2) -> bool) {
    let mut next_id = combat.targets.iter().map(|target| target.id + 1).max().unwrap_or(0);
    let mut id = || {
        next_id += 1;
        next_id - 1
    };
    for barrier in file.barriers.iter().filter(|barrier| keep(Vec2::from(barrier.center))) {
        combat.barriers.push((Vec2::from(barrier.center), Vec2::from(barrier.size) * 0.5));
    }
    // The dummy comes first, so that it keeps the first place among targets.
    for marker in file.markers.iter().filter(|marker| marker.has_script("Killable")) {
        let Some(bounds) = marker.bounds.filter(|bounds| keep(Vec2::from(bounds.center))) else {
            continue;
        };
        let health = marker.number("Killable.maxHealth").unwrap_or(1.0);
        combat.targets.push(Target {
            id: id(),
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
    for object in file.objects.iter().filter(|object| keep(Vec2::from(object.position))) {
        let position = Vec2::from(object.position);
        if object.prefab == "EffectGrate" {
            combat.grates.push(grate(object, position));
            continue;
        }
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
            let mut target = enemy::spawn(defs, enemy, position, id());
            if let Some(state) = &mut target.enemy {
                // `Killable.safeToUpdateRoomCounter`: under the room's `Enemies`.
                state.counted = defs.enemy(enemy).counts_as_enemy && object.group.as_deref() == Some("Enemies");
            }
            combat.targets.push(target);
        } else if let Some(chest) = defs.chest_named(&object.prefab) {
            let def = defs.chest(chest);
            combat.targets.push(Target {
                id: id(),
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
    fn boxes<'a>(file: &'a crate::data::RoomFile, script: &'a str) -> impl Iterator<Item = (Vec2, Vec2)> + 'a {
        file.markers.iter().filter(move |marker| marker.has_script(script)).map(|marker| match marker.bounds {
            Some(bounds) => (Vec2::from(bounds.center), Vec2::from(bounds.size) * 0.5),
            None => (Vec2::from(marker.position), Vec2::splat(0.1)),
        })
    }
    // Unity's `DebugRoom` and `DebugRoomExit`: interact on the spot to be moved
    // by a fixed amount, into the gun room and back out of it, with a line.
    let ways = [
        ("DebugRoom", Vec2::new(7.0, 0.0), (PlayerEvent::Secret, &dialogue::SECRET[..])),
        ("DebugRoomExit", Vec2::new(0.0, -2.5), (PlayerEvent::Secret2, &dialogue::SECRET2[..])),
    ];
    for (script, offset, says) in ways {
        for (position, half) in boxes(file, script).filter(|(position, _)| keep(*position)) {
            combat.teleports.push(Teleport { position, half, offset, says: Some(says) });
        }
    }
    // `DialogueEvent` boxes, with their event and lines, and the exit room's.
    for marker in &file.markers {
        let (event, lines): (Option<PlayerEvent>, std::sync::Arc<[std::sync::Arc<str>]>) = if marker.has_script("DialogueEvent") {
            let event = marker.number("DialogueEvent.playerEvent").and_then(|number| PlayerEvent::from_number(number as usize));
            let lines = marker.lists.get("DialogueEvent.dialogue").map(|lines| lines.iter().map(|line| std::sync::Arc::from(line.as_str())).collect());
            match (event, lines) {
                (Some(event), Some(lines)) => (Some(event), lines),
                _ => continue,
            }
        } else if marker.has_script("ExitRoomDialogue") {
            (None, dialogue::lines(&dialogue::EXIT_ROOM).collect())
        } else {
            continue;
        };
        let Some(bounds) = marker.bounds.filter(|bounds| keep(Vec2::from(bounds.center))) else {
            continue;
        };
        combat.dialogues.push(DialogueTrigger {
            center: Vec2::from(bounds.center),
            half: Vec2::from(bounds.size) * 0.5,
            event,
            lines,
            said: false,
        });
    }
    combat.exits.extend(boxes(file, "ExitDungeon").filter(|(position, _)| keep(*position)));
    combat.entrances.extend(boxes(file, "ExitHub").filter(|(position, _)| keep(*position)));
    combat.benches.extend(boxes(file, "UpgradeBench").filter(|(position, _)| keep(*position)));
}

/// A placed `EffectGrate`. Every placement in the rooms sets its box and, all
/// but the plain ones, whether it is water or lava; the rest comes from
/// `Prefabs/Enemies/EffectGrate.prefab`: water, a 1 by 1 box, duration 3.
fn grate(object: &crate::data::ObjectFile, position: Vec2) -> Grate {
    let number = |field: &str, default: f32| {
        object.overrides.get(field).and_then(|value| value.parse().ok()).unwrap_or(default)
    };
    let lava = number("EffectGrate.lava", 0.0) != 0.0;
    let water = number("EffectGrate.water", 1.0) != 0.0;
    Grate {
        position: position + Vec2::new(number("BoxCollider2D.m_Offset.x", 0.0), number("BoxCollider2D.m_Offset.y", 0.0)),
        half: Vec2::new(number("BoxCollider2D.m_Size.x", 1.0), number("BoxCollider2D.m_Size.y", 1.0)) * 0.5,
        // `EffectGrate.OnTriggerStay2D` checks water before lava.
        kind: if water || !lava { GrateKind::Water } else { GrateKind::Lava },
        duration: number("EffectGrate.duration", 3.0),
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
            says: None,
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
        // The shot's sound comes too; one shake.
        let shakes = |world: &mut World| world.drain_events().filter(|event| matches!(event, Event::Shake(_))).count();
        assert_eq!(shakes(&mut world), 1);
        assert_eq!(shakes(&mut world), 0);
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
