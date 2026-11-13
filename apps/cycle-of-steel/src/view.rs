//! World state to sprites, and the camera that looks at them.
//!
//! The view never changes the world. Each frame it reads the last two simulation
//! steps and blends between them, so drawing at 144 Hz shows motion between the
//! 60 Hz steps instead of repeating them.
//!
//! A room's tiles and decorations never change, so they are uploaded once as
//! static batches when the room is set: one group drawn under the characters and
//! one over them. A frame is then those batches plus a few moving sprites.

use std::collections::HashMap;

use anyhow::{Context, Result};
use crystal_engine::camera2d::Camera2d;
use crystal_engine::glam::{Vec2, Vec3};
use crystal_engine::render2d::{BatchId, DrawList, Pass, Renderer2d, Target, TextureId};
use crystal_engine::scene::{Texture, TextureFilter};
use crystal_engine::sprite::{Grid, Sheet, Sprite};

use crate::assets::{AssetSource, PIXELS_PER_UNIT};
use crate::data::{DecorationFile, DropKind, FontFile, RoomFile, ShakeFile, SortingLayer, SpriteCatalogue};
use crate::hud::{HudLayout, Rect};
use crate::input::StickVisual;
use crate::sim::combat::Interaction;
use crate::sim::defs::Ai;
use crate::sim::enemy::BeamPhase;
use crate::sim::loot::RemainsKind;
use crate::sim::defs::Defs;
use crate::sim::player::{Facing, Player};
use crate::sim::room::{CAMERA_BLOCK, Room};
use crate::sim::{INVENTORY_SLOTS, World};

/// World units from the bottom of the screen to the top: Unity's orthographic
/// size of 2.11506, doubled. A 16:9 window shows about 7.5 of a room's 16 units.
const VIEW_HEIGHT: f32 = 2.0 * 2.11506;
/// How fast the camera catches up with where it should be, in units per second.
/// Fast enough to be invisible inside a room; it shows when changing rooms.
const CAMERA_SPEED: f32 = 40.0;
/// Heights of the three draw groups. Under the straight-down camera only their
/// order matters; they are real heights so a tilted camera can use them later.
const Z_BELOW: f32 = 0.0;
const Z_ENTITIES: f32 = 0.1;
const Z_BULLETS: f32 = 0.15;
const Z_ABOVE: f32 = 0.2;
const Z_DAMAGE_NUMBERS: f32 = 0.25;

/// How damage numbers look, from `DamageNumber.prefab` and `DamageNumberCrit.prefab`:
/// a text box 10.3078 by 3.6328 at scale 0.1, centred (0.47, -0.039) from the
/// number's position, its text set left and top. `size` is world units per em:
/// font size 3 (3.5 for a crit) at that scale of 0.1.
struct NumberStyle {
    size: f32,
    color: [f32; 4],
}
const DAMAGE_NUMBER: NumberStyle = NumberStyle { size: 0.3, color: [0.8208, 0.2052, 0.2052, 1.0] };
const DAMAGE_NUMBER_CRIT: NumberStyle = NumberStyle { size: 0.35, color: [1.0, 0.6726, 0.0, 1.0] };
/// From a number's position to the top-left corner of its text box.
const DAMAGE_NUMBER_BOX: Vec2 = Vec2::new(0.47 - 10.3078 * 0.1 * 0.5, -0.039 + 3.6328 * 0.1 * 0.5);
/// The font's ascent in ems (`ThaleahFat_TTF SDF.asset`: ascent line 59.941406 at
/// point size 90): how far below the top of its box the first baseline sits.
const FONT_ASCENT: f32 = 59.941406 / 90.0;
/// A damage number fades out over this last part of its life. Unity's simply
/// vanishes; the fade is ours, short enough not to change how long it reads.
const DAMAGE_NUMBER_FADE: f32 = 0.2;

/// The gun arm's parts, relative to the pivot it turns about, as in the Unity
/// player prefab: where each sits and how far it is turned, in degrees.
const GUN_IN_HAND: (Vec2, f32) = (Vec2::new(-0.2, 0.0), 0.0);
/// How far each hand's sprite is turned on the arm. Where the hands sit depends
/// on how the player is moving: see `sim::player::Arm`.
const RIGHT_HAND_TURN: f32 = -90.0;
const LEFT_HAND_TURN: f32 = -70.0;
/// The shadow under the player's feet, relative to the player.
const SHADOW_OFFSET: Vec2 = Vec2::new(0.0, -0.182);
/// How the arm hangs when not aiming: down to the left, or mirrored to the right.
const CARRY_ANGLE: f32 = 50.0;

/// World units of camera movement per unit of a gun's shake amplitude, and how
/// many wobbles per second per unit of its frequency. Unity used a Cinemachine
/// noise profile here; these numbers are an approximation of its feel.
const SHAKE_DISTANCE: f32 = 0.06;
const SHAKE_RATE: f32 = 22.0;

const HUD_SLOT_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.45];
const HUD_SELECTED_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.85];
const HUD_HEAT_BACK_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.45];
const HUD_HEAT_COOL_COLOR: [f32; 4] = [1.0, 0.85, 0.2, 0.9];
const HUD_HEAT_HOT_COLOR: [f32; 4] = [1.0, 0.2, 0.1, 0.95];
const HUD_BUTTON_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.28];
/// How many flat pieces a LASERCrab's fading line is drawn in.
const BEAM_PIECES: u32 = 8;
/// A frozen enemy's colour (`ElementalEffectHandler`), as picked on screen.
const FROZEN_TINT: [f32; 4] = [0.3, 1.0, 1.0, 1.0];
const HUD_HEALTH_COLOR: [f32; 4] = [0.55, 0.02, 0.02, 0.9];
const HUD_TEXT_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

const CLEAR_COLOR: [f32; 4] = [0.012, 0.012, 0.016, 1.0];

/// The on-screen stick: a faint base with a brighter knob a third of its size.
const STICK_BASE_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.16];
const STICK_KNOB_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.42];
const STICK_KNOB_SCALE: f32 = 0.38;
/// Side of the generated disc texture, in pixels.
const DISC_PIXELS: u32 = 128;

const PLAYER_FRAME_PIXELS: u32 = 32;

struct PlayerSheets {
    front: TextureId,
    back: TextureId,
    left: TextureId,
    right: TextureId,
    /// All four sheets share one layout: a row of 32 px frames.
    frames: Grid,
}

/// A texture on the GPU and its size, for turning pixel rectangles into UVs.
#[derive(Clone, Copy)]
struct LoadedTexture {
    id: TextureId,
    width: u32,
    height: u32,
}

pub struct View {
    assets: AssetSource,
    catalogue: SpriteCatalogue,
    /// Textures uploaded so far, by file name. Rooms share most of theirs.
    textures: HashMap<String, LoadedTexture>,
    player: PlayerSheets,

    /// The current room's static sprites: drawn before and after the characters.
    below: Vec<BatchId>,
    above: Vec<BatchId>,
    /// The current room's size in world units, for keeping the camera inside it.
    room_size: Vec2,

    camera: Camera2d,
    /// False until the first frame, which places the camera without easing.
    camera_placed: bool,
    /// Show the whole room instead of following the player. For screenshots.
    overview: bool,
    list: DrawList,

    /// A soft white disc, for the touch sticks and round buttons.
    disc: TextureId,
    /// One white pixel, for flat rectangles.
    white: TextureId,
    overlay: DrawList,

    /// Sprites of the things defined in `guns.json`, by their ids. `None` where a
    /// definition names no sprite.
    gun_sprites: Vec<Option<ResolvedSprite>>,
    bullet_sprites: Vec<Option<ResolvedSprite>>,
    effect_sprites: Vec<Option<ResolvedSprite>>,
    /// The pixel font: each character's glyph and how far the pen moves on, in ems.
    font: HashMap<char, (ResolvedSprite, f32)>,
    /// Per chest: whole, damaged and broken.
    chest_sprites: Vec<[Option<ResolvedSprite>; 3]>,
    drop_sprites: Vec<ResolvedSprite>,
    enemy_sprites: Vec<EnemySprites>,
    /// A data cell's picture, beside the count in the HUD.
    data_icon: Option<ResolvedSprite>,
    /// Per gun: a laser's start cap, middle stretch and end cap.
    laser_sprites: Vec<[Option<ResolvedSprite>; 3]>,
    shadow: ResolvedSprite,
    /// The player standing still. The run sheets are used only while moving.
    standing: ResolvedSprite,
    left_hand: ResolvedSprite,
    right_hand: ResolvedSprite,

    /// The camera shake in progress.
    shake: ShakeFile,
    /// Seconds of shaking left.
    shake_left: f32,
    /// Runs on through every shake: where the wobble is in its cycle.
    shake_clock: f32,
}

impl View {
    /// Loads what every room needs: the sprite catalogue, the player's sheets and
    /// the sprites of every gun, bullet and effect.
    pub fn new(renderer: &mut Renderer2d, assets: &AssetSource, defs: &Defs) -> Result<Self> {
        let catalogue = SpriteCatalogue::load(assets)?;

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
        let white = renderer.add_texture(&Texture {
            name: "white".to_string(),
            width: 1,
            height: 1,
            rgba: vec![255; 4],
            filter: TextureFilter::Nearest,
        });

        let mut view = Self {
            assets: assets.clone(),
            catalogue,
            textures: HashMap::new(),
            player: PlayerSheets {
                front,
                back,
                left,
                right,
                frames,
            },
            below: Vec::new(),
            above: Vec::new(),
            room_size: Vec2::new(16.0, 9.0),
            camera: Camera2d::new(Vec2::ZERO, VIEW_HEIGHT),
            camera_placed: false,
            overview: false,
            list: DrawList::new(),
            disc,
            white,
            overlay: DrawList::new(),
            gun_sprites: Vec::new(),
            bullet_sprites: Vec::new(),
            effect_sprites: Vec::new(),
            font: HashMap::new(),
            chest_sprites: Vec::new(),
            drop_sprites: Vec::new(),
            enemy_sprites: Vec::new(),
            data_icon: None,
            laser_sprites: Vec::new(),
            // Placeholders until the catalogue is consulted just below.
            shadow: ResolvedSprite::EMPTY,
            standing: ResolvedSprite::EMPTY,
            left_hand: ResolvedSprite::EMPTY,
            right_hand: ResolvedSprite::EMPTY,
            shake: ShakeFile::default(),
            shake_left: 0.0,
            shake_clock: 0.0,
        };

        view.shadow = view.sprite(renderer, "shadow")?;
        view.standing = view.sprite(renderer, "player_no_hands")?;
        view.left_hand = view.sprite(renderer, "left_hand")?;
        view.right_hand = view.sprite(renderer, "right_hand")?;
        view.gun_sprites = view.optional_sprites(renderer, defs.guns.iter().map(|gun| gun.sprite.as_deref()))?;
        view.bullet_sprites =
            view.optional_sprites(renderer, defs.bullets.iter().map(|bullet| bullet.sprite.as_deref()))?;
        view.effect_sprites =
            view.optional_sprites(renderer, defs.effects.iter().map(|effect| effect.sprite.as_deref()))?;
        for chest in &defs.chests {
            let mut sprites = [None, None, None];
            for (sprite, name) in sprites.iter_mut().zip([Some(&chest.sprite), chest.damaged_sprite.as_ref(), chest.dead_sprite.as_ref()]) {
                if let Some(name) = name {
                    *sprite = Some(view.sprite(renderer, name)?);
                }
            }
            view.chest_sprites.push(sprites);
        }
        for drop in &defs.drops {
            let sprite = view.sprite(renderer, &drop.sprite)?;
            view.drop_sprites.push(sprite);
        }
        for enemy in &defs.enemies {
            let idle = view.sprite(renderer, &enemy.sprite)?;
            let mut animations: [Vec<ResolvedSprite>; 9] = Default::default();
            for (frames, animation) in animations.iter_mut().zip(&enemy.animations) {
                for frame in animation.iter().flat_map(|animation| &animation.frames) {
                    frames.push(view.sprite(renderer, frame)?);
                }
            }
            let bodies = enemy.dead_sprites.iter().map(|name| view.sprite(renderer, name)).collect::<Result<_>>()?;
            view.enemy_sprites.push(EnemySprites { idle, animations, bodies });
        }
        // The plainest data cell: the one worth least.
        view.data_icon = defs
            .drops
            .iter()
            .zip(&view.drop_sprites)
            .filter(|(drop, _)| drop.kind == DropKind::Data)
            .min_by_key(|(drop, _)| drop.value)
            .map(|(_, sprite)| *sprite);
        for gun in &defs.guns {
            let mut pieces = [None, None, None];
            for (piece, name) in pieces.iter_mut().zip(&gun.laser_sprites) {
                if let Some(name) = name {
                    *piece = Some(view.sprite(renderer, name)?);
                }
            }
            view.laser_sprites.push(pieces);
        }
        for (character, glyph) in &FontFile::load(&view.assets)?.glyphs {
            // The font's atlas is black where it is not clear: Unity's font shader
            // reads only its alpha. Made white here, text takes whatever colour it
            // is drawn with.
            if let Some(entry) = view.catalogue.sprites.get(&glyph.sprite)
                && !view.textures.contains_key(&entry.texture)
            {
                let image = whitened(view.assets.sprite(&entry.texture)?);
                let texture = LoadedTexture {
                    id: renderer.add_texture(&image),
                    width: image.width,
                    height: image.height,
                };
                view.textures.insert(entry.texture.clone(), texture);
            }
            let sprite = view.sprite(renderer, &glyph.sprite)?;
            view.font.insert(*character, (sprite, glyph.advance));
        }
        Ok(view)
    }

    fn optional_sprites<'a>(
        &mut self,
        renderer: &mut Renderer2d,
        names: impl Iterator<Item = Option<&'a str>>,
    ) -> Result<Vec<Option<ResolvedSprite>>> {
        names
            .map(|name| name.map(|name| self.sprite(renderer, name)).transpose())
            .collect()
    }

    /// Starts a camera shake, replacing any in progress. As in Unity's
    /// `CameraController.Shake`, this sets how hard and how much longer the camera
    /// shakes without restarting the wobble, so a laser asking every step shakes
    /// smoothly instead of holding the camera at the wobble's first instant.
    pub fn start_shake(&mut self, shake: ShakeFile) {
        self.shake = shake;
        self.shake_left = shake.time;
    }

    /// The world position under a point of the screen, as the camera stood on the
    /// last drawn frame. `point` and `screen` are in pixels.
    pub fn screen_to_world(&self, point: Vec2, screen: Vec2) -> Vec2 {
        self.camera.screen_to_world(point, screen)
    }

    /// Frames the whole room instead of following the player, to check a room
    /// against its Unity original.
    pub fn set_overview(&mut self, overview: bool) {
        self.overview = overview;
    }

    /// Replaces the uploaded room with `room`. Call when the room changes.
    pub fn set_room(&mut self, renderer: &mut Renderer2d, room: &Room) -> Result<()> {
        for batch in self.below.drain(..).chain(self.above.drain(..)) {
            renderer.remove_batch(batch);
        }
        self.room_size = room.size();
        self.camera_placed = false;

        // A room built in code has nothing to draw.
        let Some(file) = &room.file else {
            return Ok(());
        };
        let groups = self.room_sprites(renderer, file)?;
        for group in groups {
            let batches = if group.above { &mut self.above } else { &mut self.below };
            // Sprites in a group never overlap each other out of order, so they can
            // be regrouped by texture: one draw call per texture per group.
            let mut by_texture: Vec<(TextureId, Vec<Sprite>)> = Vec::new();
            for (texture, sprite) in group.sprites {
                match by_texture.iter_mut().find(|(id, _)| *id == texture) {
                    Some((_, sprites)) => sprites.push(sprite),
                    None => by_texture.push((texture, vec![sprite])),
                }
            }
            for (texture, sprites) in by_texture {
                batches.push(renderer.add_batch(texture, &sprites));
            }
        }
        Ok(())
    }

    /// The room's static sprites as groups in draw order: each tile layer is one
    /// group, and each decoration its own.
    fn room_sprites(&mut self, renderer: &mut Renderer2d, file: &RoomFile) -> Result<Vec<SpriteGroup>> {
        // Resolve the palette once: every tile is an index into it.
        let palette = file
            .palette
            .iter()
            .map(|name| self.sprite(renderer, name))
            .collect::<Result<Vec<_>>>()?;

        // (sorting layer, order within it, order in the file) decides what is on top.
        let mut groups: Vec<((SortingLayer, i32, usize), SpriteGroup)> = Vec::new();

        let rows = file.size[1] as usize;
        for (index, layer) in file.layers.iter().enumerate().filter(|(_, layer)| layer.enabled) {
            let sorting_layer = SortingLayer::from_name(&layer.sorting_layer);
            let above = sorting_layer.is_above_entities();
            let z = if above { Z_ABOVE } else { Z_BELOW };

            let mut sprites = Vec::new();
            for (row_from_top, row) in layer.tiles.iter().enumerate() {
                let y = (rows - 1 - row_from_top) as f32;
                for (x, &tile) in row.iter().enumerate() {
                    if tile < 0 {
                        continue;
                    }
                    let resolved = &palette[(tile / 8) as usize];
                    // Unity places the sprite's pivot at the middle of the cell.
                    let centre = Vec3::new((x as f32 + 0.5) * file.cell, (y + 0.5) * file.cell, z);
                    let quarter_turns = (tile % 4) as f32;
                    let mirrored = tile % 8 >= 4;
                    sprites.push((
                        resolved.texture,
                        resolved.placed(centre, [1.0, 1.0], quarter_turns * std::f32::consts::FRAC_PI_2, [mirrored, false]),
                    ));
                }
            }
            groups.push(((sorting_layer, layer.order, index), SpriteGroup { above, sprites }));
        }

        for (index, decoration) in file.decorations.iter().enumerate() {
            let sorting_layer = SortingLayer::from_name(&decoration.sorting_layer);
            let above = sorting_layer.is_above_entities();
            let sprite = self.decoration_sprite(renderer, decoration, if above { Z_ABOVE } else { Z_BELOW })?;
            // The file lists decorations in draw order already; keep them after the
            // tile layers that share their sorting layer.
            let key = (sorting_layer, i32::MAX, file.layers.len() + index);
            groups.push((key, SpriteGroup { above, sprites: vec![sprite] }));
        }

        groups.sort_by_key(|(key, _)| *key);
        Ok(groups.into_iter().map(|(_, group)| group).collect())
    }

    fn decoration_sprite(
        &mut self,
        renderer: &mut Renderer2d,
        decoration: &DecorationFile,
        z: f32,
    ) -> Result<(TextureId, Sprite)> {
        let resolved = self.sprite(renderer, &decoration.sprite)?;
        let position = Vec2::from(decoration.position).extend(z);
        let sprite = resolved
            .placed(position, decoration.scale, decoration.rotation.to_radians(), decoration.flip)
            // `SpriteRenderer.color` is as picked on screen; tints here are linear.
            .with_color(srgb_to_linear(decoration.color));
        Ok((resolved.texture, sprite))
    }

    /// Looks a sprite up in the catalogue, uploading its texture on first use.
    fn sprite(&mut self, renderer: &mut Renderer2d, name: &str) -> Result<ResolvedSprite> {
        let entry = self
            .catalogue
            .sprites
            .get(name)
            .with_context(|| format!("sprite `{name}` is not in sprites.json"))?;

        let texture = match self.textures.get(&entry.texture) {
            Some(texture) => *texture,
            None => {
                let image = self.assets.sprite(&entry.texture)?;
                let texture = LoadedTexture {
                    id: renderer.add_texture(&image),
                    width: image.width,
                    height: image.height,
                };
                self.textures.insert(entry.texture.clone(), texture);
                texture
            }
        };

        let [x, y, width, height] = entry.rect;
        let sheet = Sheet::new(texture.width, texture.height, entry.pixels_per_unit);
        Ok(ResolvedSprite {
            texture: texture.id,
            uv: sheet.uv(x, y, width, height),
            size: sheet.size(width, height),
            pivot: entry.pivot,
        })
    }

    /// Records one frame of `world` into `target`.
    ///
    /// `blend` is how far the frame is between the previous simulation step (0) and
    /// the current one (1); `frame_seconds` is the time since the last drawn frame.
    /// `sticks` are the on-screen sticks fingers are holding, to draw over the HUD.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        renderer: &mut Renderer2d,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        world: &World,
        blend: f32,
        frame_seconds: f32,
        sticks: &[StickVisual],
    ) {
        let blend = blend.clamp(0.0, 1.0);
        let aspect = target.width.max(1) as f32 / target.height.max(1) as f32;
        let player_position = world.player.previous_position.lerp(world.player.position, blend);

        self.follow(player_position, aspect, frame_seconds);
        let shake = self.shake_offset(frame_seconds);

        self.list.clear();
        for &batch in &self.below {
            self.list.batch(batch);
        }

        // Broken chests are on Unity's `Background` sorting layer.
        for remains in &world.combat.remains {
            let sprite = match remains.kind {
                RemainsKind::Chest(chest) => self.chest_sprites[chest.0 as usize][2].as_ref(),
                RemainsKind::Body(enemy, pick) => self.enemy_sprites[enemy.0 as usize].bodies.get(pick as usize),
            };
            if let Some(sprite) = sprite {
                let placed = sprite.placed(remains.position.extend(Z_BELOW), [1.0, 1.0], 0.0, [false, false]);
                self.list.sprite(sprite.texture, placed);
            }
        }

        // Guns and drops on the floor lie under the characters.
        for drop in &world.combat.drops {
            let sprite = &self.drop_sprites[drop.def.0 as usize];
            let position = drop.previous_position.lerp(drop.position, blend);
            self.list.sprite(sprite.texture, sprite.placed(position.extend(Z_BELOW), [1.0, 1.0], 0.0, [false, false]));
        }
        for pickup in &world.combat.pickups {
            if let Some(sprite) = &self.gun_sprites[pickup.gun.def.0 as usize] {
                let placed = sprite.placed(pickup.position.extend(Z_BELOW), [1.0, 1.0], 0.0, [false, false]);
                self.list.sprite(sprite.texture, placed);
            }
        }

        self.draw_enemies(world, blend);
        self.draw_player(world, player_position);

        for bullet in &world.combat.bullets {
            let def = world.defs.bullet(bullet.def);
            if let Some(sprite) = &self.bullet_sprites[bullet.def.0 as usize] {
                let position = bullet.previous_position.lerp(bullet.position, blend);
                let placed = sprite
                    .placed(position.extend(Z_BULLETS), def.scale, bullet.rotation, [false, false])
                    .with_color(srgb_to_linear(def.color));
                self.list.sprite(sprite.texture, placed);
            }
        }
        for shot in &world.combat.shots {
            let def = world.defs.bullet(shot.def);
            if let Some(sprite) = &self.bullet_sprites[shot.def.0 as usize] {
                let position = shot.previous_position.lerp(shot.position, blend);
                let placed = sprite
                    .placed(position.extend(Z_BULLETS), def.scale, shot.rotation, [false, false])
                    .with_color(srgb_to_linear(def.color));
                self.list.sprite(sprite.texture, placed);
            }
        }
        self.draw_enemy_beams(world);
        if let (Some(beam), Some(gun)) = (&world.combat.beam, world.current_gun()) {
            // The beam leaves the gun, so it moves with the blended player.
            let origin = beam.origin + (player_position - world.player.position);
            let pieces = &self.laser_sprites[gun.def.0 as usize];
            for (sprite, placed) in pieces.iter().zip(beam_pieces(origin, beam.direction, beam.length)) {
                if let Some(sprite) = sprite {
                    let (position, stretch) = placed;
                    // Drawn pointing left, like bullets.
                    let rotation = (-beam.direction).to_angle();
                    let placed = sprite.placed(position.extend(Z_BULLETS), [stretch, 1.0], rotation, [false, false]);
                    self.list.sprite(sprite.texture, placed);
                }
            }
        }
        for effect in &world.combat.effects {
            let def = world.defs.effect(effect.def);
            if let Some(sprite) = &self.effect_sprites[effect.def.0 as usize] {
                let placed = sprite
                    .placed(effect.position.extend(Z_BULLETS), def.scale, effect.rotation, [false, false])
                    .with_color(srgb_to_linear(effect.color));
                self.list.sprite(sprite.texture, placed);
            }
        }

        for &batch in &self.above {
            self.list.batch(batch);
        }

        // Chests are on Unity's `HUD Items` sorting layer, over even the foreground.
        for target in &world.combat.targets {
            let Some(chest) = target.chest else {
                continue;
            };
            let [whole, damaged, _] = &self.chest_sprites[chest.0 as usize];
            let sprite = if target.is_damaged() { damaged.as_ref().or(whole.as_ref()) } else { whole.as_ref() };
            if let Some(sprite) = sprite {
                let placed = sprite.placed(target.origin.extend(Z_ABOVE), [1.0, 1.0], 0.0, [false, false]);
                self.list.sprite(sprite.texture, placed);
            }
        }

        for number in &world.combat.damage_numbers {
            let style = if number.crit { DAMAGE_NUMBER_CRIT } else { DAMAGE_NUMBER };
            // The prefabs' colours are as picked on screen; tints here are linear.
            let mut color = srgb_to_linear(style.color);
            color[3] *= (number.time_left / DAMAGE_NUMBER_FADE).min(1.0);
            let mut buffer = [0; 16];
            let text = number_text(number.amount, &mut buffer);
            let baseline = number.position + DAMAGE_NUMBER_BOX - Vec2::new(0.0, FONT_ASCENT * style.size);
            self.draw_text(text, baseline, Z_DAMAGE_NUMBERS, style.size, color);
        }

        let mut camera = self.camera;
        camera.center += shake;
        renderer.begin_frame();
        renderer.draw(
            encoder,
            target,
            &Pass {
                view_projection: camera.view_projection(aspect),
                clear: Some(CLEAR_COLOR),
                viewport: None,
            },
            &self.list,
        );

        self.draw_hud(renderer, encoder, target, world, sticks);
    }

    /// Adds one line of text to the world, starting at `baseline`, its left end.
    /// `size` is world units per em. Characters the font lacks are skipped.
    fn draw_text(&mut self, text: &str, baseline: Vec2, z: f32, size: f32, color: [f32; 4]) {
        push_text(&mut self.list, &self.font, text, baseline, z, size, color);
    }

    /// Enemies on Unity's `Entities` layer: their animation's frame, or the idle
    /// picture until they first move, tinted cyan while frozen.
    fn draw_enemies(&mut self, world: &World, blend: f32) {
        let now = world.time();
        for target in &world.combat.targets {
            let Some(enemy) = &target.enemy else {
                continue;
            };
            let def = world.defs.enemy(enemy.def);
            let sprites = &self.enemy_sprites[enemy.def.0 as usize];
            let sprite = match enemy.animation {
                Some((animation, started)) => {
                    let frames = &sprites.animations[animation.index()];
                    let fps = def.animations[animation.index()].as_ref().map_or(1.0, |animation| animation.fps as f64);
                    let frame = ((now - started).max(0.0) * fps) as usize % frames.len().max(1);
                    frames.get(frame).unwrap_or(&sprites.idle)
                }
                None => &sprites.idle,
            };
            // `ElementalEffectHandler` sets the renderer's colour while frozen.
            let frozen = target.elemental.as_ref().is_some_and(|elemental| elemental.is_frozen());
            let color = if frozen { FROZEN_TINT } else { def.color };
            let position = enemy.previous_origin.lerp(target.origin, blend);
            let placed = sprite
                .placed(position.extend(Z_ENTITIES), def.scale, 0.0, [false, false])
                .with_color(srgb_to_linear(color));
            self.list.sprite(sprite.texture, placed);
        }
    }

    /// LASERCrab lines, as Unity's `LineRenderer` draws them: thin, fading in from
    /// clear at the crab to their colour at the far end, on the `Bullets` layer.
    fn draw_enemy_beams(&mut self, world: &World) {
        for target in &world.combat.targets {
            let (Some(enemy), Some(beam)) = (&target.enemy, target.enemy.as_ref().and_then(|enemy| enemy.beam)) else {
                continue;
            };
            let Ai::Laser(laser) = &world.defs.enemy(enemy.def).ai else {
                continue;
            };
            // `RangedAI_Laser`'s colours: (start, end) of each phase.
            let (start, end) = match beam.phase {
                BeamPhase::Sight => ([0.5, 0.5, 0.5, 0.0], [0.0, 1.0, 0.0, 1.0]),
                BeamPhase::Warning => ([1.0, 0.92, 0.016, 0.0], [1.0, 0.92, 0.016, 1.0]),
                BeamPhase::Firing => ([1.0, 1.0, 1.0, 0.0], [1.0, 0.0, 0.0, 1.0]),
            };
            let span = beam.end - beam.origin;
            let rotation = span.to_angle();
            // The gradient in a few flat pieces: a sprite has one colour.
            for piece in 0..BEAM_PIECES {
                let middle = (piece as f32 + 0.5) / BEAM_PIECES as f32;
                let center = beam.origin + span * middle;
                let color = std::array::from_fn(|channel| start[channel] + (end[channel] - start[channel]) * middle);
                let size = [span.length() / BEAM_PIECES as f32, laser.width];
                let sprite = Sprite::new(center.extend(Z_BULLETS), size, [0.0, 0.0, 1.0, 1.0])
                    .with_rotation(rotation)
                    .with_color(srgb_to_linear(color));
                self.list.sprite(self.white, sprite);
            }
        }
    }

    /// The player: shadow, the hand behind, the body, the gun and the hand in front.
    fn draw_player(&mut self, world: &World, position: Vec2) {
        let player = &world.player;
        let (angle, mirrored) = arm_pose(player);
        let arm = player.arm();
        let pivot = position + arm.pivot;
        // A part of the arm: its offset is turned with the arm, and mirrored
        // top-to-bottom when the arm points right so the gun is never upside down.
        let part = |sprite: &ResolvedSprite, (offset, degrees): (Vec2, f32)| {
            let side = if mirrored { -1.0 } else { 1.0 };
            let offset = Vec2::from_angle(angle).rotate(Vec2::new(offset.x, offset.y * side));
            sprite.placed(
                (pivot + offset).extend(Z_ENTITIES),
                [1.0, 1.0],
                angle + side * degrees.to_radians(),
                [false, mirrored],
            )
        };

        let shadow = self.shadow.placed((position + SHADOW_OFFSET).extend(Z_ENTITIES), [1.0, 1.0], 0.0, [false, false]);
        self.list.sprite(self.shadow.texture, shadow);
        self.list
            .sprite(self.left_hand.texture, part(&self.left_hand, (arm.left_hand, LEFT_HAND_TURN)));

        if player.running {
            let sheet = match player.facing {
                Facing::Front => self.player.front,
                Facing::Back => self.player.back,
                Facing::Left => self.player.left,
                Facing::Right => self.player.right,
            };
            self.list.sprite(
                sheet,
                Sprite::new(
                    position.extend(Z_ENTITIES),
                    self.player.frames.cell_size(),
                    self.player.frames.uv(player.animation_frame()),
                ),
            );
        } else {
            // Unity's animator let go of the sprite when movement stopped, which
            // put the standing picture back.
            let standing = self.standing.placed(position.extend(Z_ENTITIES), [1.0, 1.0], 0.0, [false, false]);
            self.list.sprite(self.standing.texture, standing);
        }

        if let Some(sprite) = world
            .current_gun()
            .and_then(|gun| self.gun_sprites[gun.def.0 as usize].as_ref())
        {
            self.list.sprite(sprite.texture, part(sprite, GUN_IN_HAND));
        }
        self.list
            .sprite(self.right_hand.texture, part(&self.right_hand, (arm.right_hand, RIGHT_HAND_TURN)));
    }

    /// How far to move the camera this frame for the shake in progress.
    fn shake_offset(&mut self, frame_seconds: f32) -> Vec2 {
        if self.shake_left <= 0.0 {
            return Vec2::ZERO;
        }
        let frame_seconds = frame_seconds.max(0.0);
        self.shake_left -= frame_seconds;
        // Kept small so the sines below stay precise however long the game runs.
        self.shake_clock = (self.shake_clock + frame_seconds) % 1000.0;
        let phase = self.shake_clock * self.shake.frequency.max(0.1) * SHAKE_RATE * std::f32::consts::TAU;
        // Two unrelated wobbles per axis, so it does not read as a circle.
        let wobble = Vec2::new(
            phase.sin() + 0.5 * (phase * 2.3 + 1.3).sin(),
            (phase * 1.3 + 0.7).cos() + 0.5 * (phase * 1.9 + 2.1).sin(),
        );
        wobble * self.shake.amplitude * SHAKE_DISTANCE
    }

    /// A second pass in pixel space, over the finished world: inventory slots, the
    /// heat bar, the pick-up button and any on-screen sticks.
    fn draw_hud(
        &mut self,
        renderer: &mut Renderer2d,
        encoder: &mut wgpu::CommandEncoder,
        target: Target<'_>,
        world: &World,
        sticks: &[StickVisual],
    ) {
        let screen = Vec2::new(target.width as f32, target.height as f32);
        let hud = HudLayout::new(screen);
        // The HUD is laid out in window pixels (y down); the pixel camera's y points up.
        let flip = |point: Vec2| Vec3::new(point.x, screen.y - point.y, 0.0);
        let full = [0.0, 0.0, 1.0, 1.0];
        let rect = |rect: Rect, color: [f32; 4]| {
            Sprite::new(flip(rect.center()), rect.size().to_array(), full).with_color(color)
        };
        // A gun's picture fitted inside a square of `side` pixels around `center`.
        let icon = |sprite: &ResolvedSprite, center: Vec2, side: f32| {
            let scale = side / sprite.size[0].max(sprite.size[1]).max(1e-6);
            Sprite::new(flip(center), [sprite.size[0] * scale, sprite.size[1] * scale], sprite.uv)
        };

        self.overlay.clear();

        for (index, slot) in hud.slots.iter().enumerate().take(INVENTORY_SLOTS) {
            if index == world.selected_slot {
                let border = slot.size().x * 0.07;
                let outline = Rect {
                    min: slot.min - Vec2::splat(border),
                    max: slot.max + Vec2::splat(border),
                };
                self.overlay.sprite(self.white, rect(outline, HUD_SELECTED_COLOR));
            }
            self.overlay.sprite(self.white, rect(*slot, HUD_SLOT_COLOR));
            if let Some(sprite) = world.inventory[index]
                .as_ref()
                .and_then(|gun| self.gun_sprites[gun.def.0 as usize].as_ref())
            {
                self.overlay.sprite(sprite.texture, icon(sprite, slot.center(), slot.size().x * 0.8));
            }
        }

        if let Some(gun) = world.current_gun() {
            let heat = gun.heat_fraction(world.defs.gun(gun.def));
            self.overlay.sprite(self.white, rect(hud.heat_bar, HUD_HEAT_BACK_COLOR));
            if heat > 0.0 {
                let mut filled = hud.heat_bar;
                filled.max.x = filled.min.x + hud.heat_bar.size().x * heat;
                let color = std::array::from_fn(|channel| {
                    HUD_HEAT_COOL_COLOR[channel] + (HUD_HEAT_HOT_COLOR[channel] - HUD_HEAT_COOL_COLOR[channel]) * heat
                });
                self.overlay.sprite(self.white, rect(filled, color));
            }
        }

        let health = (world.player.health / world.player.max_health).clamp(0.0, 1.0);
        self.overlay.sprite(self.white, rect(hud.health_bar, HUD_HEAT_BACK_COLOR));
        if health > 0.0 {
            let mut filled = hud.health_bar;
            filled.max.x = filled.min.x + hud.health_bar.size().x * health;
            self.overlay.sprite(self.white, rect(filled, HUD_HEALTH_COLOR));
        }
        if let Some(sprite) = &self.data_icon {
            self.overlay.sprite(sprite.texture, icon(sprite, hud.data_icon, hud.text_size));
        }
        let mut buffer = [0; 16];
        let count = number_text(world.data_cells as f32, &mut buffer);
        let top = flip(hud.data_text).truncate();
        let baseline = top - Vec2::new(0.0, FONT_ASCENT * hud.text_size);
        push_text(&mut self.overlay, &self.font, count, baseline, 0.0, hud.text_size, HUD_TEXT_COLOR);

        // The interact button shows what would be picked up; over a hidden spot
        // it is just the button.
        if let Some(interaction) = world.interaction() {
            let diameter = hud.interact_radius * 2.0;
            self.overlay.sprite(
                self.disc,
                Sprite::new(flip(hud.interact_center), [diameter, diameter], full).with_color(HUD_BUTTON_COLOR),
            );
            if let Interaction::Pickup(index) = interaction
                && let Some(sprite) = &self.gun_sprites[world.combat.pickups[index].gun.def.0 as usize]
            {
                self.overlay
                    .sprite(sprite.texture, icon(sprite, hud.interact_center, hud.interact_radius * 1.2));
            }
        }

        for stick in sticks {
            let base_size = stick.radius * 2.0;
            let knob_size = base_size * STICK_KNOB_SCALE;
            self.overlay.sprites(
                self.disc,
                &[
                    Sprite::new(flip(stick.base), [base_size, base_size], full).with_color(STICK_BASE_COLOR),
                    Sprite::new(flip(stick.knob), [knob_size, knob_size], full).with_color(STICK_KNOB_COLOR),
                ],
            );
        }

        renderer.draw(
            encoder,
            target,
            &Pass {
                view_projection: Camera2d::pixels(screen.x, screen.y).view_projection(),
                clear: None,
                viewport: None,
            },
            &self.overlay,
        );
    }

    /// Moves the camera towards the player, kept inside the block of the world the
    /// player is in. Same rule as Unity's `CameraController`: clamp, then approach
    /// at a fixed speed, which reads as a quick slide when the block changes.
    fn follow(&mut self, player: Vec2, aspect: f32, frame_seconds: f32) {
        if self.overview {
            // Fit the room in both directions, letterboxed in the other.
            let fit_height = self.room_size.y.max(self.room_size.x / aspect.max(1e-6));
            self.camera = Camera2d::new(self.room_size * 0.5, fit_height);
            return;
        }

        let block = (player / CAMERA_BLOCK).floor() * CAMERA_BLOCK;
        let mut wanted = Camera2d::new(player, VIEW_HEIGHT);
        wanted.clamp_to(block, block + CAMERA_BLOCK, aspect);

        if self.camera_placed {
            let step = CAMERA_SPEED * frame_seconds.max(0.0);
            self.camera.center = self.camera.center.move_towards(wanted.center, step);
        } else {
            self.camera.center = wanted.center;
            self.camera_placed = true;
        }
    }
}

/// Sprites drawn together: one tile layer, or one decoration.
struct SpriteGroup {
    /// Drawn over the characters rather than under them.
    above: bool,
    sprites: Vec<(TextureId, Sprite)>,
}

/// An enemy's pictures: standing, each animation's frames, and its bodies.
struct EnemySprites {
    idle: ResolvedSprite,
    /// Indexed by `Animation::index`.
    animations: [Vec<ResolvedSprite>; 9],
    bodies: Vec<ResolvedSprite>,
}

/// A catalogue sprite ready to be placed: its texture, UVs, size and pivot.
#[derive(Clone, Copy)]
struct ResolvedSprite {
    texture: TextureId,
    uv: [f32; 4],
    /// World size at scale 1.
    size: [f32; 2],
    pivot: [f32; 2],
}

/// Where a laser's start cap, middle stretch and end cap go, and how much each is
/// stretched along the beam. From `LaserGun.updateFire`: the caps sit on the two
/// ends at their own size, and the middle, a quarter of a unit long, is scaled by
/// four times the length so that it spans the whole beam.
/// Adds one line of text to `list`, starting at `baseline`, its left end, with y
/// up. `size` is units per em. Characters the font lacks are skipped.
fn push_text(
    list: &mut DrawList,
    font: &HashMap<char, (ResolvedSprite, f32)>,
    text: &str,
    baseline: Vec2,
    z: f32,
    size: f32,
    color: [f32; 4],
) {
    let mut pen = baseline;
    for character in text.chars() {
        if let Some((sprite, advance)) = font.get(&character) {
            // Flipped: the font's atlas holds its glyphs upside down.
            let placed = sprite.placed(pen.extend(z), [size, size], 0.0, [false, true]).with_color(color);
            list.sprite(sprite.texture, placed);
            pen.x += advance * size;
        }
    }
}

fn beam_pieces(origin: Vec2, direction: Vec2, length: f32) -> [(Vec2, f32); 3] {
    [
        (origin, 1.0),
        (origin + direction * (length * 0.5), length * 4.0),
        (origin + direction * length, 1.0),
    ]
}

/// Which way the gun arm points, in radians, and whether it is mirrored.
///
/// Unity's `PlayerAnimations`: while aiming, the arm's -x axis points along the
/// aim and the arm is mirrored when aiming right; otherwise it hangs to the side
/// last walked towards.
/// A damage amount as Unity's `float.ToString()` shows it ("100", "37.5"),
/// written into `buffer` so that drawing a number allocates nothing.
fn number_text(amount: f32, buffer: &mut [u8; 16]) -> &str {
    use std::io::Write;
    let mut cursor = &mut buffer[..];
    // An amount too long for the buffer is cut short rather than dropped.
    let _ = write!(cursor, "{amount}");
    let written = 16 - cursor.len();
    std::str::from_utf8(&buffer[..written]).unwrap_or("")
}

fn arm_pose(player: &Player) -> (f32, bool) {
    match player.aim {
        Some(aim) => ((-aim).to_angle(), aim.x >= 0.0),
        None if player.moved_right => ((180.0 - CARRY_ANGLE).to_radians(), true),
        None => (CARRY_ANGLE.to_radians(), false),
    }
}

impl ResolvedSprite {
    /// Stands in for a sprite that has not been looked up yet. Draws nothing useful.
    const EMPTY: Self = Self {
        texture: TextureId::PLACEHOLDER,
        uv: [0.0; 4],
        size: [0.0; 2],
        pivot: [0.5; 2],
    };

    /// The sprite with its pivot at `position`, scaled, turned counter-clockwise by
    /// `rotation` radians about the pivot, and optionally mirrored in place.
    fn placed(&self, position: Vec3, scale: [f32; 2], rotation: f32, flip: [bool; 2]) -> Sprite {
        let mut uv = self.uv;
        let mut pivot = self.pivot;
        // Mirroring swaps the picture's edges, and the pivot moves with the picture.
        if flip[0] {
            uv.swap(0, 2);
            pivot[0] = 1.0 - pivot[0];
        }
        if flip[1] {
            uv.swap(1, 3);
            pivot[1] = 1.0 - pivot[1];
        }
        Sprite::new(position, [self.size[0] * scale[0], self.size[1] * scale[1]], uv)
            .with_pivot(pivot)
            .with_rotation(rotation)
    }
}

/// A white disc with a soft edge on a transparent background, sampled smoothly so
/// it stays round at any size.
/// A colour as Unity's colour picker shows it, as the linear tint that draws it.
///
/// Unity's sprite shaders (URP's `Sprite-Lit-Default`, which nearly every sprite and
/// tilemap of the project uses, and the built-in `Sprites/Default`) multiply the
/// texture by the vertex colour as it comes. The project is in linear colour space,
/// where the engine turns `SpriteRenderer.color` from sRGB to linear when it fills
/// that vertex colour, so a white pixel tinted with a colour shows as that colour.
/// The render target here is sRGB and tints are linear, hence the same conversion.
/// Alpha is not a colour and stays as it is.
fn srgb_to_linear(color: [f32; 4]) -> [f32; 4] {
    let channel = |value: f32| {
        if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    [channel(color[0]), channel(color[1]), channel(color[2]), color[3]]
}

/// The same picture with every pixel white, keeping only how opaque each is.
fn whitened(mut texture: Texture) -> Texture {
    for pixel in texture.rgba.chunks_exact_mut(4) {
        pixel[..3].fill(255);
    }
    texture
}

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
    fn mirroring_a_sprite_moves_its_pivot_with_the_picture() {
        let sprite = ResolvedSprite {
            texture: TextureId::PLACEHOLDER,
            uv: [0.0, 0.25, 0.5, 0.75],
            size: [1.0, 2.0],
            pivot: [0.25, 0.0],
        };
        let plain = sprite.placed(Vec3::ZERO, [2.0, 1.0], 0.0, [false, false]);
        assert_eq!((plain.uv, plain.pivot, plain.size), ([0.0, 0.25, 0.5, 0.75], [0.25, 0.0], [2.0, 2.0]));

        let mirrored = sprite.placed(Vec3::ZERO, [1.0, 1.0], 0.0, [true, true]);
        assert_eq!(mirrored.uv, [0.5, 0.75, 0.0, 0.25]);
        assert_eq!(mirrored.pivot, [0.75, 1.0]);
    }

    #[test]
    fn the_arm_points_along_the_aim_and_mirrors_when_aiming_right() {
        let mut player = Player::new(Vec2::ZERO);
        // Carried: down to the left until the player has walked right.
        assert_eq!(arm_pose(&player), (50f32.to_radians(), false));
        player.moved_right = true;
        assert_eq!(arm_pose(&player), (130f32.to_radians(), true));

        // The gun sits on the arm's -x axis, so that axis must lie along the aim.
        for aim in [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::new(0.6, -0.8)] {
            player.aim = Some(aim);
            let (angle, mirrored) = arm_pose(&player);
            let gun_direction = Vec2::from_angle(angle).rotate(Vec2::NEG_X);
            assert!(gun_direction.distance(aim) < 1e-5, "aiming {aim}: gun at {gun_direction}");
            assert_eq!(mirrored, aim.x >= 0.0);
        }
    }

    #[test]
    fn damage_amounts_read_as_unity_prints_them() {
        let mut buffer = [0; 16];
        assert_eq!(number_text(100.0, &mut buffer), "100");
        let mut picture = disc_texture(2);
        picture.rgba = vec![0, 0, 0, 255, 9, 9, 9, 0];
        assert_eq!(whitened(picture).rgba, [255, 255, 255, 255, 255, 255, 255, 0]);
        assert_eq!(number_text(37.5, &mut buffer), "37.5");
        let linear = srgb_to_linear([1.0, 0.5, 0.0, 0.7]);
        assert!((linear[1] - 0.2140).abs() < 1e-3 && linear[0] == 1.0 && linear[2] == 0.0 && linear[3] == 0.7);
        assert_eq!(number_text(9_999_999.0, &mut buffer), "9999999");
        // The text box's top-left corner, from the prefab: just left of and above
        // the number's position.
        assert!((DAMAGE_NUMBER_BOX - Vec2::new(-0.0454, 0.1426)).length() < 1e-3);
    }

    /// What an sRGB render target stores for a linear value.
    fn linear_to_srgb_byte(value: f32) -> u8 {
        let encoded = if value <= 0.003_130_8 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 };
        (encoded * 255.0).round() as u8
    }

    #[test]
    fn a_unity_tint_reaches_the_screen_as_authored() {
        // `Bullet_Pistol_2` and a half-clear hub decoration, as in the converted data.
        for authored in [[0.1273, 0.4008, 0.4906, 1.0], [1.0, 1.0, 1.0, 0.2902], [0.0, 0.03, 0.7642, 0.5]] {
            let tint = srgb_to_linear(authored);
            assert_eq!(tint[3], authored[3], "alpha is not converted");
            for channel in 0..3 {
                let authored_byte = (authored[channel] * 255.0_f32).round() as u8;
                assert_eq!(linear_to_srgb_byte(tint[channel]), authored_byte, "{authored:?} channel {channel}");
            }
        }
        // Used as it is, the pistol's teal would be far too bright: (100, 170, 186)
        // on screen instead of (32, 102, 125).
        assert_eq!(linear_to_srgb_byte(0.1273), 100);
        assert_eq!(linear_to_srgb_byte(srgb_to_linear([0.1273, 0.0, 0.0, 1.0])[0]), 32);
        // White and black tints are the same either way.
        assert_eq!(srgb_to_linear([1.0, 0.0, 1.0, 1.0]), [1.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn disc_is_solid_in_the_middle_and_clear_in_the_corners() {
        let disc = disc_texture(32);
        let alpha = |x: u32, y: u32| disc.rgba[((y * 32 + x) * 4 + 3) as usize];
        assert_eq!(alpha(16, 16), 255);
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(31, 31), 0);
        assert_eq!(disc.rgba.len(), 32 * 32 * 4);
    }
    #[test]
    fn a_beam_is_two_caps_with_the_middle_stretched_between_them() {
        let pieces = beam_pieces(Vec2::new(1.0, 2.0), Vec2::new(0.0, 1.0), 3.0);
        assert_eq!(pieces[0], (Vec2::new(1.0, 2.0), 1.0));
        // The middle sprite is a quarter of a unit long: twelve of them make three units.
        assert_eq!(pieces[1], (Vec2::new(1.0, 3.5), 12.0));
        assert_eq!(pieces[2], (Vec2::new(1.0, 5.0), 1.0));
    }
}
