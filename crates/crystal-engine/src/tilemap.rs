//! Tile layers: a grid of cells, each showing one cell of a tileset.
//!
//! A layer is turned into sprites once, when a room loads, and `render2d` keeps
//! them as a static instance buffer: drawing a layer is then one call however many
//! tiles it has. Rows are stored top row first, which is how a level reads on
//! screen and in a text file; world y still points up.

use crate::math::Vec3;
use crate::sprite::{Grid, Sprite};

/// Marks a cell with nothing in it.
pub const EMPTY: u16 = u16::MAX;

#[derive(Clone, Debug, PartialEq)]
pub struct TileLayer {
    pub width: u32,
    pub height: u32,
    /// `width * height` tileset cell indices, top row first; [`EMPTY`] for none.
    pub tiles: Vec<u16>,
}

impl TileLayer {
    /// A layer with every cell empty.
    pub fn empty(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            tiles: vec![EMPTY; width as usize * height as usize],
        }
    }

    /// The tile at column `x`, row `y` counted from the *bottom* (world-style), or
    /// `None` when the cell is empty or outside the layer.
    pub fn get(&self, x: i32, y: i32) -> Option<u16> {
        let index = self.index(x, y)?;
        self.tiles.get(index).copied().filter(|&tile| tile != EMPTY)
    }

    /// Sets the tile at column `x`, row `y` from the bottom. Outside the layer is a no-op.
    pub fn set(&mut self, x: i32, y: i32, tile: u16) {
        if let Some(index) = self.index(x, y) {
            self.tiles[index] = tile;
        }
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        let row_from_top = self.height as usize - 1 - y as usize;
        Some(row_from_top * self.width as usize + x as usize)
    }

    /// One sprite per non-empty cell. `origin` is the world position of the layer's
    /// bottom-left corner (its z is the layer's height); `cell` is the world size of
    /// one cell.
    pub fn sprites(&self, tileset: &Grid, origin: Vec3, cell: f32) -> Vec<Sprite> {
        let mut sprites = Vec::with_capacity(self.tiles.iter().filter(|&&tile| tile != EMPTY).count());
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let Some(tile) = self.get(x, y) else {
                    continue;
                };
                let position = origin + Vec3::new(x as f32 * cell, y as f32 * cell, 0.0);
                sprites.push(
                    Sprite::new(position, [cell, cell], tileset.uv(tile as u32)).with_pivot([0.0, 0.0]),
                );
            }
        }
        sprites
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprite::Sheet;

    #[test]
    fn rows_are_stored_top_first_but_addressed_from_the_bottom() {
        let layer = TileLayer {
            width: 2,
            height: 2,
            tiles: vec![1, 2, 3, EMPTY],
        };
        assert_eq!(layer.get(0, 1), Some(1));
        assert_eq!(layer.get(1, 1), Some(2));
        assert_eq!(layer.get(0, 0), Some(3));
        assert_eq!(layer.get(1, 0), None);
        assert_eq!(layer.get(2, 0), None);
        assert_eq!(layer.get(0, -1), None);
    }

    #[test]
    fn sprites_skip_empty_cells_and_sit_on_the_grid() {
        let mut layer = TileLayer::empty(3, 2);
        layer.set(2, 1, 0);
        let tileset = Sheet::new(64, 64, 64.0).grid(32, 32);
        let sprites = layer.sprites(&tileset, Vec3::new(10.0, 20.0, 0.5), 0.5);
        assert_eq!(sprites.len(), 1);
        assert_eq!(sprites[0].position, [11.0, 20.5, 0.5]);
        assert_eq!(sprites[0].pivot, [0.0, 0.0]);
        assert_eq!(sprites[0].size, [0.5, 0.5]);
    }
}
