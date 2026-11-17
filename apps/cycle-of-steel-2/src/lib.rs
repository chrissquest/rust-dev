//! Cycle of Steel 2: Parts Collector.
//!
//! A new game in Cycle of Steel's vein, not a port: nothing here has to match
//! Unity. The design and plan live in the Cycle of Steel 2 doc (linked from
//! the README). The crate keeps Cycle of Steel 1's split, which earned its keep:
//!
//! - `sim` is the game: world state and one fixed-rate `step`. It knows nothing
//!   about winit, wgpu or wall-clock time, so tests can step it directly.
//! - `view` turns world state into sprites and owns the camera; `art` is the
//!   placeholder pixel art it draws with.
//! - `touch` is the phone's twin sticks.
//! - `app` is the window, the frame loop and the screenshot mode.
//!
//! On desktop `main.rs` starts the game. On Android the system loads this
//! library and calls [`android_main`].

pub mod app;
pub mod art;
pub mod sim;
pub mod touch;
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
            .with_tag("cycle-of-steel-2"),
    );

    let result = EventLoop::builder()
        .with_android_app(android_app)
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|event_loop| app::run(wgpu::Backends::all(), event_loop));
    if let Err(error) = result {
        // There is no console on a phone: logcat is the only place this shows.
        log::error!("cycle-of-steel-2 stopped: {error:#}");
    }
}
