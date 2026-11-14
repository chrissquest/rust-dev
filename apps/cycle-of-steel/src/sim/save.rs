//! What carries over between runs and launches: the player's stats, upgrades,
//! data cells and guns (Unity's `PlayerSaveData`), and the upgrade bench's prices.
//!
//! The world keeps its last [`Save`] in memory and bumps a counter each time it
//! saves; the app writes it to disk. Dying restores the inventory from it, as
//! Unity's `Respawn` reloads the file. Nothing here touches a file.
//!
//! Unlike Unity's `BinaryFormatter` file this is plain JSON with a version, and
//! guns are kept by name, not by number: several guns share a number in Unity.

use serde::{Deserialize, Serialize};

use super::INVENTORY_SLOTS;

/// Bumped when the format changes in a way old files cannot be read as.
pub const SAVE_VERSION: u32 = 1;

/// The gun a new save starts with: Unity's `IDSystem.gameItems[0]`.
pub const STARTING_GUN: &str = "pistol_1";

/// What the upgrade bench sells (`UpgradeMenu`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upgrade {
    Crit,
    Health,
    Move,
}

impl Upgrade {
    pub const ALL: [Upgrade; 3] = [Upgrade::Crit, Upgrade::Health, Upgrade::Move];
}

/// How many times each upgrade has been bought (`UpgradeMenu.critLevel`,
/// `hpLevel`, `moveLevel`). The prices and gains rise and fall with them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upgrades {
    pub crit: u32,
    pub health: u32,
    #[serde(rename = "move")]
    pub move_speed: u32,
}

impl Upgrades {
    pub fn level(&self, upgrade: Upgrade) -> u32 {
        match upgrade {
            Upgrade::Crit => self.crit,
            Upgrade::Health => self.health,
            Upgrade::Move => self.move_speed,
        }
    }

    fn level_mut(&mut self, upgrade: Upgrade) -> &mut u32 {
        match upgrade {
            Upgrade::Crit => &mut self.crit,
            Upgrade::Health => &mut self.health,
            Upgrade::Move => &mut self.move_speed,
        }
    }

    /// `UpgradeMenu.updateStats`: data cells for the next one.
    pub fn price(&self, upgrade: Upgrade) -> u32 {
        let level = self.level(upgrade);
        match upgrade {
            // `5 + FloorToInt(2.5 * critLevel)`.
            Upgrade::Crit => 5 + level * 5 / 2,
            Upgrade::Health => 10 + 5 * level,
            Upgrade::Move => 6 + 3 * level,
        }
    }

    /// `UpgradeMenu.updateStats`: what the next one adds. Crit chance as a
    /// fraction, health as maximum health, move as `moveSpeed`.
    pub fn gain(&self, upgrade: Upgrade) -> f32 {
        let level = self.level(upgrade);
        match upgrade {
            // `Max(1, 5 - FloorToInt(0.5 * critLevel)) / 100`.
            Upgrade::Crit => (5 - (level / 2).min(4) as i32).max(1) as f32 / 100.0,
            Upgrade::Health => 200.0,
            Upgrade::Move => (2.0 - 0.25 * level as f32).max(0.25),
        }
    }

    pub(super) fn bought(&mut self, upgrade: Upgrade) {
        *self.level_mut(upgrade) += 1;
    }
}

/// Unity's `PlayerSaveData`, field for field, plus a version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Save {
    pub version: u32,
    pub max_health: f32,
    pub health: f32,
    pub move_speed: f32,
    pub crit_chance: f32,
    pub data_cells: u32,
    pub selected_slot: usize,
    /// Gun names from `guns.json`, by slot.
    pub inventory: [Option<String>; INVENTORY_SLOTS],
    pub upgrades: Upgrades,
    /// `PlayerData.difficulty`.
    pub difficulty: f32,
    /// `PlayerData.eventsCompleted`: the [`PlayerEvent`](super::dialogue::PlayerEvent)s
    /// said, by name.
    #[serde(default)]
    pub events: Vec<String>,
}

impl Save {
    /// `PlayerData.LoadData` with no file: full health, base stats, the
    /// starting gun in the first slot.
    pub fn new_game(max_health: f32) -> Self {
        Self {
            version: SAVE_VERSION,
            max_health,
            health: max_health,
            move_speed: super::player::BASE_MOVE_SPEED,
            crit_chance: 0.0,
            data_cells: 0,
            selected_slot: 0,
            inventory: [Some(STARTING_GUN.to_string()), None, None, None],
            upgrades: Upgrades::default(),
            difficulty: 1.0,
            events: Vec::new(),
        }
    }

    /// Reads a save file's text. A file from a newer version is refused
    /// rather than half read.
    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        let save: Save = serde_json::from_str(text)?;
        anyhow::ensure!(save.version <= SAVE_VERSION, "save version {} is newer than this game", save.version);
        Ok(save)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a save always serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_and_gains_follow_the_upgrade_menu() {
        let mut upgrades = Upgrades::default();
        assert_eq!(upgrades.price(Upgrade::Crit), 5);
        assert_eq!(upgrades.gain(Upgrade::Crit), 0.05);
        assert_eq!(upgrades.price(Upgrade::Health), 10);
        assert_eq!(upgrades.gain(Upgrade::Health), 200.0);
        assert_eq!(upgrades.price(Upgrade::Move), 6);
        assert_eq!(upgrades.gain(Upgrade::Move), 2.0);
        for _ in 0..3 {
            upgrades.bought(Upgrade::Crit);
            upgrades.bought(Upgrade::Move);
        }
        // 5 + floor(7.5) and 5 - floor(1.5).
        assert_eq!(upgrades.price(Upgrade::Crit), 12);
        assert_eq!(upgrades.gain(Upgrade::Crit), 0.04);
        assert_eq!(upgrades.price(Upgrade::Move), 15);
        assert_eq!(upgrades.gain(Upgrade::Move), 1.25);
        for _ in 0..20 {
            upgrades.bought(Upgrade::Crit);
            upgrades.bought(Upgrade::Move);
        }
        assert_eq!(upgrades.gain(Upgrade::Crit), 0.01, "never below 1%");
        assert_eq!(upgrades.gain(Upgrade::Move), 0.25, "never below 0.25");
    }

    #[test]
    fn a_save_reads_back_as_written() {
        let mut save = Save::new_game(1000.0);
        save.inventory[2] = Some("laser_2".into());
        save.upgrades.health = 2;
        let read = Save::from_json(&save.to_json()).unwrap();
        assert_eq!(read, save);
        let newer = save.to_json().replace("\"version\": 1", "\"version\": 99");
        assert!(Save::from_json(&newer).is_err());
    }
}
