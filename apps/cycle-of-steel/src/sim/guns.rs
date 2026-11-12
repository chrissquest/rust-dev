//! One gun's moving parts: when it last fired and how hot it is.
//!
//! This reproduces the Unity `Gun` heat mechanic. Every shot adds one unit of
//! heat. The shot that reaches `heat_capacity` overheats the gun: it locks for
//! `heat_cooldown` seconds while the heat steps down to zero in quarter-second
//! ticks. A gun that is not being fired also loses heat, one step per quarter
//! second. Guns keep this state whether held, holstered or lying on the floor.

use super::defs::{GunDef, GunId};

/// Heat falls in steps this many seconds apart.
const HEAT_TICK: f64 = 0.25;

#[derive(Clone, Debug, PartialEq)]
pub struct Gun {
    pub def: GunId,
    pub heat: f32,
    /// The trigger is held on this gun: it does not cool by itself meanwhile.
    pub firing: bool,
    /// When it last fired, in world seconds. Starts long ago so the first shot is free.
    last_fire: f64,
    /// When the current (or last) overheat began and ends.
    overheated_at: f64,
    locked_until: f64,
    /// Whether an overheat is still stepping the heat down.
    cooling_down: bool,
    /// The next time idle cooling may take a step.
    next_idle_tick: f64,
    /// Lasers: when the beam last did its damage.
    last_damage: f64,
}

impl Gun {
    pub fn new(def: GunId) -> Self {
        Self {
            def,
            heat: 0.0,
            firing: false,
            last_fire: f64::NEG_INFINITY,
            overheated_at: 0.0,
            locked_until: f64::NEG_INFINITY,
            cooling_down: false,
            next_idle_tick: 0.0,
            last_damage: f64::NEG_INFINITY,
        }
    }

    /// How many steps an overheat takes to clear, and how much heat each removes.
    /// `None` when the gun has no heat mechanic.
    fn heat_steps(def: &GunDef) -> Option<(u32, f32)> {
        let steps = (def.heat_cooldown as f64 / HEAT_TICK) as u32;
        (def.heat_capacity > 0.0 && def.heat_cooldown > 0.0 && steps > 0)
            .then(|| (steps, def.heat_capacity / steps as f32))
    }

    /// Heat as a fraction of capacity, for the heat bar. 0 for guns without heat.
    pub fn heat_fraction(&self, def: &GunDef) -> f32 {
        match Self::heat_steps(def) {
            Some(_) => (self.heat / def.heat_capacity).clamp(0.0, 1.0),
            None => 0.0,
        }
    }

    /// Whether the gun is locked by an overheat at `now`.
    pub fn is_locked(&self, now: f64) -> bool {
        now <= self.locked_until
    }

    /// Cools the gun. Call once per simulation step for every gun in the world.
    pub fn cool(&mut self, def: &GunDef, now: f64) {
        let Some((steps, step_heat)) = Self::heat_steps(def) else {
            return;
        };

        if self.cooling_down {
            // An overheat steps down on a fixed schedule and ends at exactly zero.
            let elapsed = now - self.overheated_at;
            if elapsed >= def.heat_cooldown as f64 {
                self.heat = 0.0;
                self.cooling_down = false;
            } else {
                let taken = ((elapsed / HEAT_TICK) as u32).min(steps - 1);
                self.heat = def.heat_capacity - taken as f32 * step_heat;
            }
            return;
        }

        if !self.firing && now > self.locked_until && now > self.next_idle_tick {
            self.heat = (self.heat - step_heat).max(0.0);
            self.next_idle_tick = now + HEAT_TICK;
        }
    }

    /// Asks the gun to fire at `now`. Returns true when it does: the caller then
    /// spawns the bullets. A locked gun, or one still inside its fire delay, refuses.
    pub fn try_fire(&mut self, def: &GunDef, now: f64) -> bool {
        if self.is_locked(now) || now - self.last_fire <= def.fire_delay as f64 {
            return false;
        }
        self.last_fire = now;
        self.add_heat(def, now);
        true
    }

    /// Lasers: asks whether a beam that is touching something at `now` does its
    /// damage this step. It does once every `damage_delay`, and each time adds one
    /// unit of heat, as `LaserGun.updateFire` calls `TickHeat`.
    pub fn try_laser_hit(&mut self, def: &GunDef, now: f64) -> bool {
        if now - self.last_damage <= def.damage_delay as f64 {
            return false;
        }
        self.last_damage = now;
        self.add_heat(def, now);
        true
    }

    fn add_heat(&mut self, def: &GunDef, now: f64) {
        if self.heat + 1.0 < def.heat_capacity {
            self.heat += 1.0;
            return;
        }
        // Reached capacity: lock and start stepping down.
        self.heat = def.heat_capacity.max(0.0);
        self.overheated_at = now;
        self.locked_until = now + def.heat_cooldown as f64;
        self.cooling_down = Self::heat_steps(def).is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::defs::tests::test_defs;
    use crate::sim::{DT, TICK_RATE};

    /// Holds the trigger for `seconds`, cooling each step, and counts the shots.
    fn hold_trigger(gun: &mut Gun, def: &GunDef, start: f64, seconds: f64) -> u32 {
        let mut shots = 0;
        gun.firing = true;
        for step in 0..(seconds * TICK_RATE as f64).round() as u32 {
            let now = start + step as f64 * DT as f64;
            gun.cool(def, now);
            if gun.try_fire(def, now) {
                shots += 1;
            }
        }
        shots
    }

    #[test]
    fn a_pistol_overheats_on_every_shot_so_fires_once_a_second() {
        let defs = test_defs();
        let def = defs.gun(defs.gun_named("pistol").unwrap());
        let mut gun = Gun::new(defs.gun_named("pistol").unwrap());
        // Capacity 1, cooldown 1 s, delay 1 s: shots at 0, just after 1, just after 2...
        assert_eq!(hold_trigger(&mut gun, def, 10.0, 0.5), 1);
        assert!(gun.is_locked(10.4));
        assert_eq!(gun.heat, 0.75, "one quarter-second step down after half a second");
        assert_eq!(hold_trigger(&mut gun, def, 10.5, 2.9), 3);
    }

    #[test]
    fn an_overheat_steps_down_to_zero_over_the_cooldown() {
        let defs = test_defs();
        let id = defs.gun_named("shotgun").unwrap();
        let def = defs.gun(id);
        let mut gun = Gun::new(id);
        assert!(gun.try_fire(def, 5.0));
        // 1.5 s cooldown is six quarter-second steps of 1/6 each.
        gun.cool(def, 5.3);
        assert!((gun.heat - (1.0 - 1.0 / 6.0)).abs() < 1e-5, "{}", gun.heat);
        gun.cool(def, 6.3);
        assert!((gun.heat - (1.0 - 5.0 / 6.0)).abs() < 1e-5, "{}", gun.heat);
        gun.cool(def, 6.5);
        assert_eq!(gun.heat, 0.0);
        assert!(!gun.try_fire(def, 6.5), "locked until the cooldown has fully passed");
        assert!(gun.try_fire(def, 6.52));
    }

    #[test]
    fn a_rifle_fires_fifteen_shots_then_locks_and_cools_when_idle() {
        let defs = test_defs();
        let id = defs.gun_named("rifle").unwrap();
        let def = defs.gun(id);
        let mut gun = Gun::new(id);
        // Delay 0.4 s, capacity 15: the fifteenth shot overheats it.
        let shots = hold_trigger(&mut gun, def, 0.0, 20.0);
        // 15 shots take 14 gaps of just over 0.4 s, then 2 s locked, repeating.
        assert!(shots > 15, "it resumed after the lock: {shots}");
        assert_eq!(gun.heat_fraction(def).clamp(0.0, 1.0), gun.heat_fraction(def));

        // A fresh rifle fired three times and then left alone cools back to zero.
        let mut gun = Gun::new(id);
        for shot in 0..3 {
            assert!(gun.try_fire(def, shot as f64 * 0.5));
        }
        assert_eq!(gun.heat, 3.0);
        gun.firing = false;
        for step in 0..(10 * TICK_RATE) {
            gun.cool(def, 1.5 + step as f64 * DT as f64);
        }
        assert_eq!(gun.heat, 0.0);
    }

    #[test]
    fn a_gun_without_heat_settings_never_locks_for_long() {
        let defs = test_defs();
        let id = defs.gun_named("pistol").unwrap();
        let mut def = defs.gun(id).clone();
        def.heat_capacity = 0.0;
        def.heat_cooldown = 0.0;
        def.fire_delay = 0.0;
        let mut gun = Gun::new(id);
        // With no delay and no heat it fires on every step.
        assert_eq!(hold_trigger(&mut gun, &def, 1.0, 1.0), TICK_RATE);
        assert_eq!(gun.heat_fraction(&def), 0.0);
    }
}
