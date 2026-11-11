//! World state to sprites, and the camera that looks at them.
//!
//! The view never changes the world. Each frame it reads the last two simulation
//! steps and blends between them, so drawing at 144 Hz shows motion between the
//! 60 Hz steps instead of repeating them.

use std::path::Path;

use anyhow::Result;
use crystal_engine::camera2d::Camera2d;
use crystal_engine::glam::{Vec2, Vec3};
use crystal_engine::render2d::{BatchId, DrawList, Pass, Renderer2d, Target, TextureId};
use crystal_engine::sprite::{Grid, Sheet, Sprite};

use crate::assets::{self, PIXELS_PER_UNIT};
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
}

impl View {
    /// Loads the textures every room needs.
    pub fn new(renderer: &mut Renderer2d, assets_dir: &Path) -> Result<Self> {
        let tiles_texture = assets::load_sprite(assets_dir, "main_dark_compound")?;
        let tileset = Sheet::of(&tiles_texture, PIXELS_PER_UNIT).grid(TILE_PIXELS, TILE_PIXELS);
        let tiles = renderer.add_texture(&tiles_texture);

        let mut frames = None;
        let mut load_player = |name: &str| -> Result<TextureId> {
            let texture = assets::load_sprite(assets_dir, name)?;
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
    pub fn draw(
        &mut self,
        renderer: &mut Renderer2d,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        world: &World,
        blend: f32,
        frame_seconds: f32,
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
