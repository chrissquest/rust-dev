//! The placeholder menus: what each shows and what choosing a row does.
//!
//! A menu pauses the game while it is open, as Unity's menus set
//! `Time.timeScale` to 0: the app stops stepping the world and hands keys,
//! taps and buttons here instead. The menus are Unity's: the start menu
//! (`MainMenu`), the pause menu (`PauseMenu`), the death screen (`DeathMenu`)
//! and the upgrade bench's (`UpgradeMenu`). Their look is redone in the UI pass.

use crate::sim::World;
use crate::sim::save::Upgrade;

/// One line of a menu as drawn: its text, a price in data cells, and whether
/// choosing it can do anything right now.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRow {
    pub label: String,
    pub price: Option<u32>,
    pub enabled: bool,
}

/// A menu as drawn: its title, a line under it, its rows, and the one
/// highlighted.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuView {
    pub title: &'static str,
    pub note: Option<String>,
    pub rows: Vec<MenuRow>,
    pub selected: usize,
    /// Hide the game behind it entirely.
    pub opaque: bool,
}

/// What a menu asks the app for when a row is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Nothing: a row that is only there to be read.
    Nothing,
    Buy(Upgrade),
    Close,
    /// Start menu: `MainMenu.PlayGame`, the controls page, `DeleteSave`, `ExitGame`.
    Play,
    Controls,
    DeleteSave,
    Exit,
    /// Back from the controls page to the start menu.
    BackToStart,
    /// Pause menu: `PauseMenu.Menu` (save, then the start menu).
    ToStartMenu,
    /// Death screen: `PlayerData.Respawn`, now rather than at the count's end.
    Respawn,
}

/// Which menu is open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuKind {
    /// `confirm_delete`: Delete Save was chosen once and asks to be chosen
    /// again. Unity deleted on the first press.
    Start { confirm_delete: bool },
    Controls,
    Pause,
    Upgrades,
    /// `DeathMenu`: real seconds left before the respawn.
    Death { seconds_left: f32 },
}

/// `DeathMenu.Died` and `PlayerData.RespawnRoutine`: back after 5 seconds.
pub const DEATH_SECONDS: f32 = 5.0;

/// The controls page's lines, after Unity's "Move", "Shoot" and "Interact key"
/// panels, with this build's keys.
const CONTROLS: [&str; 6] = [
    "Move  W A S D",
    "Shoot  arrows or mouse",
    "Space  pick up, use, read on",
    "1 2 3 4 Q E  guns",
    "Tab  map",
    "Esc  pause",
];

/// Quitting is left to the system on Android, where an app does not close itself.
const CAN_QUIT: bool = !cfg!(target_os = "android");

/// An open menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Menu {
    pub kind: MenuKind,
    pub selected: usize,
}

impl Menu {
    pub fn new(kind: MenuKind) -> Self {
        // The controls page is read, not chosen from: its way back is lit.
        let selected = if kind == MenuKind::Controls { CONTROLS.len() } else { 0 };
        Self { kind, selected }
    }

    /// A menu by name, for `--menu`: start, controls, pause, upgrades, death.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "start" => Self::start(),
            "controls" => Self::new(MenuKind::Controls),
            "pause" => Self::new(MenuKind::Pause),
            "upgrades" => Self::upgrades(),
            "death" => Self::death(),
            _ => return None,
        })
    }

    pub fn upgrades() -> Self {
        Self::new(MenuKind::Upgrades)
    }

    pub fn start() -> Self {
        Self::new(MenuKind::Start { confirm_delete: false })
    }

    pub fn death() -> Self {
        Self::new(MenuKind::Death { seconds_left: DEATH_SECONDS })
    }

    /// Whether Escape or a pad's B leaves it: not the start menu, which has
    /// nowhere to go back to, nor the death screen (`ResumeShowMenu` checks
    /// `isDead`).
    pub fn can_go_back(&self) -> bool {
        matches!(self.kind, MenuKind::Controls | MenuKind::Pause | MenuKind::Upgrades)
    }

    /// The rows' choices, top to bottom.
    fn choices(&self) -> &'static [Choice] {
        match self.kind {
            MenuKind::Start { .. } if CAN_QUIT => &[Choice::Play, Choice::Controls, Choice::DeleteSave, Choice::Exit],
            MenuKind::Start { .. } => &[Choice::Play, Choice::Controls, Choice::DeleteSave],
            MenuKind::Controls => &[Choice::Nothing; CONTROLS.len() + 1],
            MenuKind::Pause if CAN_QUIT => &[Choice::Close, Choice::ToStartMenu, Choice::Exit],
            MenuKind::Pause => &[Choice::Close, Choice::ToStartMenu],
            MenuKind::Upgrades => &[Choice::Buy(Upgrade::Crit), Choice::Buy(Upgrade::Health), Choice::Buy(Upgrade::Move), Choice::Close],
            MenuKind::Death { .. } => &[Choice::Respawn],
        }
    }

    pub fn row_count(&self) -> usize {
        self.choices().len()
    }

    /// Moves the highlight by `rows`, wrapping.
    pub fn move_by(&mut self, rows: i32) {
        let count = self.row_count() as i32;
        self.selected = (self.selected as i32 + rows).rem_euclid(count) as usize;
    }

    pub fn choice(&self, row: usize) -> Choice {
        match self.kind {
            // The page's last row is its way back.
            MenuKind::Controls if row == CONTROLS.len() => Choice::BackToStart,
            _ => self.choices().get(row).copied().unwrap_or(Choice::Nothing),
        }
    }

    /// What it shows. The upgrade rows are `UpgradeMenu.updateStats`'s: the
    /// stat as it stands and the next one's price.
    pub fn view(&self, world: &World) -> MenuView {
        let row = |label: &str| MenuRow { label: label.to_string(), price: None, enabled: true };
        let (title, note, rows, opaque) = match self.kind {
            MenuKind::Start { confirm_delete } => {
                let delete = if confirm_delete { "Sure? Delete Save" } else { "Delete Save" };
                let mut rows = vec![row("Play"), row("Controls"), row(delete)];
                if CAN_QUIT {
                    rows.push(row("Exit"));
                }
                ("Cycle Of Steel", None, rows, true)
            }
            MenuKind::Controls => {
                let mut rows: Vec<MenuRow> =
                    CONTROLS.iter().map(|&line| MenuRow { label: line.to_string(), price: None, enabled: false }).collect();
                rows.push(row("Back"));
                ("Controls", None, rows, true)
            }
            MenuKind::Pause => {
                let mut rows = vec![row("Resume"), row("Menu")];
                if CAN_QUIT {
                    rows.push(row("Quit"));
                }
                ("Paused", None, rows, false)
            }
            MenuKind::Upgrades => {
                let player = &world.player;
                let mut rows: Vec<MenuRow> = Upgrade::ALL
                    .iter()
                    .map(|&upgrade| {
                        let label = match upgrade {
                            Upgrade::Crit => format!("Crit {}%", short(player.crit_chance * 100.0)),
                            Upgrade::Health => format!("Hp  {}", short(player.max_health)),
                            Upgrade::Move => format!("Move {}", short(player.move_speed)),
                        };
                        let price = world.upgrades.price(upgrade);
                        MenuRow { label, price: Some(price), enabled: world.data_cells >= price }
                    })
                    .collect();
                rows.push(row("Back"));
                ("System Upgrade", None, rows, false)
            }
            MenuKind::Death { seconds_left } => {
                // `DeathMenu.Timer`: 4 down to 0, a second each.
                let count = (seconds_left.ceil() as i32 - 1).max(0);
                ("You Died", Some(count.to_string()), vec![row("Respawn")], false)
            }
        };
        MenuView { title, note, rows, selected: self.selected, opaque }
    }
}

/// A number as C#'s `ToString` would show these: no trailing zeros, and the
/// float error of `0.05 * 100` hidden.
fn short(value: f32) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    let text = format!("{rounded:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_as_unity_shows_them() {
        assert_eq!(short(0.05 * 100.0), "5");
        assert_eq!(short(1000.0), "1000");
        assert_eq!(short(13.75), "13.75");
        assert_eq!(short(0.0), "0");
    }

    #[test]
    fn the_highlight_wraps_and_the_last_row_closes() {
        let mut menu = Menu::upgrades();
        menu.move_by(-1);
        assert_eq!(menu.selected, 3);
        assert_eq!(menu.choice(menu.selected), Choice::Close);
        menu.move_by(2);
        assert_eq!(menu.choice(menu.selected), Choice::Buy(Upgrade::Health));
    }

    #[test]
    fn the_controls_page_is_read_and_left_by_its_last_row() {
        let menu = Menu::new(MenuKind::Controls);
        assert_eq!(menu.choice(0), Choice::Nothing);
        assert_eq!(menu.choice(menu.row_count() - 1), Choice::BackToStart);
        assert_eq!(menu.choice(menu.selected), Choice::BackToStart, "Back is lit");
        assert!(menu.can_go_back());
        assert!(!Menu::start().can_go_back() && !Menu::death().can_go_back());
    }
}
