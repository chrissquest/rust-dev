//! The placeholder menus: what each shows and what choosing a row does.
//!
//! A menu pauses the game while it is open, as Unity's menus set
//! `Time.timeScale` to 0: the app stops stepping the world and hands keys,
//! taps and buttons here instead. Only the upgrade bench's menu so far. Their
//! look is redone in the UI pass.

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

/// A menu as drawn: its title, rows, and the one highlighted.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuView {
    pub title: &'static str,
    pub rows: Vec<MenuRow>,
    pub selected: usize,
}

/// What a menu asks for when a row is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Buy(Upgrade),
    Close,
}

/// An open menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Menu {
    pub selected: usize,
}

impl Menu {
    /// `UpgradeMenu`: crit, health and speed for data cells, then a way out.
    pub fn upgrades() -> Self {
        Self { selected: 0 }
    }

    pub fn row_count(&self) -> usize {
        Upgrade::ALL.len() + 1
    }

    /// Moves the highlight by `rows`, wrapping.
    pub fn move_by(&mut self, rows: i32) {
        let count = self.row_count() as i32;
        self.selected = (self.selected as i32 + rows).rem_euclid(count) as usize;
    }

    pub fn choice(&self, row: usize) -> Choice {
        match Upgrade::ALL.get(row) {
            Some(&upgrade) => Choice::Buy(upgrade),
            None => Choice::Close,
        }
    }

    /// The rows with `UpgradeMenu.updateStats`'s texts: the stat as it stands
    /// and the next one's price.
    pub fn view(&self, world: &World) -> MenuView {
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
        rows.push(MenuRow { label: "Back".into(), price: None, enabled: true });
        MenuView { title: "Upgrades", rows, selected: self.selected }
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
}
