//! Cycle of Steel.
//!
//! The crate is split so that the game itself can run without a window or a GPU:
//!
//! - `sim` is the game: world state and one fixed-rate `step`. It knows nothing
//!   about winit, wgpu or wall-clock time, so tests can step it directly.
//! - `input` turns keys, mouse, gamepads and touch into the `sim::Input` a step
//!   consumes; `hud` is where the on-screen controls sit.
//! - `view` turns world state into sprites and owns the camera.
//! - `assets` finds and reads the files under `assets/`; `data` is their formats.
//! - `menu` is the placeholder menus; `storage` reads and writes the save.
//! - `app` is the window, the frame loop and the screenshot mode.
//!
//! On desktop `main.rs` starts the game. On Android there is no `main`: the system
//! loads this library and calls [`android_main`].

pub mod app;
pub mod assets;
pub mod audio;
pub mod data;
pub mod hud;
pub mod input;
pub mod menu;
pub mod sim;
pub mod storage;
pub mod view;

/// Android's entry point, called on the app's main thread once the library loads.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    use winit::event_loop::EventLoop;
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("cycle-of-steel"),
    );

    let result = EventLoop::builder()
        .with_android_app(android_app.clone())
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|event_loop| {
            let save = android_app.internal_data_path().map(|folder| folder.join(storage::SAVE_NAME));
            app::run(
                app::Launch {
                    assets: assets::AssetSource::Apk(android_app),
                    room: app::DEFAULT_ROOM.to_string(),
                    backends: wgpu::Backends::all(),
                    frames: None,
                    give: Vec::new(),
                    save,
                },
                event_loop,
            )
        });
    if let Err(error) = result {
        // There is no console on a phone: logcat is the only place this shows.
        log::error!("cycle-of-steel stopped: {error:#}");
    }
}
