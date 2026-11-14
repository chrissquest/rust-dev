//! Where the on-screen controls and readouts sit.
//!
//! Both the view (to draw them) and the touch input (to hit-test them) need the
//! same rectangles, so the layout lives here and neither owns it. Everything is in
//! pixels from the screen's top-left corner, sized from the screen's height so it
//! looks the same on a phone and a monitor.
//!
//! This is the placeholder HUD: plain boxes, enough to play with. Its look is
//! redone in the UI pass.

use crystal_engine::glam::Vec2;

use crate::sim::INVENTORY_SLOTS;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn contains(&self, point: Vec2) -> bool {
        point.cmpge(self.min).all() && point.cmple(self.max).all()
    }

    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudLayout {
    /// The inventory slots, left to right, along the top-left.
    pub slots: [Rect; INVENTORY_SLOTS],
    /// The gun-in-hand's heat, under the slots.
    pub heat_bar: Rect,
    /// The player's health, under the heat bar.
    pub health_bar: Rect,
    /// The data cell count: where its icon is centred, and the text's top-left
    /// corner and height in pixels per em.
    pub data_icon: Vec2,
    pub data_text: Vec2,
    pub text_size: f32,
    /// A boss's health, along the bottom middle, its name above it.
    pub boss_bar: Rect,
    /// The button that shows the minimap, top right, and where the map is drawn.
    pub map_button: Rect,
    pub map_area: Rect,
    /// The player's line, along the bottom over the boss bar (`PlayerUI.dialogueUI`).
    pub dialogue: Rect,
    /// The round "pick up" button on the right, shown when something is in reach.
    pub interact_center: Vec2,
    pub interact_radius: f32,
}

impl HudLayout {
    pub fn new(screen: Vec2) -> Self {
        let unit = screen.y.max(1.0);
        let margin = unit * 0.03;
        let side = unit * 0.085;
        let gap = unit * 0.012;

        let slots = std::array::from_fn(|index| {
            let min = Vec2::new(margin + index as f32 * (side + gap), margin);
            Rect {
                min,
                max: min + Vec2::splat(side),
            }
        });
        let heat_top = margin + side + gap;
        let heat_bar = Rect {
            min: Vec2::new(margin, heat_top),
            max: Vec2::new(slots[INVENTORY_SLOTS - 1].max.x, heat_top + unit * 0.014),
        };

        let health_top = heat_bar.max.y + gap;
        let health_bar = Rect {
            min: Vec2::new(margin, health_top),
            max: Vec2::new(heat_bar.max.x, health_top + unit * 0.022),
        };
        let text_size = unit * 0.06;
        let data_icon = Vec2::new(margin + text_size * 0.5, health_bar.max.y + gap + text_size * 0.5);
        Self {
            slots,
            heat_bar,
            health_bar,
            data_icon,
            data_text: Vec2::new(margin + text_size * 1.3, health_bar.max.y + gap),
            text_size,
            map_button: Rect {
                min: Vec2::new(screen.x - margin - unit * 0.13, margin),
                max: Vec2::new(screen.x - margin, margin + unit * 0.065),
            },
            map_area: Rect {
                min: Vec2::new(screen.x * 0.5 - unit * 0.6, unit * 0.15),
                max: Vec2::new(screen.x * 0.5 + unit * 0.6, unit * 0.85),
            },
            boss_bar: Rect {
                min: Vec2::new(screen.x * 0.25, screen.y - margin - unit * 0.03),
                max: Vec2::new(screen.x * 0.75, screen.y - margin),
            },
            dialogue: Rect {
                min: Vec2::new(screen.x * 0.5 - unit * 0.75, screen.y - margin - unit * 0.3),
                max: Vec2::new(screen.x * 0.5 + unit * 0.75, screen.y - margin - unit * 0.08),
            },
            interact_center: Vec2::new(screen.x - unit * 0.16, unit * 0.32),
            interact_radius: unit * 0.075,
        }
    }

    /// The slot under a point, if any.
    pub fn slot_at(&self, point: Vec2) -> Option<usize> {
        self.slots.iter().position(|slot| slot.contains(point))
    }

    pub fn interact_contains(&self, point: Vec2) -> bool {
        point.distance(self.interact_center) <= self.interact_radius
    }
}

/// A placeholder menu: a panel in the middle of the screen, its title on top and
/// its rows one under another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuLayout {
    pub panel: Rect,
    /// The title's top-left corner.
    pub title: Vec2,
    /// Pixels per em, for the title and the rows.
    pub text_size: f32,
    first_row: Rect,
    row_step: f32,
    rows: usize,
}

impl MenuLayout {
    pub fn new(screen: Vec2, rows: usize) -> Self {
        let unit = screen.y.max(1.0);
        let text_size = unit * 0.05;
        let row_height = unit * 0.09;
        let row_step = row_height * 1.15;
        let width = (unit * 0.9).min(screen.x * 0.9);
        let height = text_size * 2.5 + row_step * rows as f32 + text_size;
        let min = Vec2::new(screen.x * 0.5 - width * 0.5, screen.y * 0.5 - height * 0.5);
        let padding = text_size * 0.6;
        let first_top = min.y + text_size * 2.5;
        Self {
            panel: Rect { min, max: min + Vec2::new(width, height) },
            title: min + Vec2::splat(padding),
            text_size,
            first_row: Rect {
                min: Vec2::new(min.x + padding, first_top),
                max: Vec2::new(min.x + width - padding, first_top + row_height),
            },
            row_step,
            rows,
        }
    }

    pub fn row(&self, index: usize) -> Rect {
        let down = Vec2::new(0.0, self.row_step * index as f32);
        Rect { min: self.first_row.min + down, max: self.first_row.max + down }
    }

    /// The row under a point, if any.
    pub fn row_at(&self, point: Vec2) -> Option<usize> {
        (0..self.rows).find(|&index| self.row(index).contains(point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_rows_stack_inside_the_panel_and_are_found_by_a_tap() {
        let menu = MenuLayout::new(Vec2::new(1280.0, 720.0), 4);
        for index in 0..4 {
            let row = menu.row(index);
            assert!(menu.panel.contains(row.min) && menu.panel.contains(row.max));
            assert_eq!(menu.row_at(row.center()), Some(index));
        }
        assert!(menu.row(0).max.y < menu.row(1).min.y);
        assert_eq!(menu.row_at(Vec2::ZERO), None);
    }

    #[test]
    fn slots_sit_side_by_side_without_overlapping() {
        let hud = HudLayout::new(Vec2::new(1280.0, 720.0));
        for pair in hud.slots.windows(2) {
            assert!(pair[0].max.x < pair[1].min.x);
            assert_eq!(pair[0].min.y, pair[1].min.y);
        }
        assert_eq!(hud.slot_at(hud.slots[2].center()), Some(2));
        assert_eq!(hud.slot_at(Vec2::new(640.0, 360.0)), None);
        assert!(hud.heat_bar.min.y > hud.slots[0].max.y);
        assert_eq!(hud.heat_bar.size().x, hud.slots[3].max.x - hud.slots[0].min.x);
    }

    #[test]
    fn the_layout_scales_with_the_screen_height() {
        let small = HudLayout::new(Vec2::new(800.0, 360.0));
        let large = HudLayout::new(Vec2::new(1600.0, 720.0));
        assert_eq!(large.slots[0].size(), small.slots[0].size() * 2.0);
        assert!(large.interact_contains(large.interact_center));
        assert!(!large.interact_contains(large.interact_center + Vec2::splat(large.interact_radius)));
        // The button hugs the right edge whatever the width.
        assert!(large.interact_center.x > 1600.0 * 0.8);
    }
}
