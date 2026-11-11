# chris-wgpu

Experimenting with rendering APIs in Rust using
[`wgpu`](https://wgpu.rs/).

## Libraries

| Crate | Version | What it does here |
| --- | --- | --- |
| [`winit`](https://docs.rs/winit) | `0.30.*` | Cross-platform window creation and the event loop. The app implements the `ApplicationHandler` trait (`resumed`, `window_event`). |
| [`wgpu`](https://docs.rs/wgpu) | `30.0.*` | The graphics abstraction layer: instances, adapters, devices, queues, surfaces, render pipelines and render passes. |
| [`pollster`](https://docs.rs/pollster) | `1.0.*` | Minimal async executor. `wgpu` setup is async, so `pollster::block_on(...)` bridges it into `winit`'s synchronous callback without pulling in a full async runtime. |
| [`log`](https://docs.rs/log) / [`env_logger`](https://docs.rs/env_logger) | `0.4.*` / `0.11.*` | Declared as dependencies for future diagnostics. Not wired up yet — no `env_logger::init()` call exists, so nothing is currently logged. |

## Requirements

- A Rust toolchain with **edition 2024** support (Rust **1.85+**; developed on 1.98).
- A GPU/driver that can run the OpenGL backend (see [Current state](#current-state)).

## Build and run

```bash
# Debug build + run (opens a window)
cargo run

# Type-check only, without producing a binary
cargo check

# Optimised build
cargo build --release
```

## Project structure

```
.
├── Cargo.toml
├── Cargo.lock
└── src
    ├── main.rs                 # Entry point: creates the EventLoop, sets ControlFlow::Wait, runs the App
    ├── application.rs          # winit ApplicationHandler impl: window creation + event routing
    ├── render_state.rs         # All wgpu setup and the per-frame render() call
    └── shaders
        └── test_shader.wgsl    # WGSL vertex + fragment shader for the triangle
```

## Current state

Basic triangle demo, using opengl internally for quick launch time, see [blog post](https://garstka.dev/2026/09/05/rust-wgpu-exploration/) for more info.
