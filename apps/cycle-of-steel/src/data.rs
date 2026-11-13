//! The asset files' formats: rooms and the sprite catalogue.
//!
//! These files were first written by `cos-convert` from the Unity project and are
//! now the source of truth. Everything is in room space: x right, y up, origin at
//! the room's bottom-left corner, in world units.
//!
//! Only what the game uses so far is read.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::assets::AssetSource;

/// `sprites.json`: every sprite by name.
#[derive(Debug, Deserialize)]
pub struct SpriteCatalogue {
    pub sprites: HashMap<String, SpriteEntry>,
}

/// One sprite: a rectangle of a texture and how it sits in the world.
#[derive(Clone, Debug, Deserialize)]
pub struct SpriteEntry {
    /// `sprites/<texture>.png`.
    pub texture: String,
    /// `[x, y, width, height]` in pixels, origin at the texture's top-left.
    pub rect: [u32; 4],
    /// The point placed at the sprite's position: (0, 0) bottom-left, (1, 1) top-right.
    pub pivot: [f32; 2],
    pub pixels_per_unit: f32,
}

/// `font.json`: the pixel font, one glyph sprite per character.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FontFile {
    /// From one line's baseline to the next, in ems.
    pub line_height: f32,
    pub glyphs: HashMap<char, GlyphFile>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GlyphFile {
    /// A key of `sprites.json`. Its pivot is the pen position on the baseline and
    /// its size is in ems. The picture is upside down, as Unity's atlas stores it:
    /// draw it flipped top to bottom.
    pub sprite: String,
    /// How far the pen moves on, in ems.
    pub advance: f32,
}

impl FontFile {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("font.json")?;
        serde_json::from_slice(&bytes).context("could not parse font.json")
    }
}

impl SpriteCatalogue {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("sprites.json")?;
        serde_json::from_slice(&bytes).context("could not parse sprites.json")
    }
}

/// `rooms/<name>.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct RoomFile {
    pub name: String,
    /// Columns and rows of cells.
    pub size: [u32; 2],
    /// World size of one cell.
    pub cell: f32,
    /// Sprite names; tiles refer to them by position in this list.
    pub palette: Vec<String>,
    /// Bottom layer first.
    pub layers: Vec<LayerFile>,
    /// Loose sprites, in draw order.
    #[serde(default)]
    pub decorations: Vec<DecorationFile>,
    /// Prefabs placed in the room: guns to pick up, enemies, chests, hazards.
    #[serde(default)]
    pub objects: Vec<ObjectFile>,
    #[serde(default)]
    pub markers: Vec<MarkerFile>,
    /// Boxes at the doorways that stop enemies (Unity: everything but the player).
    #[serde(default)]
    pub barriers: Vec<BoxFile>,
    /// Which sides have a doorway that `RoomScript.SealRoom` closes.
    #[serde(default)]
    pub doors: DoorsFile,
    /// Unity's 2D lights in the room.
    #[serde(default)]
    pub lights: Vec<LightFile>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightKind {
    /// Lights everything on its layers.
    Global,
    /// A circle, brightest inside `inner_radius`, gone at `outer_radius`.
    Point,
    /// A polygon, `path`.
    Freeform,
}

/// A `Light2D`. Each lights only the sorting layers it names.
#[derive(Clone, Debug, Deserialize)]
pub struct LightFile {
    pub kind: LightKind,
    #[serde(default)]
    pub position: [f32; 2],
    pub color: [f32; 3],
    pub intensity: f32,
    #[serde(default)]
    pub inner_radius: f32,
    #[serde(default)]
    pub outer_radius: f32,
    /// Unity's falloff strength.
    #[serde(default)]
    pub falloff: f32,
    /// A freeform light's outline, in room space.
    #[serde(default)]
    pub path: Vec<[f32; 2]>,
    /// Set if walls shadow it: how strongly (`m_ShadowIntensity`).
    #[serde(default)]
    pub shadow: Option<f32>,
    pub layers: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct DoorsFile {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LayerFile {
    pub name: String,
    /// Unity's sorting layer: `Background`, `Default`, `Entities`, `Bullets` or
    /// `Foreground`. Everything up to `Entities` is drawn under the characters.
    pub sorting_layer: String,
    pub order: i32,
    /// Switched-off layers are optional wall sets, swapped in to close a side.
    pub enabled: bool,
    /// A ledge blocks walkers but not bullets or flyers.
    #[serde(default)]
    pub ledge: bool,
    /// One row per line, top row first. -1 is empty; otherwise
    /// `palette index * 8 + transform`, where the transform's low two bits are
    /// quarter turns counter-clockwise and 4 means mirrored left-to-right first.
    pub tiles: Vec<Vec<i32>>,
    /// Which cells block movement: one string per row, top first, `#` for solid.
    #[serde(default)]
    pub solid: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DecorationFile {
    pub sprite: String,
    pub position: [f32; 2],
    pub sorting_layer: String,
    /// Counter-clockwise, in degrees.
    #[serde(default)]
    pub rotation: f32,
    #[serde(default = "one")]
    pub scale: [f32; 2],
    /// Mirror left-to-right, top-to-bottom.
    #[serde(default)]
    pub flip: [bool; 2],
    #[serde(default = "white")]
    pub color: [f32; 4],
}

/// A prefab placed in the room. What it is depends on its name: a key of
/// `guns.json` is a gun lying on the floor, and so on.
#[derive(Clone, Debug, Deserialize)]
pub struct ObjectFile {
    pub prefab: String,
    pub position: [f32; 2],
    /// The object it sits under in the room, such as `Enemies`.
    #[serde(default)]
    pub group: Option<String>,
    /// Settings this placement changes from its prefab, as `Script.field` -> text.
    #[serde(default)]
    pub overrides: HashMap<String, String>,
}

/// An object that carried one of the Unity game's scripts.
#[derive(Clone, Debug, Deserialize)]
pub struct MarkerFile {
    pub name: String,
    pub scripts: Vec<String>,
    pub position: [f32; 2],
    /// Its box collider, if it had one.
    #[serde(default, rename = "box")]
    pub bounds: Option<BoxFile>,
    /// Settings typed into the Unity inspector, as `Script.field` -> text.
    #[serde(default)]
    pub fields: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct BoxFile {
    pub center: [f32; 2],
    pub size: [f32; 2],
}

impl MarkerFile {
    pub fn has_script(&self, script: &str) -> bool {
        self.scripts.iter().any(|name| name == script)
    }

    /// A numeric setting, e.g. `number("Killable.maxHealth")`.
    pub fn number(&self, field: &str) -> Option<f32> {
        self.fields.get(field)?.parse().ok()
    }
}

/// `guns.json`: every gun, the bullets they fire and the effects those leave.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct GunsFile {
    pub guns: HashMap<String, GunFile>,
    pub bullets: HashMap<String, BulletFile>,
    pub effects: HashMap<String, EffectFile>,
}

impl GunsFile {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("guns.json")?;
        serde_json::from_slice(&bytes).context("could not parse guns.json")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GunKind {
    /// One bullet per shot, straight along the aim.
    Single,
    /// Several bullets per shot, each off-aim by a random angle and speed.
    Spray,
    /// A continuous beam.
    Laser,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GunFile {
    /// The number save files know the gun by.
    pub id: i32,
    /// What firing sounds like.
    #[serde(default)]
    pub sound: Option<SoundFile>,
    pub kind: GunKind,
    #[serde(default)]
    pub sprite: Option<String>,
    /// The box the player must touch to pick the gun up, in world units.
    #[serde(default = "quarter")]
    pub pickup_size: [f32; 2],
    /// Shots (or laser ticks) until the gun overheats. 0 with a 0 cooldown means
    /// it never does.
    pub heat_capacity: f32,
    /// Seconds an overheated gun is locked while it cools.
    pub heat_cooldown: f32,
    pub damage: f32,
    pub knockback: f32,
    #[serde(default)]
    pub shake: ShakeFile,
    /// Seconds between shots.
    #[serde(default)]
    pub fire_delay: f32,
    /// World units per second.
    #[serde(default)]
    pub bullet_speed: f32,
    #[serde(default = "one_bullet")]
    pub bullet_count: u32,
    /// Spray guns: each bullet leaves up to this many degrees either side of the aim.
    #[serde(default)]
    pub spray_angle: f32,
    /// Spray guns: each bullet's speed is multiplied by a random 1..=this.
    #[serde(default = "unit")]
    pub random_speed: f32,
    /// A key of `bullets`. Lasers have none.
    #[serde(default)]
    pub bullet: Option<String>,
    /// Lasers: seconds between the beam's hits.
    #[serde(default)]
    pub damage_delay: f32,
    /// Lasers: the sprites of the beam's two caps and of the stretch between them.
    #[serde(default)]
    pub laser_start: Option<String>,
    #[serde(default)]
    pub laser_middle: Option<String>,
    #[serde(default)]
    pub laser_end: Option<String>,
}

/// A camera shake: how long, how far and how fast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct ShakeFile {
    pub time: f32,
    pub amplitude: f32,
    pub frequency: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BulletFile {
    /// The lights it carries, placed from its position (its prefab's `Light2D`s).
    #[serde(default)]
    pub lights: Vec<LightFile>,
    #[serde(default)]
    pub sprite: Option<String>,
    #[serde(default = "white")]
    pub color: [f32; 4],
    #[serde(default = "one")]
    pub scale: [f32; 2],
    /// Collision box, in world units.
    #[serde(default = "eighth")]
    pub size: [f32; 2],
    /// Seconds before it disappears by itself.
    pub time_to_live: f32,
    /// Carries on through what it hits instead of stopping at the first.
    #[serde(default)]
    pub piercing: bool,
    /// A key of `effects`: what is left where it hits.
    #[serde(default)]
    pub effect: Option<String>,
    /// Unity's `HomingBullet.homingPower`: the force that steers it. 0 flies straight.
    #[serde(default)]
    pub homing: f32,
    /// The bullet's `Rigidbody2D` linear drag.
    #[serde(default)]
    pub drag: f32,
    /// `Bullet_Freeze.freezeStrength`: how hard it freezes what it hits. 0 does not.
    #[serde(default)]
    pub freeze: f32,
}

/// An effect that hurts everything it touches when it appears (`ExplosionScript`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct ExplosionFile {
    pub damage: f32,
    /// World units, from the prefab's `CircleCollider2D` and its scale.
    pub radius: f32,
    /// `ExplosionScript` divides its push by the collider's own radius, which a
    /// scaled prefab's blast reaches beyond. Absent when they are the same.
    #[serde(default)]
    pub knockback_radius: Option<f32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EffectFile {
    /// Played as it appears (`ExplosionScript.explosion`).
    #[serde(default)]
    pub sound: Option<SoundFile>,
    /// The lights it carries, placed from its position (its prefab's `Light2D`s).
    #[serde(default)]
    pub lights: Vec<LightFile>,
    #[serde(default)]
    pub sprite: Option<String>,
    #[serde(default = "white")]
    pub color: [f32; 4],
    #[serde(default = "one")]
    pub scale: [f32; 2],
    pub time_to_live: f32,
    #[serde(default)]
    pub explosion: Option<ExplosionFile>,
}

/// A sound: Unity's `AudioSource` with its clip (a file under `audio/`, by stem),
/// volume and whether it loops.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SoundFile {
    pub clip: String,
    #[serde(default = "full")]
    pub volume: f32,
    #[serde(default, rename = "loop")]
    pub looping: bool,
}

fn full() -> f32 {
    1.0
}

/// `sounds.json`: the player's sounds and the music.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SoundsFile {
    pub player: PlayerSoundsFile,
    pub music: Option<SoundFile>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PlayerSoundsFile {
    pub hurt: Option<SoundFile>,
    pub heal: Option<SoundFile>,
    pub pickup: Option<SoundFile>,
    pub walk: Option<SoundFile>,
}

impl SoundsFile {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("sounds.json")?;
        serde_json::from_slice(&bytes).context("could not parse sounds.json")
    }
}

/// `dungeon.json`: the dungeon generator's settings (`DungeonGenerator`).
#[derive(Clone, Debug, Deserialize)]
pub struct DungeonFile {
    /// Rooms on each side of the generator's square grid.
    pub grid_size: u32,
    /// How many rooms it grows, the spawn included.
    pub rooms: u32,
    /// The spawn room's place on the grid.
    pub spawn: [i32; 2],
    /// Room files: what a grown room is until swapped, and the special rooms.
    pub open_room: String,
    pub spawn_room: String,
    pub exit_room: String,
    pub boss_room: String,
    /// For each set of doors (`N`, `NE`, `NSEW`...), the rooms that fit, in the
    /// order they are tried.
    pub by_doors: HashMap<String, Vec<String>>,
    /// The tiles `SealRoom` puts in doorways.
    pub door_tiles: DoorTilesFile,
    /// The minimap's pictures (`MapMenu`).
    pub map: MapFile,
    /// Each room's `distanceFromSpawn`: the layer it may appear from.
    #[serde(default)]
    pub distance: HashMap<String, i32>,
}

/// The minimap's pictures, by what a room is to the player.
#[derive(Clone, Debug, Deserialize)]
pub struct MapFile {
    pub unknown: String,
    pub spawn: String,
    pub explored: String,
    pub exit: String,
    pub boss: String,
    pub highlight: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DoorTilesFile {
    /// The two rows of a top doorway, and the one of a bottom doorway (`top`).
    pub top: DoorTileFile,
    pub bottom: DoorTileFile,
    /// The three cells of a side doorway.
    pub vertical: DoorTileFile,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DoorTileFile {
    pub sprite: String,
    pub solid: bool,
}

impl DungeonFile {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("dungeon.json")?;
        serde_json::from_slice(&bytes).context("could not parse dungeon.json")
    }
}

/// `loot.json`: chests, the loot pools they roll on, and the drops they leave.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct LootFile {
    pub chests: HashMap<String, ChestFile>,
    pub pools: HashMap<String, PoolFile>,
    pub drops: HashMap<String, DropFile>,
}

impl LootFile {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("loot.json")?;
        serde_json::from_slice(&bytes).context("could not parse loot.json")
    }
}

/// One line of a loot table: what `drop` names (a pool, a drop or a gun) appears
/// when a roll of 0 to 100 comes out at most `weight`.
#[derive(Clone, Debug, Deserialize)]
pub struct LootEntryFile {
    pub drop: String,
    pub weight: f32,
}

/// A chest: Unity's `Killable` on a prefab under `Prefabs/Room Objects`.
#[derive(Clone, Debug, Deserialize)]
pub struct ChestFile {
    /// The lights it carries, placed from its position (its prefab's `Light2D`s).
    #[serde(default)]
    pub lights: Vec<LightFile>,
    pub max_health: f32,
    pub sprite: String,
    /// Shown below 70% health.
    pub damaged_sprite: Option<String>,
    /// What is left on the floor once it breaks.
    pub dead_sprite: Option<String>,
    /// Its solid box, relative to its position.
    pub size: [f32; 2],
    #[serde(default)]
    pub offset: [f32; 2],
    pub loot: Vec<LootEntryFile>,
}

/// A loot pool: Unity's `Spawner`.
#[derive(Clone, Debug, Deserialize)]
pub struct PoolFile {
    /// Every entry rolls on its own; otherwise exactly one entry is spawned.
    pub multiple: bool,
    pub entries: Vec<LootEntryFile>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropKind {
    /// `DataDrop`: adds to the player's data cells.
    Data,
    /// `HealDrop`: gives the player health back.
    Heal,
}

/// A pickup that flies to the player once they are near.
#[derive(Clone, Debug, Deserialize)]
pub struct DropFile {
    /// The lights it carries, placed from its position (its prefab's `Light2D`s).
    #[serde(default)]
    pub lights: Vec<LightFile>,
    pub kind: DropKind,
    pub value: u32,
    pub sprite: String,
    pub pickup_range: f32,
    pub pickup_speed: f32,
    pub drag: f32,
    /// Its trigger circle, relative to its position.
    pub radius: f32,
    #[serde(default)]
    pub offset: [f32; 2],
}

/// `enemies.json`: every enemy the port has an AI for.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct EnemiesFile {
    pub enemies: HashMap<String, EnemyFile>,
}

impl EnemiesFile {
    pub fn load(assets: &AssetSource) -> Result<Self> {
        let bytes = assets.read("enemies.json")?;
        serde_json::from_slice(&bytes).context("could not parse enemies.json")
    }
}

/// An enemy: Unity's `Killable` and its AI script on a prefab under `Prefabs/Enemies`.
#[derive(Clone, Debug, Deserialize)]
pub struct EnemyFile {
    /// Its body's noise as it dies (`RandomBodyScript.deathSound`).
    #[serde(default)]
    pub death_sound: Option<SoundFile>,
    /// The lights it carries, placed from its position (its prefab's `Light2D`s).
    #[serde(default)]
    pub lights: Vec<LightFile>,
    pub max_health: f32,
    /// Shown until it first moves: Unity's Animator starts with no controller.
    pub sprite: String,
    #[serde(default = "one")]
    pub scale: [f32; 2],
    #[serde(default = "white")]
    pub color: [f32; 4],
    /// Its solid box, relative to its position, in world units.
    pub size: [f32; 2],
    #[serde(default)]
    pub offset: [f32; 2],
    pub mass: f32,
    pub drag: f32,
    /// Has an `ElementalEffectHandler`, so freezing bullets slow it.
    pub freezable: bool,
    /// Its room stays sealed until it dies (`countAsEnemy`).
    #[serde(default)]
    pub counts_as_enemy: bool,
    pub loot: Vec<LootEntryFile>,
    /// One is picked at random for the body it leaves.
    pub dead_sprites: Vec<String>,
    /// Its AI: exactly one of these.
    #[serde(default)]
    pub warrior: Option<WarriorFile>,
    #[serde(default)]
    pub ranged: Option<RangedFile>,
    #[serde(default)]
    pub turret: Option<TurretFile>,
    #[serde(default)]
    pub laser: Option<LaserFile>,
    #[serde(default)]
    pub drone: Option<DroneFile>,
    /// On the `FlyingEnemy` layer: ledges do not stop it.
    #[serde(default)]
    pub flying: bool,
    /// The effect it leaves when it dies (`Killable.explosive`), a key of
    /// `guns.json`'s effects.
    #[serde(default)]
    pub explosion: Option<String>,
    /// A box that kills it, and so sets off its explosion, when the player enters
    /// it (`ExplosionTrigger`).
    #[serde(default)]
    pub detonate: Option<OffsetBoxFile>,
    #[serde(default)]
    pub boss: Option<BossFile>,
    /// A boss's name, shown over its health bar (`Killable.enemyName`).
    #[serde(default)]
    pub boss_bar: Option<String>,
    /// Where its picture is drawn from its position, when that is on a child.
    #[serde(default)]
    pub sprite_offset: [f32; 2],
    /// A bite on touch (`BasicAIAttack`), beside its AI.
    #[serde(default)]
    pub touch: Option<TouchFile>,
    /// By name: `idle`, `run_right`, `attack_up` and so on.
    #[serde(default)]
    pub animations: HashMap<String, AnimationFile>,
}

/// Unity's `RangedAI`: keeps its distance and shoots at the player.
#[derive(Clone, Debug, Deserialize)]
pub struct RangedFile {
    pub follow_range: f32,
    pub attack_range: f32,
    pub move_speed: f32,
    pub wander_interval: f32,
    pub fire_interval: f32,
    pub damage: f32,
    pub spotting_length: f32,
    /// A key of `guns.json`'s bullets.
    pub bullet: String,
}

/// Unity's `BossAI`, the Priest's.
#[derive(Clone, Debug, Deserialize)]
pub struct BossFile {
    pub move_speed: f32,
    /// Bullets, keys of `guns.json`'s: the staff's spiral, the eight-way spiral
    /// and the homing ring.
    pub bullet: String,
    pub far_bullet: String,
    pub homing_bullet: String,
    pub bullet_damage: f32,
    pub spiral_interval: f32,
    pub spiral_eight_interval: f32,
    pub homing_interval: f32,
    /// Where the staff's spiral leaves from.
    pub staff_offset: [f32; 2],
    pub laser: BossLaserFile,
}

/// The boss's laser prefab (`Laser`).
#[derive(Clone, Debug, Deserialize)]
pub struct BossLaserFile {
    pub damage: f32,
    pub damage_delay: f32,
    pub start: String,
    pub middle: String,
    pub end: String,
}

/// Unity's `DashAIDrone`.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct DroneFile {
    pub follow_range: f32,
    pub move_speed: f32,
    pub wander_interval: f32,
}

/// Unity's `RangedAI_Laser`: follows, then sweeps a beam at the player.
#[derive(Clone, Debug, Deserialize)]
pub struct LaserFile {
    pub follow_range: f32,
    pub attack_range: f32,
    pub move_speed: f32,
    pub wander_interval: f32,
    /// Seconds of each attack: the first second only warns.
    pub max_fire_time: f32,
    /// Seconds between attacks.
    pub fire_interval: f32,
    pub damage: f32,
    /// Seconds between the beam's hits.
    pub damage_rate: f32,
    pub spotting_length: f32,
    /// How fast the beam turns towards the player, degrees per second.
    pub turn_speed: f32,
    /// The beam's width in world units.
    pub width: f32,
}

/// Unity's `RangedAITurret`: fires from every muzzle at once while the player is near.
#[derive(Clone, Debug, Deserialize)]
pub struct TurretFile {
    pub attack_range: f32,
    pub fire_interval: f32,
    pub damage: f32,
    pub bullet: String,
    pub muzzles: Vec<MuzzleFile>,
}

/// Where a turret's shot starts, from its position, and which way it flies.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct MuzzleFile {
    pub offset: [f32; 2],
    /// Degrees counter-clockwise from the right.
    pub angle: f32,
}

/// Unity's `BasicAIAttack`: hurts the player touching its box, at most every cooldown.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct TouchFile {
    pub damage: f32,
    pub damage_cooldown: f32,
    pub reach: OffsetBoxFile,
}

/// A box relative to its owner's position, in world units.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct OffsetBoxFile {
    pub size: [f32; 2],
    #[serde(default)]
    pub offset: [f32; 2],
}

/// Unity's `Warrior` script.
#[derive(Clone, Debug, Deserialize)]
pub struct WarriorFile {
    pub spotting_length: f32,
    pub follow_range: f32,
    pub move_speed: f32,
    pub wander_interval: f32,
    pub damage: f32,
    pub damage_cooldown: f32,
    /// The trigger that starts a swing when the player touches it.
    pub reach: OffsetBoxFile,
    /// Where each swing lands: `right`, `left`, `up`, `down`.
    pub swings: HashMap<String, OffsetBoxFile>,
}

/// Sprites shown one after another at `fps`, looping.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AnimationFile {
    pub fps: f32,
    pub frames: Vec<String>,
}

fn quarter() -> [f32; 2] {
    [0.25, 0.25]
}

fn eighth() -> [f32; 2] {
    [0.125, 0.125]
}

fn one_bullet() -> u32 {
    1
}

fn unit() -> f32 {
    1.0
}

fn one() -> [f32; 2] {
    [1.0, 1.0]
}

fn white() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

impl RoomFile {
    pub fn load(assets: &AssetSource, name: &str) -> Result<Self> {
        let path = format!("rooms/{name}.json");
        let bytes = assets.read(&path)?;
        let room: RoomFile = serde_json::from_slice(&bytes).with_context(|| format!("could not parse {path}"))?;
        room.check().with_context(|| format!("{path} is not a usable room"))?;
        Ok(room)
    }

    /// Rejects files whose grids do not match their stated size, so that nothing
    /// later has to bounds-check a tile lookup.
    fn check(&self) -> Result<()> {
        let [columns, rows] = self.size;
        if columns == 0 || rows == 0 || self.cell <= 0.0 {
            bail!("size {columns}x{rows} with cell {} is empty", self.cell);
        }
        for layer in &self.layers {
            if layer.tiles.len() != rows as usize || layer.tiles.iter().any(|row| row.len() != columns as usize) {
                bail!("layer `{}` is not {columns}x{rows} tiles", layer.name);
            }
            for &tile in layer.tiles.iter().flatten() {
                if tile >= 0 && (tile / 8) as usize >= self.palette.len() {
                    bail!("layer `{}` uses sprite {} but the palette has {}", layer.name, tile / 8, self.palette.len());
                }
            }
            if let Some(solid) = &layer.solid
                && (solid.len() != rows as usize || solid.iter().any(|row| row.len() != columns as usize))
            {
                bail!("layer `{}` has a solid grid that is not {columns}x{rows}", layer.name);
            }
        }
        Ok(())
    }
}

/// Unity's sorting layers, in draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SortingLayer {
    Background,
    Default,
    Entities,
    Bullets,
    Foreground,
}

impl SortingLayer {
    /// Unknown names draw as `Default`, which is what Unity does with a missing layer.
    pub fn from_name(name: &str) -> Self {
        match name {
            "Background" => Self::Background,
            "Entities" => Self::Entities,
            "Bullets" => Self::Bullets,
            "Foreground" => Self::Foreground,
            _ => Self::Default,
        }
    }

    /// Whether things on this layer are drawn over the characters.
    pub fn is_above_entities(self) -> bool {
        self > Self::Entities
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room_json(tiles: &str) -> String {
        format!(
            r##"{{"name": "t", "size": [2, 2], "cell": 0.5, "palette": ["a"],
                "layers": [{{"name": "Walls", "sorting_layer": "Default", "order": 0, "enabled": true,
                             "tiles": {tiles}, "solid": ["#.", ".."]}}]}}"##
        )
    }

    #[test]
    fn a_room_parses_with_defaults_for_what_is_missing() {
        let room: RoomFile = serde_json::from_str(&room_json("[[0, -1], [5, -1]]")).unwrap();
        room.check().unwrap();
        assert_eq!(room.size, [2, 2]);
        assert!(room.decorations.is_empty());
        assert!(!room.layers[0].ledge);
    }

    #[test]
    fn mismatched_grids_and_bad_palette_indices_are_rejected() {
        let short: RoomFile = serde_json::from_str(&room_json("[[0, -1]]")).unwrap();
        assert!(short.check().is_err());
        // 8 is palette index 1; the palette has one sprite.
        let bad: RoomFile = serde_json::from_str(&room_json("[[8, -1], [-1, -1]]")).unwrap();
        assert!(bad.check().is_err());
    }

    #[test]
    fn sorting_layers_order_as_unity_draws_them() {
        assert!(SortingLayer::from_name("Background") < SortingLayer::from_name("Default"));
        assert!(!SortingLayer::from_name("Entities").is_above_entities());
        assert!(SortingLayer::from_name("Foreground").is_above_entities());
        assert_eq!(SortingLayer::from_name("HUD Items"), SortingLayer::Default);
    }
}
