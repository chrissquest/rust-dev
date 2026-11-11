//! The window: a 3D scene, the folder panel, and all of the input.
//!
//! Both binaries are thin wrappers around [`run`]:
//!
//! - `ff-view` passes a model file, an export folder, or nothing at all;
//! - `ff-browser` passes the remembered folder and keeps the panel on screen even
//!   when no folder is open.
//!
//! The panel is shown whenever a catalog is open - whether it came from the
//! command line, the remembered folder, or a drop - which is what turns dropping
//! an export folder on the window into "pick a creature and a clip".

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use egui_wgpu::{RendererOptions, ScreenDescriptor};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{ModifiersState, PhysicalKey};

use crate::input::{Action, Bindings, KeyChord};
use winit::window::{Window, WindowId};

use crystal_engine::anim::Playback;
use crystal_engine::camera::OrbitCamera;
use crystal_engine::loader;
use crystal_engine::math::{Mat4, Vec3};
use crystal_engine::renderer::{Renderer, Viewport};
use crystal_engine::scene::Scene;

use crate::library::Catalog;
use crate::settings;

/// What the window starts with.
#[derive(Debug)]
pub enum Initial {
    /// One model, already loaded.
    Scene(Scene),
    /// An export folder to pick from.
    Catalog(Catalog),
    /// Nothing: wait for a drop.
    Empty,
}

impl Default for Initial {
    fn default() -> Self {
        Self::Empty
    }
}

/// Everything the two binaries can decide before the window opens.
pub struct Launch {
    pub initial: Initial,
    pub backends: wgpu::Backends,
    /// Clip to start on, by index or name.
    pub clip: Option<String>,
    pub wireframe: bool,
    pub bind_pose: bool,
    /// Render this many frames, then exit (smoke test).
    pub frames: Option<u32>,
    /// Render one frame off-screen, save it, and exit.
    pub screenshot: Option<PathBuf>,
    /// `--view`: yaw and pitch in degrees, distance, then the orbit target x, y, z.
    /// Applied to the first scene instead of the automatic framing.
    pub view: Option<[f32; 6]>,
    /// Which keys do what. Defaults unless the binaries load the user's overrides.
    pub bindings: Bindings,
    /// Keep the folder panel visible even when no folder is open.
    pub always_panel: bool,
    /// First word of the window title.
    pub app_name: String,
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            initial: Initial::Empty,
            backends: wgpu::Backends::all(),
            clip: None,
            wireframe: false,
            bind_pose: false,
            frames: None,
            screenshot: None,
            view: None,
            bindings: Bindings::default(),
            always_panel: false,
            app_name: String::from("ff-view"),
        }
    }
}

impl Launch {
    /// The `ff-browser` personality: the panel is always there.
    pub fn browser(initial: Initial) -> Self {
        Self {
            initial,
            always_panel: true,
            app_name: String::from("ff-browser"),
            ..Self::default()
        }
    }
}

pub fn run(launch: Launch) -> Result<()> {
    let event_loop = EventLoop::new().context("could not create the event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(launch);
    event_loop.run_app(&mut app)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// A change the panel asked for, applied after the layout so the UI closure never
/// has to borrow the renderer.
enum Intent {
    ChooseFolder,
    Rescan,
    /// A creature was picked; its clip is whatever was last being watched.
    SelectCreature(usize),
    /// A specific clip was picked, which becomes the clip to keep.
    SelectClip(usize, usize),
    /// The follow-the-animation toggle was flipped.
    SetFollow(bool),
}

struct App {
    app_name: String,
    backends: wgpu::Backends,
    clip: Option<String>,
    wireframe: bool,
    bind_pose: bool,
    frames: Option<u32>,
    screenshot: Option<PathBuf>,
    /// Taken by the first scene installed.
    view: Option<[f32; 6]>,
    bindings: Bindings,
    always_panel: bool,
    /// Where the 3D view was drawn last frame, in physical pixels; picking maps the
    /// cursor through it.
    last_viewport: Viewport,

    /// Taken in `resumed`.
    pending_scene: Option<Scene>,
    pending_catalog: Option<Catalog>,

    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,

    scene: Option<Scene>,
    camera: OrbitCamera,
    playback: Playback,
    /// What the title calls the scene; `None` means "use the file name".
    label: Option<String>,

    /// The folder currently open, if any; `selected` indexes into it.
    catalog: Option<Catalog>,
    selected: Option<(usize, usize)>,
    /// The clip the user last chose, by name, so switching creatures keeps it
    /// (idle stays idle) instead of snapping back to the first clip.
    preferred_clip: Option<String>,
    folder_text: String,
    status: String,

    /// The 3D area, in points, as of the last layout.
    viewport_points: egui::Rect,
    dragging: Option<MouseButton>,
    cursor: Option<(f32, f32)>,
    modifiers: ModifiersState,
    /// How far the animated centre has moved from the rest-pose centre, as last
    /// applied to the camera. Only the change is applied each frame, so panning
    /// is not undone by the follow.
    follow: Vec3,
    /// Move the camera with the animated centre (on by default, `toggle_follow` - T).
    follow_animation: bool,
    /// Set when the follow is switched back on: the next frame adopts the current
    /// offset instead of jumping the camera by it.
    rebase_follow: bool,
    /// Ask the creature list to scroll to its selection on the next layout. Set by
    /// keyboard navigation, so arrow keys keep the highlighted dino on screen.
    reveal_selection: bool,

    last_frame: Instant,
    frames_drawn: u32,
    fps: f32,
    fps_time: f32,
    fps_frames: u32,
}

struct Gpu {
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

impl App {
    fn new(launch: Launch) -> Self {
        let Launch {
            initial,
            backends,
            clip,
            wireframe,
            bind_pose,
            frames,
            screenshot,
            view,
            bindings,
            always_panel,
            app_name,
        } = launch;

        let (pending_scene, pending_catalog) = match initial {
            Initial::Scene(scene) => (Some(scene), None),
            Initial::Catalog(catalog) => (None, Some(catalog)),
            Initial::Empty => (None, None),
        };

        let folder_text = pending_catalog
            .as_ref()
            .map(|catalog| catalog.root.display().to_string())
            .unwrap_or_default();
        let status = if always_panel {
            String::from("Drop an export folder here, or press Folder...")
        } else {
            String::from("Drop a model or an export folder on this window")
        };

        // `idle` is the sensible thing to open a creature on; `--clip` overrides
        // it, and picking another clip in the panel replaces it for the session.
        let preferred_clip = Some(clip.clone().unwrap_or_else(|| String::from("idle")));

        Self {
            app_name,
            backends,
            clip,
            wireframe,
            bind_pose,
            frames,
            screenshot,
            view,
            bindings,
            always_panel,
            last_viewport: Viewport::full(0, 0),
            pending_scene,
            pending_catalog,
            window: None,
            gpu: None,
            scene: None,
            camera: OrbitCamera::default(),
            playback: Playback::new(),
            label: None,
            catalog: None,
            selected: None,
            preferred_clip,
            folder_text,
            status,
            viewport_points: egui::Rect::ZERO,
            dragging: None,
            cursor: None,
            modifiers: ModifiersState::empty(),
            follow: Vec3::ZERO,
            follow_animation: true,
            rebase_follow: false,
            reveal_selection: false,
            last_frame: Instant::now(),
            frames_drawn: 0,
            fps: 0.0,
            fps_time: 0.0,
            fps_frames: 0,
        }
    }

    /// The panel appears with a folder, or when there is nothing on screen yet, so
    /// a bare launch is already a picker instead of a blank window. `ff-browser`
    /// keeps it either way.
    fn panel_visible(&self) -> bool {
        self.always_panel
            || self.catalog.is_some()
            || self
                .scene
                .as_ref()
                .is_none_or(|scene| scene.meshes.is_empty() && scene.terrains.is_empty())
    }

    /// The pointer position in egui points, once winit has reported one.
    fn cursor_points(&self, window: &Window) -> Option<egui::Pos2> {
        let (x, y) = self.cursor?;
        let scale = window.scale_factor() as f32;
        Some(egui::pos2(x / scale, y / scale))
    }

    /// Whether the camera should act on the pointer: anywhere when there is no
    /// panel, and only inside the 3D rectangle when there is one.
    fn camera_owns_pointer(&self, window: &Window) -> bool {
        camera_owns_pointer(
            self.panel_visible(),
            self.viewport_points,
            self.cursor_points(window),
        )
    }

    fn title(&self, scene: &Scene, status: Option<(f32, &Playback)>) -> String {
        compose_title(&self.app_name, scene, self.label.as_deref(), status)
    }

    /// The playback state a scene should start with.
    ///
    /// Inside a folder the clip the user last chose wins, so a new creature opens
    /// on the same animation - idle stays idle. A single file only honours
    /// `--clip`. Pausing and speed are about how you are watching rather than
    /// about the model, so they survive a creature switch too.
    fn playback_for(&self, scene: &Scene) -> Playback {
        // `preferred_clip` already starts at `idle` (or the `--clip` value), so a
        // scene opens on that animation whenever it has one.
        let wanted = self.preferred_clip.as_deref().or(self.clip.as_deref());

        let mut playback = Playback::new();
        if let Some(query) = wanted {
            if let Some(index) = find_clip(scene, query) {
                playback.clip = index;
            }
        }
        // Only an explicit `--clip` that this scene does not have is worth a
        // warning; the idle default is a preference, not a promise.
        if let Some(explicit) = &self.clip {
            if !scene.source.is_empty() && find_clip(scene, explicit).is_none() {
                eprintln!("warning: no clip matches `{explicit}`");
            }
        }
        if self.catalog.is_some() {
            playback.paused = self.playback.paused;
            playback.speed = self.playback.speed;
        }
        playback
    }

    /// Swaps in a scene from a file or a folder entry. `keep_camera` is set when
    /// switching within the open folder: the angle and zoom the user set up
    /// survive, and only the orbit target moves to the new model.
    fn install_scene(&mut self, scene: Scene, label: Option<String>, keep_camera: bool) {
        let title = compose_title(&self.app_name, &scene, label.as_deref(), None);
        let playback = self.playback_for(&scene);

        if let Some(gpu) = &mut self.gpu {
            gpu.renderer.set_scene(&scene);
        }

        if keep_camera {
            self.camera.retarget(&scene.bounds);
        } else {
            self.camera = OrbitCamera::default();
            self.frame_camera(&scene);
        }
        self.playback = playback;
        self.follow = Vec3::ZERO;
        self.scene = Some(scene);
        self.label = label;

        if let Some(window) = &self.window {
            window.set_title(&title);
            window.request_redraw();
        }
    }

    /// Frames `scene`, or - once, for the first scene - uses the `--view` placement.
    fn frame_camera(&mut self, scene: &Scene) {
        self.camera.frame(&scene.bounds);
        if let Some([yaw, pitch, distance, x, y, z]) = self.view.take() {
            self.camera
                .set_view(yaw.to_radians(), pitch.to_radians(), distance, Vec3::new(x, y, z));
        }
    }

    /// Shows the first entry of `catalog` and makes it the folder the next
    /// argument-less launch opens.
    fn install_catalog(&mut self, catalog: Catalog) {
        eprintln!(
            "folder      : {} ({} creatures, {} clips)",
            catalog.root.display(),
            catalog.creatures.len(),
            catalog.clip_count()
        );
        // Remembered before the first clip loads, so a folder that fails to
        // render is still the one the next launch opens. The path is printed
        // because a folder that is not remembered is otherwise undiagnosable.
        let remembered = settings::remember_folder(&catalog.root);
        if let Some(path) = &remembered {
            eprintln!("settings    : {}", path.display());
        }

        self.folder_text = catalog.root.display().to_string();
        let first = catalog.first_clip();
        self.catalog = Some(catalog);
        self.selected = None;
        self.label = None;

        match first {
            Some((creature, _)) => self.select_creature(creature),
            None => {
                self.status = String::from("That folder lists no clips");
                eprintln!("warning: that folder lists no clips");
            }
        }

        if remembered.is_none() {
            self.status
                .push_str("  (not remembered - see the console for why)");
        }
    }

    /// The clip a creature should open on: the one the user last chose, by name,
    /// or its first clip when it does not have that animation.
    fn clip_index_for(&self, creature: usize) -> usize {
        match &self.catalog {
            Some(catalog) => clip_to_carry(catalog, creature, self.preferred_clip.as_deref()),
            None => 0,
        }
    }

    /// Picks a creature and keeps the animation the user was watching.
    fn select_creature(&mut self, creature: usize) {
        let clip = self.clip_index_for(creature);
        self.select_entry(creature, clip);
    }

    /// Picks a clip explicitly, which is what future creature switches keep.
    fn choose_clip(&mut self, creature: usize, clip: usize) {
        if let Some(name) = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.clip_name(creature, clip))
        {
            self.preferred_clip = Some(name.to_string());
        }
        self.select_entry(creature, clip);
    }

    /// Loads (creature, clip) out of the open folder and shows it.
    fn select_entry(&mut self, creature: usize, clip: usize) {
        let Some(path) = self.entry_path(creature, clip) else {
            return;
        };
        let label = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.label(creature, clip));

        // The first entry of a folder frames itself; later switches keep whatever
        // angle and zoom the user is using.
        let keep_camera = self.selected.is_some();

        match loader::load(&path) {
            Ok(scene) => {
                self.status = format!(
                    "{}  ({} tris, {} joints)",
                    label.as_deref().unwrap_or("model"),
                    scene.triangle_count(),
                    scene.skins
                        .first()
                        .map(|skin| skin.joints.len())
                        .unwrap_or(0)
                );
                self.selected = Some((creature, clip));
                self.install_scene(scene, label, keep_camera);
            }
            Err(error) => {
                self.status = format!("Could not load: {error:#}");
                eprintln!("error: could not load {}: {error:#}", path.display());
            }
        }
    }

    fn entry_path(&self, creature: usize, clip: usize) -> Option<PathBuf> {
        self.catalog.as_ref()?.clip_path(creature, clip)
    }

    /// Next / previous creature that has clips. Stops at the ends instead of
    /// wrapping, so holding the key walks the list rather than looping forever.
    fn step_creature(&mut self, delta: isize) {
        let (Some(catalog), Some((creature, _))) = (&self.catalog, self.selected) else {
            return;
        };
        let Some(next) = next_creature(catalog, creature, delta) else {
            return;
        };

        // Keep the highlighted dino on screen when arrowing through the list.
        self.reveal_selection = true;
        self.select_creature(next);
    }

    /// Next / previous clip of the current creature, wrapping around.
    fn step_clip(&mut self, delta: isize) {
        let (Some(catalog), Some((creature, clip))) = (&self.catalog, self.selected) else {
            return;
        };
        let count = catalog
            .creatures
            .get(creature)
            .map(|entry| entry.clips.len())
            .unwrap_or(0) as isize;
        if count == 0 {
            return;
        }

        let next = (clip as isize + delta).rem_euclid(count) as usize;
        self.choose_clip(creature, next);
    }

    /// A drop, a remembered folder, or a path on the command line.
    fn open_path(&mut self, path: &Path) {
        if crystal_engine::terrain::is_terrain_folder(path) {
            match crystal_engine::terrain::load(path) {
                Ok(scene) => {
                    self.catalog = None;
                    self.selected = None;
                    self.status = format!("Loaded terrain {}", file_name(path));
                    self.install_scene(scene, None, false);
                }
                Err(error) => {
                    self.status = format!("Could not load terrain: {error:#}");
                    eprintln!("error: could not load terrain {}: {error:#}", path.display());
                }
            }
            return;
        }

        if path.is_dir() {
            match Catalog::open(path) {
                Ok(catalog) => self.install_catalog(catalog),
                Err(error) => {
                    self.status = format!("Could not open folder: {error:#}");
                    eprintln!("error: {error:#}");
                }
            }
            return;
        }

        match loader::load(path) {
            Ok(scene) => {
                // A loose file is its own thing; the folder listing no longer applies.
                self.catalog = None;
                self.selected = None;
                self.status = format!("Loaded {}", file_name(path));
                self.install_scene(scene, None, false);
            }
            Err(error) => {
                self.status = format!("Could not load: {error:#}");
                eprintln!("error: could not load {}: {error:#}", path.display());
            }
        }
    }

    fn world(&self) -> Option<Vec<Mat4>> {
        let scene = self.scene.as_ref()?;
        let mut pose = scene.default_pose();
        if !self.bind_pose {
            if let Some(clip) = scene.animations.get(self.playback.clip) {
                clip.apply(self.playback.time, &mut pose);
            }
        }
        Some(scene.world_transforms(&pose))
    }

    /// Does whatever `action` means. Keys never reach here directly: they go
    /// through `bindings` first, which is what makes them rebindable.
    fn act(&mut self, event_loop: &ActiveEventLoop, action: Action) {
        match action {
            Action::Quit => event_loop.exit(),
            // Navigation replaces the scene, so it runs without `scene` borrowed.
            Action::PreviousCreature => self.step_creature(-1),
            Action::NextCreature => self.step_creature(1),
            Action::PreviousClip => self.step_clip(-1),
            Action::NextClip => self.step_clip(1),
            Action::TogglePause => self.playback.paused = !self.playback.paused,
            Action::NextClipInFile | Action::PreviousClipInFile => {
                let Some(scene) = &self.scene else {
                    return;
                };
                if action == Action::NextClipInFile {
                    self.playback.next_clip(&scene.animations);
                } else {
                    self.playback.previous_clip(&scene.animations);
                }
            }
            Action::ToggleWireframe => {
                if let Some(gpu) = &mut self.gpu {
                    self.wireframe = gpu.renderer.toggle_wireframe();
                }
            }
            Action::ToggleFollow => self.set_follow(!self.follow_animation),
            Action::FocusCursor => self.focus_under_cursor(),
            Action::ResetCamera => {
                self.camera.reset();
                if let Some(scene) = &self.scene {
                    self.camera.frame(&scene.bounds);
                }
                self.follow = Vec3::ZERO;
                self.playback.restart();
            }
        }
    }

    /// Makes the surface under the mouse the orbit centre, keeping the eye where it
    /// is. Nothing under the cursor (sky, the panel) leaves the camera alone.
    fn focus_under_cursor(&mut self) {
        let Some((x, y)) = self.cursor else {
            return;
        };
        let Some(ndc) = cursor_to_ndc(self.last_viewport, x, y) else {
            return;
        };
        let Some(world) = self.world() else {
            return;
        };
        let Some(scene) = &self.scene else {
            return;
        };

        let viewport = self.last_viewport;
        let aspect = viewport.width as f32 / viewport.height as f32;
        let (origin, direction) = self.camera.ray(ndc.0, ndc.1, aspect);
        match scene.raycast(&world, origin, direction) {
            Some(distance) => {
                let point = origin + direction * distance;
                self.camera.focus_on(point);
                self.status = format!("Focus: ({:.2}, {:.2}, {:.2})", point.x, point.y, point.z);
            }
            None => self.status = String::from("Nothing under the cursor to focus on"),
        }
    }

    /// Turns the animated-centre follow on or off. Switching it back on rebases
    /// instead of applying the whole offset at once, which would snap the camera.
    fn set_follow(&mut self, on: bool) {
        self.follow_animation = on;
        self.rebase_follow = on;
    }

    /// `--screenshot`: one frame off-screen, sampled a little into the clip so a
    /// rig is not frozen in its bind pose.
    fn screenshot(&mut self, path: &Path) {
        let (width, height) = (1280, 800);
        let Some(scene) = &self.scene else {
            eprintln!("error: nothing to screenshot");
            return;
        };
        let Some(gpu) = &mut self.gpu else {
            return;
        };

        let mut pose = scene.default_pose();
        if !self.bind_pose {
            if let Some(clip) = scene.animations.get(self.playback.clip) {
                clip.apply(clip.duration * 0.3, &mut pose);
            }
        }
        let world = scene.world_transforms(&pose);

        match gpu
            .renderer
            .render_offscreen(scene, &world, &self.camera, width, height)
        {
            Ok(pixels) => match save_png(path, width, height, &pixels) {
                Ok(()) => eprintln!("screenshot  : {}", path.display()),
                Err(error) => eprintln!("error: {error:#}"),
            },
            Err(error) => eprintln!("error: {error:#}"),
        }
    }

    /// The last layout's central rect, in physical pixels and clipped to the surface.
    fn physical_viewport(&self, pixels_per_point: f32) -> Viewport {
        let size = match &self.gpu {
            Some(gpu) => gpu.renderer.size(),
            None => return Viewport::full(0, 0),
        };
        let rect = self.viewport_points;
        let x = (rect.min.x * pixels_per_point).max(0.0) as u32;
        let y = (rect.min.y * pixels_per_point).max(0.0) as u32;
        let width = ((rect.width() * pixels_per_point) as u32).min(size.width.saturating_sub(x));
        let height = ((rect.height() * pixels_per_point) as u32).min(size.height.saturating_sub(y));
        Viewport {
            x,
            y,
            width,
            height,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }

        // No model on the command line is a normal start: the window opens empty
        // and waits for a drop, or for a remembered folder.
        let scene = self.pending_scene.take().unwrap_or_default();
        let title = self.title(&scene, None);
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(LogicalSize::new(320.0, 240.0))
            // `--screenshot` needs a surface to create the device, but nothing
            // should appear on screen.
            .with_visible(self.screenshot.is_none());

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("error: could not create a window: {error}");
                event_loop.exit();
                return;
            }
        };

        let mut renderer = match pollster::block_on(Renderer::new(
            window.clone(),
            &scene,
            self.backends,
        )) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("error: {error:#}");
                event_loop.exit();
                return;
            }
        };

        eprintln!(
            "adapter     : {} ({:?})",
            renderer.adapter_name(),
            renderer.backend()
        );
        if !renderer.supports_wireframe() {
            eprintln!("wireframe   : unavailable (POLYGON_MODE_LINE not supported)");
        }
        renderer.set_wireframe(self.wireframe);

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            renderer.device(),
            renderer.format(),
            RendererOptions::default(),
        );

        self.gpu = Some(Gpu {
            renderer,
            egui_ctx,
            egui_state,
            egui_renderer,
        });
        self.window = Some(window);
        self.last_frame = Instant::now();

        self.frame_camera(&scene);
        self.playback = self.playback_for(&scene);
        self.scene = Some(scene);

        // The renderer needs a scene to exist, so a folder is opened after the
        // window is up rather than before it.
        if let Some(catalog) = self.pending_catalog.take() {
            self.install_catalog(catalog);
        }

        if self
            .scene
            .as_ref()
            .is_some_and(|scene| scene.meshes.is_empty())
        {
            eprintln!("drop a model (.glb/.gltf) or an export folder on this window");
        }

        if let Some(path) = self.screenshot.clone() {
            self.screenshot(&path);
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };

        // egui still sees everything - it needs the events for its widgets - but
        // it does not get to veto the camera. With a panel open egui reports the
        // whole window as "wanted", which used to swallow orbit, pan and zoom;
        // the pointer's position decides instead (see `camera_owns_pointer`).
        let repaint = if self.panel_visible() {
            match &mut self.gpu {
                Some(gpu) => gpu.egui_state.on_window_event(&window, &event).repaint,
                None => false,
            }
        } else {
            false
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.renderer.resize(size);
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::MouseInput { state, button, .. } => {
                match state {
                    ElementState::Pressed => {
                        // A press that starts in the panel belongs to egui; a press
                        // in the 3D area starts a camera drag even if the drag later
                        // wanders over the panel.
                        if self.camera_owns_pointer(&window) {
                            self.dragging = Some(button);
                        }
                    }
                    ElementState::Released => {
                        if self.dragging == Some(button) {
                            self.dragging = None;
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let current = (position.x as f32, position.y as f32);
                if let (Some(button), Some(previous)) = (self.dragging, self.cursor) {
                    let delta = (current.0 - previous.0, current.1 - previous.1);
                    let pan = matches!(button, MouseButton::Right | MouseButton::Middle)
                        || self.modifiers.shift_key();
                    if pan {
                        self.camera.pan(delta.0, delta.1);
                    } else {
                        self.camera.orbit(delta.0, delta.1);
                    }
                }
                self.cursor = Some(current);
            }
            WindowEvent::CursorLeft { .. } => {
                // Leaving the window ends any drag, but keeps the last position:
                // a press can only arrive while the pointer is back over the
                // window, and that position tells the camera where it happened.
                self.dragging = None;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // No click needed: the wheel works wherever the pointer is over the
                // 3D area. Over the panel it scrolls the lists instead.
                if self.dragging.is_some() || self.camera_owns_pointer(&window) {
                    let amount = match delta {
                        MouseScrollDelta::LineDelta(_, vertical) => vertical,
                        MouseScrollDelta::PixelDelta(position) => position.y as f32 * 0.02,
                    };
                    self.camera.zoom(amount);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = KeyChord::pressed(code, self.modifiers);
                let Some(action) = self.bindings.action_for(pressed) else {
                    return;
                };
                // Holding a key walks the creature list; everything else acts once
                // per press, or pause would flicker and wireframe would strobe.
                if event.repeat && !action.repeats() {
                    return;
                }
                self.act(event_loop, action);
            }
            WindowEvent::DroppedFile(path) => self.open_path(&path),
            WindowEvent::RedrawRequested => {
                self.frame(&window);
                if let Some(limit) = self.frames {
                    if self.frames_drawn >= limit {
                        event_loop.exit();
                    }
                }
                return;
            }
            _ => {}
        }

        if repaint {
            window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    fn frame(&mut self, window: &Arc<Window>) {
        let now = Instant::now();
        let delta = (now - self.last_frame).as_secs_f32().clamp(0.0, 0.1);
        self.last_frame = now;

        if let Some(scene) = &self.scene {
            self.playback.advance(delta, &scene.animations);
        }

        // ---- egui, borrowing the context out so the closure can own `self` ----
        let panel = self.panel_visible();
        let mut intents = Vec::new();
        let mut output = None;

        if panel {
            let egui_ctx = match self.gpu.as_ref() {
                Some(gpu) => gpu.egui_ctx.clone(),
                None => return,
            };
            let raw_input = match &mut self.gpu {
                Some(gpu) => gpu.egui_state.take_egui_input(window),
                None => return,
            };
            output = Some(egui_ctx.run_ui(raw_input, |ui| {
                self.draw_ui(ui, &mut intents);
            }));
        }

        // ---- apply whatever the panel asked for, which may replace the scene ----
        for intent in intents {
            match intent {
                Intent::ChooseFolder => {
                    let mut dialog = rfd::FileDialog::new().set_title("Choose an exported folder");
                    let start = self
                        .catalog
                        .as_ref()
                        .map(|catalog| catalog.root.clone())
                        .or_else(|| {
                            (!self.folder_text.is_empty()).then(|| PathBuf::from(&self.folder_text))
                        });
                    if let Some(folder) = start {
                        dialog = dialog.set_directory(folder);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.open_path(&folder);
                    }
                }
                Intent::Rescan => {
                    if let Some(folder) = self.catalog.as_ref().map(|catalog| catalog.root.clone()) {
                        self.open_path(&folder);
                    }
                }
                Intent::SelectCreature(creature) => self.select_creature(creature),
                Intent::SelectClip(creature, clip) => self.choose_clip(creature, clip),
                Intent::SetFollow(on) => self.set_follow(on),
            }
        }

        // The list has had its chance to scroll to the selection.
        self.reveal_selection = false;

        let world = self.world();

        // The camera follows the animated centre so a lunge does not walk out of
        // frame; `toggle_follow` (or the panel checkbox) turns that off. Only the *change* in
        // the offset is applied, so a middle/right-drag pan moves the camera and
        // stays moved - setting the target outright would snap it back every
        // frame. Switching the follow back on rebases instead of jumping.
        if self.follow_animation {
            if let (Some(scene), Some(world)) = (&self.scene, &world) {
                let bounds = scene.bounds_for(world);
                if !bounds.is_empty() {
                    let offset = bounds.center() - scene.bounds.center();
                    if self.rebase_follow {
                        self.follow = offset;
                        self.rebase_follow = false;
                    } else {
                        self.camera.target += offset - self.follow;
                        self.follow = offset;
                    }
                }
            }
        }

        let pixels_per_point = output
            .as_ref()
            .map(|output| output.pixels_per_point)
            .unwrap_or(1.0);
        let viewport = if panel {
            self.physical_viewport(pixels_per_point)
        } else {
            match self.gpu.as_ref() {
                Some(gpu) => {
                    let size = gpu.renderer.size();
                    Viewport::full(size.width, size.height)
                }
                None => Viewport::full(0, 0),
            }
        };
        self.last_viewport = viewport;

        let Some(gpu) = &mut self.gpu else {
            return;
        };

        let mut primitives = Vec::new();
        let mut screen = None;

        // Textures are freed after the frame is submitted, so keep the output around.
        if let Some(output) = &mut output {
            gpu.egui_state
                .handle_platform_output(window, output.platform_output.clone());
            primitives = gpu
                .egui_ctx
                .tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);

            let size = gpu.renderer.size();
            screen = Some(ScreenDescriptor {
                size_in_pixels: [size.width, size.height],
                pixels_per_point: output.pixels_per_point,
            });

            for (id, deltas) in &output.textures_delta.set {
                for delta in deltas {
                    gpu.egui_renderer.update_texture(
                        gpu.renderer.device(),
                        gpu.renderer.queue(),
                        *id,
                        delta,
                    );
                }
            }
        }

        let frame = match gpu.renderer.begin_frame() {
            Ok(frame) => frame,
            Err(_) => {
                // epaint panics if a delta with entries is dropped unhandled.
                if let Some(output) = &mut output {
                    output.textures_delta.clear();
                }
                return;
            }
        };

        let mut encoder = gpu
            .renderer
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        if let (Some(scene), Some(world)) = (&self.scene, &world) {
            gpu.renderer.encode_scene(
                &mut encoder,
                &frame,
                scene,
                world,
                &self.camera,
                viewport,
            );
        }

        if let Some(screen) = &screen {
            gpu.egui_renderer.update_buffers(
                gpu.renderer.device(),
                gpu.renderer.queue(),
                &mut encoder,
                &primitives,
                screen,
            );

            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &frame.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                let mut pass = pass.forget_lifetime();
                gpu.egui_renderer.render(&mut pass, &primitives, screen);
            }
        }

        gpu.renderer.end_frame(frame, encoder);

        if let Some(output) = &mut output {
            for id in &output.textures_delta.free {
                gpu.egui_renderer.free_texture(id);
            }
            // Same reason as above: every delta has to be accounted for.
            output.textures_delta.clear();
        }

        self.frames_drawn += 1;
        self.fps_time += delta;
        self.fps_frames += 1;
        if self.fps_time >= 0.5 {
            self.fps = self.fps_frames as f32 / self.fps_time;
            self.fps_time = 0.0;
            self.fps_frames = 0;

            if let (Some(window), Some(scene)) = (&self.window, &self.scene) {
                let title = compose_title(
                    &self.app_name,
                    scene,
                    self.label.as_deref(),
                    Some((self.fps, &self.playback)),
                );
                window.set_title(&title);
            }
        }
    }

    fn draw_ui(&mut self, root: &mut egui::Ui, intents: &mut Vec<Intent>) {
        egui::Panel::left("browser")
            .resizable(true)
            .default_size(320.0)
            .show(root, |ui| {
                ui.add_space(4.0);
                ui.heading("Fossil Fighters");
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if ui.button("Folder...").clicked() {
                        intents.push(Intent::ChooseFolder);
                    }
                    if ui.button("Rescan").clicked() {
                        intents.push(Intent::Rescan);
                    }
                });
                ui.add(
                    egui::Label::new(egui::RichText::new(&self.folder_text).small().weak())
                        .truncate(),
                );
                ui.separator();

                match &self.catalog {
                    Some(catalog) => {
                        ui.label(format!("{} creatures", catalog.creatures.len()));
                        let selected_creature = self.selected.map(|(creature, _)| creature);
                        let reveal = self.reveal_selection;
                        egui::ScrollArea::vertical()
                            .id_salt("creatures")
                            .max_height(240.0)
                            .show(ui, |ui| {
                                for (index, creature) in catalog.creatures.iter().enumerate() {
                                    let label = format!("{:>3}  {}", creature.id, creature.name);
                                    let row = ui
                                        .selectable_label(selected_creature == Some(index), label);
                                    if reveal && selected_creature == Some(index) {
                                        // Keyboard navigation only: keep the focused
                                        // creature visible without pinning the list
                                        // to it while the user scrolls by hand.
                                        row.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if row.clicked() {
                                        intents.push(Intent::SelectCreature(index));
                                    }
                                }
                            });

                        ui.separator();

                        if let Some((creature, current)) = self.selected {
                            if let Some(entry) = catalog.creatures.get(creature) {
                                ui.label("Clips");
                                egui::ScrollArea::vertical()
                                    .id_salt("clips")
                                    .max_height(180.0)
                                    .show(ui, |ui| {
                                        for (index, clip) in entry.clips.iter().enumerate() {
                                            let label = format!(
                                                "{:<16} {:.2}s",
                                                clip.name, clip.seconds
                                            );
                                            if ui
                                                .selectable_label(index == current, label)
                                                .clicked()
                                            {
                                                intents.push(Intent::SelectClip(creature, index));
                                            }
                                        }
                                    });
                            }
                        }

                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.playback.paused, "Pause");
                            ui.label(format!("{:.2}s", self.playback.time));
                        });
                        ui.add(
                            egui::Slider::new(&mut self.playback.speed, 0.1..=2.0).text("speed"),
                        );
                        ui.add_space(4.0);
                        let mut follow = self.follow_animation;
                        if ui
                            .checkbox(&mut follow, "Follow animation")
                            .on_hover_text(
                                "Move the camera with the model instead of leaving it fixed",
                            )
                            .changed()
                        {
                            intents.push(Intent::SetFollow(follow));
                        }
                    }
                    None => {
                        ui.label("No folder open.");
                    }
                }

                ui.separator();
                ui.label(egui::RichText::new(&self.status).small());
                ui.label(
                    egui::RichText::new(
                        "drag orbit · right drag pan · wheel zoom · \
                         up/down creature · left/right clip · space pause · F follow · R reframe",
                    )
                    .small()
                    .weak(),
                );
            });

        let central = egui::CentralPanel::no_frame().show(root, |ui| {
            ui.allocate_space(ui.available_size());
        });

        // Where the 3D goes; the renderer needs it next frame, in physical pixels.
        self.viewport_points = central.response.rect;
    }
}

// ---------------------------------------------------------------------------
// Odds and ends
// ---------------------------------------------------------------------------

/// Whether pointer input belongs to the 3D view rather than the folder panel.
///
/// Without a panel the whole window is the view, so every event counts. With one,
/// the 3D view owns exactly the rectangle it was laid out into - which is also why
/// dragging a slider or scrolling the creature list does not move the camera.
///
/// This deliberately does not consult egui's `consumed` flag: with a panel open,
/// egui reports that it wants the pointer for the whole window, and asking it
/// would disable orbit, pan and zoom.
fn camera_owns_pointer(panel: bool, viewport: egui::Rect, pointer: Option<egui::Pos2>) -> bool {
    if !panel {
        return true;
    }
    pointer.is_some_and(|position| viewport.contains(position))
}

/// Which clip a creature opens on when the user was watching `preferred`: the
/// clip with that name if the creature has one, otherwise its first.
fn clip_to_carry(catalog: &Catalog, creature: usize, preferred: Option<&str>) -> usize {
    preferred
        .and_then(|name| catalog.clip_index(creature, name))
        .unwrap_or(0)
}

/// The next creature with clips in `delta`'s direction, or `None` at the ends.
///
/// Skipping entries without clips matters because some exports have them, and
/// stopping rather than wrapping is what makes holding the arrow key feel like
/// scrolling a list instead of cycling.
fn next_creature(catalog: &Catalog, creature: usize, delta: isize) -> Option<usize> {
    if delta == 0 {
        return None;
    }

    let count = catalog.creatures.len() as isize;
    let mut index = creature as isize;
    loop {
        index += delta.signum();
        if index < 0 || index >= count {
            return None;
        }
        if !catalog.creatures[index as usize].clips.is_empty() {
            return Some(index as usize);
        }
    }
}

/// A cursor position (physical pixels, window space) as normalised device
/// coordinates of the 3D viewport, or `None` when it is outside the viewport.
fn cursor_to_ndc(viewport: Viewport, x: f32, y: f32) -> Option<(f32, f32)> {
    if viewport.is_empty() {
        return None;
    }
    let u = (x - viewport.x as f32) / viewport.width as f32;
    let v = (y - viewport.y as f32) / viewport.height as f32;
    ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then(|| (u * 2.0 - 1.0, 1.0 - v * 2.0))
}

/// The window title. A scene with no source is the empty drop window, `label`
/// wins over the file name (folder entries are `T-Rex - attack`), and the clip
/// name is only appended when the label does not already contain it.
fn compose_title(
    app_name: &str,
    scene: &Scene,
    label: Option<&str>,
    status: Option<(f32, &Playback)>,
) -> String {
    if label.is_none() && scene.source.is_empty() {
        return format!("{app_name} - drop a model or an export folder");
    }

    let name = label.map(str::to_string).unwrap_or_else(|| {
        Path::new(&scene.source)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| scene.source.clone())
    });

    let mut title = format!(
        "{app_name} - {name} - {} tris",
        format_count(scene.triangle_count())
    );

    if let Some((fps, playback)) = status {
        title.push_str(&format!(" - {fps:.0} fps"));
        if label.is_none() {
            if let Some(clip) = scene.animations.get(playback.clip) {
                let state = if playback.paused { "paused" } else { "playing" };
                title.push_str(&format!(" - {} ({state})", clip.name));
            }
        }
    }

    title
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(character);
    }
    out
}

fn find_clip(scene: &Scene, query: &str) -> Option<usize> {
    if let Ok(index) = query.parse::<usize>() {
        if index < scene.animations.len() {
            return Some(index);
        }
    }
    let needle = query.to_ascii_lowercase();
    scene
        .animations
        .iter()
        .position(|animation| animation.name.to_ascii_lowercase() == needle)
        .or_else(|| {
            scene
                .animations
                .iter()
                .position(|animation| animation.name.to_ascii_lowercase().contains(&needle))
        })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file = std::fs::File::create(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    let writer = std::io::BufWriter::new(file);

    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);

    let mut writer = encoder
        .write_header()
        .with_context(|| format!("could not start the PNG for {}", path.display()))?;
    writer
        .write_image_data(rgba)
        .with_context(|| format!("could not write {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    #[test]
    fn an_empty_scene_titles_itself_as_a_drop_target() {
        let scene = Scene::default();
        assert_eq!(
            compose_title("ff-view", &scene, None, None),
            "ff-view - drop a model or an export folder"
        );
    }

    #[test]
    fn a_file_is_named_by_its_file_name() {
        let mut scene = Scene::default();
        scene.source = r"C:\models\glb\trex_001_attack.glb".to_string();
        assert_eq!(
            compose_title("ff-view", &scene, None, None),
            "ff-view - trex_001_attack.glb - 0 tris"
        );
    }

    #[test]
    fn a_folder_entry_is_named_by_its_label() {
        let mut scene = Scene::default();
        scene.source = r"C:\models\glb\trex_001_attack.glb".to_string();
        assert_eq!(
            compose_title("ff-view", &scene, Some("T-Rex - attack"), None),
            "ff-view - T-Rex - attack - 0 tris"
        );
    }

    fn viewport() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(320.0, 0.0), egui::vec2(960.0, 800.0))
    }

    #[test]
    fn without_a_panel_the_whole_window_drives_the_camera() {
        assert!(camera_owns_pointer(false, viewport(), None));
        assert!(camera_owns_pointer(
            false,
            viewport(),
            Some(egui::pos2(10.0, 10.0))
        ));
    }

    #[test]
    fn with_a_panel_only_the_viewport_drives_the_camera() {
        let viewport = viewport();
        assert!(camera_owns_pointer(
            true,
            viewport,
            Some(egui::pos2(800.0, 400.0))
        ));
        assert!(!camera_owns_pointer(
            true,
            viewport,
            Some(egui::pos2(120.0, 400.0))
        ));
        assert!(!camera_owns_pointer(true, viewport, None));
    }

    fn catalog(clips: &[&[&str]]) -> Catalog {
        Catalog {
            root: std::path::PathBuf::from("C:\\models"),
            scanned: false,
            creatures: clips
                .iter()
                .enumerate()
                .map(|(index, names)| crate::library::Creature {
                    id: index + 1,
                    name: format!("creature {index}"),
                    clips: names
                        .iter()
                        .map(|name| crate::library::Clip {
                            name: (*name).to_string(),
                            seconds: 1.0,
                            glb: format!("glb/{name}.glb"),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn switching_creatures_keeps_the_clip_by_name() {
        let catalog = catalog(&[
            &["attack", "roar", "idle"],
            // Same animation, different position in the list.
            &["idle", "attack", "roar"],
            // This one has neither idle nor roar.
            &["attack", "cheer"],
        ]);

        assert_eq!(clip_to_carry(&catalog, 0, Some("idle")), 2);
        assert_eq!(clip_to_carry(&catalog, 1, Some("idle")), 0);
        assert_eq!(clip_to_carry(&catalog, 2, Some("idle")), 0);
        assert_eq!(clip_to_carry(&catalog, 1, Some("ROAR")), 2);
    }

    #[test]
    fn without_a_chosen_clip_a_creature_opens_on_its_first() {
        let catalog = catalog(&[&["attack", "idle"]]);
        assert_eq!(clip_to_carry(&catalog, 0, None), 0);
    }

    #[test]
    fn a_folder_opens_on_idle_by_default() {
        let app = App::new(Launch::default());
        assert_eq!(app.preferred_clip.as_deref(), Some("idle"));
        assert!(app.follow_animation, "the follow should be on by default");
    }

    #[test]
    fn an_explicit_clip_wins_over_the_idle_default() {
        let app = App::new(Launch {
            clip: Some(String::from("roar")),
            ..Launch::default()
        });
        assert_eq!(app.preferred_clip.as_deref(), Some("roar"));
    }

    #[test]
    fn creature_stepping_stops_at_the_ends() {
        let catalog = catalog(&[&["attack"], &["attack"], &["attack"]]);
        assert_eq!(next_creature(&catalog, 0, -1), None);
        assert_eq!(next_creature(&catalog, 2, 1), None);
        assert_eq!(next_creature(&catalog, 0, 1), Some(1));
        assert_eq!(next_creature(&catalog, 2, -1), Some(1));
    }

    #[test]
    fn creature_stepping_skips_entries_without_clips() {
        let catalog = catalog(&[&["attack"], &[], &["attack"]]);
        assert_eq!(next_creature(&catalog, 0, 1), Some(2));
        assert_eq!(next_creature(&catalog, 2, -1), Some(0));
    }

    #[test]
    fn only_the_creature_arrows_repeat_when_held() {
        let bindings = Bindings::default();
        let held = |code| {
            bindings
                .action_for(KeyChord::key(code))
                .is_some_and(Action::repeats)
        };
        assert!(held(KeyCode::ArrowUp));
        assert!(held(KeyCode::ArrowDown));
        assert!(!held(KeyCode::Space));
        assert!(!held(KeyCode::KeyW));
        assert!(!held(KeyCode::KeyF));
    }

    #[test]
    fn the_cursor_maps_into_the_viewport_not_the_window() {
        // A 960x800 view to the right of a 320 px panel.
        let viewport = Viewport {
            x: 320,
            y: 0,
            width: 960,
            height: 800,
        };
        assert_eq!(cursor_to_ndc(viewport, 800.0, 400.0), Some((0.0, 0.0)));
        assert_eq!(cursor_to_ndc(viewport, 320.0, 0.0), Some((-1.0, 1.0)));
        assert_eq!(cursor_to_ndc(viewport, 1280.0, 800.0), Some((1.0, -1.0)));
        assert_eq!(cursor_to_ndc(viewport, 100.0, 400.0), None, "over the panel");
        assert_eq!(cursor_to_ndc(Viewport::full(0, 0), 1.0, 1.0), None);
    }
}
