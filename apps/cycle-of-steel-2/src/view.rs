//! Turning the world into sprites.
//!
//! Placeholder drawing: everything is a tinted square from one white texel, a
//! checkered floor and the player. Cycle of Steel 1's sprites come in with the
//! parts prototype; the art pass replaces them later.

use anyhow::Result;
use crystal_engine::camera2d::Camera2d;
use crystal_engine::glam::Vec2;
use crystal_engine::render2d::{DrawList, Pass, Renderer2d, Target, TextureId};
use crystal_engine::scene::{Texture, TextureFilter};
use crystal_engine::sprite::Sprite;

use crate::sim::{Player, World};

/// Tiles from the bottom of the screen to the top.
const VIEW_HEIGHT: f32 = 12.0;
/// Half the placeholder floor's width and height, in tiles.
const FLOOR_HALF: i32 = 12;
const BACKGROUND: [f32; 4] = [0.04, 0.04, 0.06, 1.0];
const FLOOR_LIGHT: [f32; 4] = [0.16, 0.16, 0.2, 1.0];
const FLOOR_DARK: [f32; 4] = [0.13, 0.13, 0.17, 1.0];
const PLAYER: [f32; 4] = [0.95, 0.65, 0.2, 1.0];

pub struct View {
    white: TextureId,
    list: DrawList,
}

impl View {
    pub fn new(renderer: &mut Renderer2d) -> Result<Self> {
        let white = renderer.add_texture(&Texture {
            name: "white".to_string(),
            width: 1,
            height: 1,
            rgba: vec![255; 4],
            filter: TextureFilter::Nearest,
        });
        Ok(Self {
            white,
            list: DrawList::new(),
        })
    }

    /// Draws the world, `blend` of the way from the previous step to the newest.
    pub fn draw(
        &mut self,
        renderer: &mut Renderer2d,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        world: &World,
        blend: f32,
    ) {
        let player = &world.player;
        let position = player.previous.lerp(player.position, blend);
        let camera = Camera2d::new(position, VIEW_HEIGHT);
        let aspect = target.width as f32 / target.height.max(1) as f32;

        self.list.clear();
        for y in -FLOOR_HALF..FLOOR_HALF {
            for x in -FLOOR_HALF..FLOOR_HALF {
                let color = if (x + y).rem_euclid(2) == 0 { FLOOR_LIGHT } else { FLOOR_DARK };
                self.square(Vec2::new(x as f32 + 0.5, y as f32 + 0.5), 1.0, 0.0, color);
            }
        }
        self.square(position, Player::HALF_SIZE * 2.0, 1.0, PLAYER);

        renderer.begin_frame();
        let pass = Pass {
            view_projection: camera.view_projection(aspect),
            clear: Some(BACKGROUND),
            viewport: None,
        };
        renderer.draw(encoder, target, &pass, &self.list);
    }

    fn square(&mut self, center: Vec2, size: f32, height: f32, color: [f32; 4]) {
        let mut sprite = Sprite::new(center.extend(height), [size, size], [0.0, 0.0, 1.0, 1.0]);
        sprite.color = color;
        self.list.sprite(self.white, sprite);
    }
}
