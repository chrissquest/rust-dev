//! World state to sprites, and the camera that looks at them.
//!
//! The view never changes the world. Each frame it reads the last two simulation
//! steps and blends between them, so drawing at 144 Hz shows motion between the
//! 60 Hz steps instead of repeating them.

use anyhow::Result;
use crystal_engine::camera2d::Camera2d;
use crystal_engine::glam::{Vec2, Vec3};
use crystal_engine::render2d::{BatchId, DrawList, Pass, Renderer2d, Target, TextureId};
use crystal_engine::scene::{Texture, TextureFilter};
use crystal_engine::sprite::{Grid, Sheet, Sprite};

use crate::assets::{AssetSource, PIXELS_PER_UNIT};
use crate::input::StickVisual;
use crate::sim::World;
use crate::sim::player::Facing;
use crate::sim::room::{self, Room};

/// World units from the bottom of the screen to the top: Unity's orthographic
/// size of 2.11506, doubled. A 16:9 window shows about 7.5 of a room's 16 units.
const VIEW_HEIGHT: f32 = 2.0 * 2.11506;
/// How fast the camera catches up with where it should be, in units per second.
/// Fast enough to be invisible inside a room; it shows when changing rooms.
const CAMERA_SPEED: f32 = 40.0;

/// Heights of the draw layers, following Unity's sorting layers (Background,
/// Default, Entities, Bullets). Under the straight-down camera only their order
/// matters.
const Z_TILES: f32 = 0.0;
const Z_ENTITIES: f32 = 0.1;

const CLEAR_COLOR: [f32; 4] = [0.012, 0.012, 0.016, 1.0];

/// The on-screen stick: a faint base with a brighter knob a third of its size.
const STICK_BASE_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.16];
const STICK_KNOB_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.42];
const STICK_KNOB_SCALE: f32 = 0.38;
/// Side of the generated disc texture, in pixels.
const DISC_PIXELS: u32 = 128;

const TILE_PIXELS: u32 = 32;
const PLAYER_FRAME_PIXELS: u32 = 32;

struct PlayerSheets {
    front: TextureId,
    back: TextureId,
    left: TextureId,
    right: TextureId,
    /// All four sheets share one layout: a row of 32 px frames.
    frames: Grid,
}

pub struct View {
    tiles: TextureId,
    tileset: Grid,
    player: PlayerSheets,

    /// The current room's tile layers, uploaded once.
    room_batches: Vec<BatchId>,
    camera: Camera2d,
    /// False until the first frame, which places the camera without easing.
    camera_placed: bool,
    list: DrawList,

    /// A soft white disc, for the touch stick.
    disc: TextureId,
    overlay: DrawList,
}

impl View {
    /// Loads the textures every room needs.
    pub fn new(renderer: &mut Renderer2d, assets: &AssetSource) -> Result<Self> {
        let tiles_texture = assets.sprite("main_dark_compound")?;
        let tileset = Sheet::of(&tiles_texture, PIXELS_PER_UNIT).grid(TILE_PIXELS, TILE_PIXELS);
        let tiles = renderer.add_texture(&tiles_texture);

        let mut frames = None;
        let mut load_player = |name: &str| -> Result<TextureId> {
            let texture = assets.sprite(name)?;
            frames.get_or_insert(
                Sheet::of(&texture, PIXELS_PER_UNIT).grid(PLAYER_FRAME_PIXELS, PLAYER_FRAME_PIXELS),
            );
            Ok(renderer.add_texture(&texture))
        };
        let front = load_player("player_run_front")?;
        let back = load_player("player_run_back")?;
        let left = load_player("player_run_left")?;
        let right = load_player("player_run_right")?;
        let frames = frames.expect("four sheets were loaded");
        let disc = renderer.add_texture(&disc_texture(DISC_PIXELS));

        Ok(Self {
            tiles,
            tileset,
            player: PlayerSheets {
                front,
                back,
                left,
                right,
                frames,
            },
            room_batches: Vec::new(),
            camera: Camera2d::new(Vec2::ZERO, VIEW_HEIGHT),
            camera_placed: false,
            list: DrawList::new(),
            disc,
            overlay: DrawList::new(),
        })
    }

    /// Replaces the uploaded tile layers with `room`'s. Call when the room changes.
    pub fn set_room(&mut self, renderer: &mut Renderer2d, room: &Room) {
        for batch in self.room_batches.drain(..) {
            renderer.remove_batch(batch);
        }
        for layer in &room.layers {
            let sprites = layer
                .tiles
                .sprites(&self.tileset, Vec3::new(0.0, 0.0, Z_TILES), room::CELL);
            self.room_batches.push(renderer.add_batch(self.tiles, &sprites));
        }
    }

    /// Records one frame of `world` into `target`.
    ///
    /// `blend` is how far the frame is between the previous simulation step (0) and
    /// the current one (1); `frame_seconds` is the time since the last drawn frame.
    /// `stick` is the on-screen stick to draw over the world, if a finger holds one.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        renderer: &mut Renderer2d,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        world: &World,
        blend: f32,
        frame_seconds: f32,
        stick: Option<StickVisual>,
    ) {
        let aspect = target.width.max(1) as f32 / target.height.max(1) as f32;
        let player_position = world
            .player
            .previous_position
            .lerp(world.player.position, blend.clamp(0.0, 1.0));

        self.follow(player_position, aspect, frame_seconds);

        self.list.clear();
        for &batch in &self.room_batches {
            self.list.batch(batch);
        }

        let sheet = match world.player.facing {
            Facing::Front => self.player.front,
            Facing::Back => self.player.back,
            Facing::Left => self.player.left,
            Facing::Right => self.player.right,
        };
        self.list.sprite(
            sheet,
            Sprite::new(
                player_position.extend(Z_ENTITIES),
                self.player.frames.cell_size(),
                self.player.frames.uv(world.player.animation_frame()),
            ),
        );

        renderer.begin_frame();
        renderer.draw(
            encoder,
            target,
            &Pass {
                view_projection: self.camera.view_projection(aspect),
                clear: Some(CLEAR_COLOR),
                viewport: None,
            },
            &self.list,
        );

        if let Some(stick) = stick {
            self.draw_stick(renderer, encoder, target, stick);
        }
    }

    /// A second pass in pixel space, over the finished world.
    fn draw_stick(
        &mut self,
        renderer: &mut Renderer2d,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        stick: StickVisual,
    ) {
        let (width, height) = (target.width as f32, target.height as f32);
        // The stick is in window pixels (y down); the pixel camera's y points up.
        let flip = |point: Vec2| Vec3::new(point.x, height - point.y, 0.0);
        let full = [0.0, 0.0, 1.0, 1.0];
        let base_size = stick.radius * 2.0;
        let knob_size = base_size * STICK_KNOB_SCALE;

        self.overlay.clear();
        self.overlay.sprites(
            self.disc,
            &[
                Sprite::new(flip(stick.base), [base_size, base_size], full).with_color(STICK_BASE_COLOR),
                Sprite::new(flip(stick.knob), [knob_size, knob_size], full).with_color(STICK_KNOB_COLOR),
            ],
        );
        renderer.draw(
            encoder,
            target,
            &Pass {
                view_projection: Camera2d::pixels(width, height).view_projection(),
                clear: None,
                viewport: None,
            },
            &self.overlay,
        );
    }

    /// Moves the camera towards the player, kept inside the room. Same rule as
    /// Unity's `CameraController`: clamp to the room, then approach at a fixed speed.
    fn follow(&mut self, player: Vec2, aspect: f32, frame_seconds: f32) {
        let mut wanted = Camera2d::new(player, VIEW_HEIGHT);
        wanted.clamp_to(Vec2::ZERO, Vec2::new(room::WIDTH, room::HEIGHT), aspect);

        if self.camera_placed {
            let step = CAMERA_SPEED * frame_seconds.max(0.0);
            self.camera.center = self.camera.center.move_towards(wanted.center, step);
        } else {
            self.camera.center = wanted.center;
            self.camera_placed = true;
        }
    }
}

/// A white disc with a soft edge on a transparent background, sampled smoothly so
/// it stays round at any size.
fn disc_texture(size: u32) -> Texture {
    let radius = size as f32 * 0.5;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let offset = Vec2::new(x as f32 + 0.5 - radius, y as f32 + 0.5 - radius);
            // Opaque inside, fading out over the last two pixels before the rim.
            let alpha = ((radius - offset.length()) / 2.0).clamp(0.0, 1.0);
            rgba.extend_from_slice(&[255, 255, 255, (alpha * 255.0).round() as u8]);
        }
    }
    Texture {
        name: "disc".to_string(),
        width: size,
        height: size,
        rgba,
        filter: TextureFilter::Linear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disc_is_solid_in_the_middle_and_clear_in_the_corners() {
        let disc = disc_texture(32);
        let alpha = |x: u32, y: u32| disc.rgba[((y * 32 + x) * 4 + 3) as usize];
        assert_eq!(alpha(16, 16), 255);
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(31, 31), 0);
        assert_eq!(disc.rgba.len(), 32 * 32 * 4);
    }
}
