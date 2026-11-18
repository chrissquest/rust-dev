//! Turning the world into sprites.
//!
//! Everything is drawn from the one atlas `art` builds. The camera follows the
//! player and carries the world's screen shake. Feel is decided here as much as
//! in the sim: numbers pop as they appear, hits flash dummies white, crits are
//! gold and bigger, and bursts on one dummy show a climbing total under it.

use anyhow::Result;
use crystal_engine::camera2d::Camera2d;
use crystal_engine::glam::Vec2;
use crystal_engine::render2d::{DrawList, Pass, Renderer2d, Target, TextureId};
use crystal_engine::scene::{Texture, TextureFilter};
use crystal_engine::sprite::Sprite;

use crate::art::{Art, Atlas, DIGIT_SIZE, PIXELS_PER_TILE, Region};
use crate::sim::feedback::{NUMBER_LIFE, NumberKind, SparkKind};
use crate::sim::{World, gun, target};
use crate::touch::StickVisual;

/// Tiles from the bottom of the screen to the top.
const VIEW_HEIGHT: f32 = 12.0;
// Colours as sRGB hex: tints multiply texels after they are decoded to
// linear light, so they go through `color` first.
const BACKGROUND: u32 = 0x07090b;
const FLOOR_LIGHT: u32 = 0x23262e;
const FLOOR_DARK: u32 = 0x1d2027;
const WALL: u32 = 0x4a505c;
const NUMBER_WHITE: u32 = 0xffffff;
const NUMBER_GOLD: u32 = 0xffc533;
const COMBO_TOTAL: u32 = 0xbfe3ff;
const CRIT_BULLET: u32 = 0xffa060;
const OUTLINE: u32 = 0x05060a;

/// An sRGB colour as linear RGBA, opaque.
fn color(hex: u32) -> [f32; 4] {
    let channel = |shift: u32| {
        let c = ((hex >> shift) & 0xff) as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    [channel(16), channel(8), channel(0), 1.0]
}

/// The same with alpha.
fn faded(hex: u32, alpha: f32) -> [f32; 4] {
    let mut rgba = color(hex);
    rgba[3] = alpha;
    rgba
}

// Depths, back to front.
const Z_FLOOR: f32 = 0.0;
const Z_SHADOW: f32 = 0.5;
const Z_ENTITIES: f32 = 1.0;
const Z_GUN: f32 = 1.2;
const Z_BULLETS: f32 = 2.0;
const Z_SPARKS: f32 = 2.5;
const Z_NUMBERS: f32 = 5.0;

pub struct View {
    texture: TextureId,
    atlas: Atlas,
    list: DrawList,
    overlay: DrawList,
}

impl View {
    pub fn new(renderer: &mut Renderer2d) -> Result<Self> {
        let atlas = Atlas::build();
        let texture = renderer.add_texture(&Texture {
            name: "art".to_string(),
            width: atlas.width,
            height: atlas.height,
            rgba: atlas.rgba.clone(),
            filter: TextureFilter::Nearest,
        });
        Ok(Self {
            texture,
            atlas,
            list: DrawList::new(),
            overlay: DrawList::new(),
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
        sticks: &[Option<StickVisual>; 2],
    ) {
        let player = &world.player;
        let position = player.previous.lerp(player.position, blend);
        let lead = player.previous_lead.lerp(player.lead, blend);
        let focus = position + lead + world.feedback.shake_offset(world.tick);
        let camera = Camera2d::new(focus, VIEW_HEIGHT);
        let aspect = target.width as f32 / target.height.max(1) as f32;

        self.list.clear();
        self.draw_room();
        self.draw_targets(world, blend);
        self.draw_player(world, position);
        self.draw_bullets(world, blend);
        self.draw_sparks(world, blend);
        self.draw_numbers(world, blend);

        renderer.begin_frame();
        let pass = Pass {
            view_projection: camera.view_projection(aspect),
            clear: Some(color(BACKGROUND)),
            viewport: None,
        };
        let (width, height) = (target.width, target.height);
        renderer.draw(encoder, target, &pass, &self.list);

        // Touch sticks on top, in window pixels.
        self.overlay.clear();
        for stick in sticks.iter().flatten() {
            self.draw_stick(stick, height as f32);
        }
        if self.overlay.sprite_count() > 0 {
            let pass = Pass {
                view_projection: Camera2d::pixels(width as f32, height as f32).view_projection(),
                clear: None,
                viewport: None,
            };
            renderer.draw(encoder, target, &pass, &self.overlay);
        }
    }

    /// A ring of dots for the base and a diamond for the knob. Window y runs
    /// down; the pixel camera's runs up.
    fn draw_stick(&mut self, stick: &StickVisual, height: f32) {
        let flip = |v: Vec2| Vec2::new(v.x, height - v.y);
        let white = self.atlas.region(Art::White);
        let (ring, knob) = if stick.held { (0.55, 0.85) } else { (0.25, 0.35) };
        let dot = (stick.radius * 0.07).max(3.0);
        for i in 0..24 {
            let at = flip(stick.base) + Vec2::from_angle(i as f32 / 24.0 * std::f32::consts::TAU) * stick.radius;
            let sprite = Sprite::new(at.extend(0.0), [dot, dot], white.uv).with_color(faded(0xffffff, ring));
            self.overlay.sprite(self.texture, sprite);
        }
        let size = stick.radius * 0.5;
        let mut sprite = Sprite::new(flip(stick.knob).extend(0.1), [size, size], white.uv).with_color(faded(0xffffff, knob));
        sprite.rotation = std::f32::consts::FRAC_PI_4;
        self.overlay.sprite(self.texture, sprite);
    }

    fn put(&mut self, region: Region, center: Vec2, z: f32, scale: [f32; 2], rotation: f32, color: [f32; 4]) {
        let mut sprite = Sprite::new(center.extend(z), [region.size[0] * scale[0], region.size[1] * scale[1]], region.uv);
        sprite.rotation = rotation;
        sprite.color = color;
        self.list.sprite(self.texture, sprite);
    }

    fn square(&mut self, center: Vec2, size: f32, z: f32, color: [f32; 4]) {
        let region = self.atlas.region(Art::White);
        let scale = size * PIXELS_PER_TILE;
        self.put(region, center, z, [scale, scale], 0.0, color);
    }

    fn draw_room(&mut self) {
        let half = crate::sim::ROOM_HALF;
        let (columns, rows) = (half.x as i32, half.y as i32);
        for y in -rows - 1..=rows {
            for x in -columns - 1..=columns {
                let inside = x >= -columns && x < columns && y >= -rows && y < rows;
                let tile = if !inside {
                    WALL
                } else if (x + y).rem_euclid(2) == 0 {
                    FLOOR_LIGHT
                } else {
                    FLOOR_DARK
                };
                self.square(Vec2::new(x as f32 + 0.5, y as f32 + 0.5), 1.0, Z_FLOOR, color(tile));
            }
        }
    }

    fn draw_targets(&mut self, world: &World, blend: f32) {
        let shadow = self.atlas.region(Art::Shadow);
        for dummy in &world.targets {
            let at = dummy.previous.lerp(dummy.position, blend);
            let feet = Vec2::new(dummy.home.x, dummy.home.y - target::HALF_SIZE.y);
            self.put(shadow, feet, Z_SHADOW, [1.0, 1.0], 0.0, Sprite::WHITE);
            // Lean away from the push: the spring's stretch shows as a tilt.
            let lean = ((dummy.home.x - at.x) * 0.6).clamp(-0.5, 0.5);
            let region = if dummy.flash > 0.0 { self.atlas.silhouette(Art::Dummy) } else { self.atlas.region(Art::Dummy) };
            self.put(region, at, Z_ENTITIES, [1.0, 1.0], lean, Sprite::WHITE);

            if dummy.combo_hits >= 2 {
                let fade = (dummy.combo_timer / 0.25).clamp(0.0, 1.0);
                let pop = 1.0 + 0.5 * (1.0 - dummy.combo_age / 0.1).max(0.0);
                // Under the feet, clear of the numbers flying up.
                let below = feet - Vec2::new(0.0, 0.45);
                self.number(dummy.combo_total, below, 1.4 * pop, faded(COMBO_TOTAL, fade));
            }
        }
    }

    fn draw_player(&mut self, world: &World, at: Vec2) {
        let player = &world.player;
        let aim = player.aim;
        let facing = if aim.x < 0.0 { -1.0 } else { 1.0 };
        self.put(self.atlas.region(Art::Shadow), at - Vec2::new(0.0, 0.42), Z_SHADOW, [1.0, 1.0], 0.0, Sprite::WHITE);
        self.put(self.atlas.region(Art::Player), at, Z_ENTITIES, [facing, 1.0], 0.0, Sprite::WHITE);

        // The gun turns with the aim, flipped when pointing left so it is never
        // upside down, and kicks back along the aim after each shot.
        let angle = aim.y.atan2(aim.x);
        let hand = at + Vec2::new(0.0, -0.12) + aim * (0.3 - 0.12 * world.gun.kick);
        self.put(self.atlas.region(Art::Gun), hand, Z_GUN, [1.0, facing], angle, Sprite::WHITE);
        if world.gun.flash > 0.0 {
            let muzzle = at + aim * (gun::BARREL + 0.1);
            self.put(self.atlas.region(Art::Muzzle), muzzle, Z_SPARKS, [1.0, 1.0], angle, Sprite::WHITE);
        }
    }

    fn draw_bullets(&mut self, world: &World, blend: f32) {
        let region = self.atlas.region(Art::Bullet);
        for bullet in &world.bullets {
            let at = bullet.previous.lerp(bullet.position, blend);
            let angle = bullet.velocity.y.atan2(bullet.velocity.x);
            let (scale, tint) = if bullet.crit { (1.5, color(CRIT_BULLET)) } else { (1.0, Sprite::WHITE) };
            self.put(region, at, Z_BULLETS, [scale, scale], angle, tint);
        }
    }

    fn draw_sparks(&mut self, world: &World, blend: f32) {
        for spark in &world.feedback.sparks {
            let at = spark.previous.lerp(spark.position, blend);
            let left = 1.0 - spark.age / spark.life;
            let (size, hex) = match spark.kind {
                SparkKind::Impact => (2.0, 0xfff2b0),
                SparkKind::Crit => (3.0, 0xffc533),
                SparkKind::Wall => (1.0, 0xb4b8c4),
            };
            self.square(at, size / PIXELS_PER_TILE, Z_SPARKS, faded(hex, left));
        }
    }

    fn draw_numbers(&mut self, world: &World, blend: f32) {
        for number in &world.feedback.numbers {
            let at = number.previous.lerp(number.position, blend);
            // Pop in large and settle; fade out at the end.
            let pop = 1.0 + 0.9 * (1.0 - number.age / 0.12).max(0.0);
            let (size, hex) = match number.kind {
                NumberKind::Normal => (1.3, NUMBER_WHITE),
                NumberKind::Crit => (2.4, NUMBER_GOLD),
            };
            let alpha = ((NUMBER_LIFE - number.age) / 0.3).clamp(0.0, 1.0);
            self.number(number.value, at, size * pop, faded(hex, alpha));
        }
    }

    /// Draws `value` centred on `center`, each font pixel `scale` art pixels,
    /// with a one-pixel dark outline.
    fn number(&mut self, value: u32, center: Vec2, scale: f32, color: [f32; 4]) {
        let mut digits = [0u32; 10];
        let mut count = 0;
        let mut rest = value;
        loop {
            digits[count] = rest % 10;
            count += 1;
            rest /= 10;
            if rest == 0 || count == digits.len() {
                break;
            }
        }
        let pixel = scale / PIXELS_PER_TILE;
        let advance = (DIGIT_SIZE[0] + 1) as f32 * pixel;
        let width = count as f32 * advance - pixel;
        let outline = faded(OUTLINE, color[3]);
        for i in 0..count {
            let digit = digits[count - 1 - i];
            let x = center.x - width * 0.5 + i as f32 * advance + DIGIT_SIZE[0] as f32 * pixel * 0.5;
            let at = Vec2::new(x, center.y);
            let region = self.atlas.digit(digit);
            for offset in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                self.put(region, at + offset * pixel, Z_NUMBERS, [scale, scale], 0.0, outline);
            }
            self.put(region, at, Z_NUMBERS + 0.01, [scale, scale], 0.0, color);
        }
    }
}
