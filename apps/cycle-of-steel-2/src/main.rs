//! Desktop entry point: parses the command line and starts the game, or takes
//! a screenshot without a window.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use crystal_engine::glam::Vec2;
use cycle_of_steel_2::app;
use winit::event_loop::EventLoop;

const HELP: &str = "\
cycle-of-steel-2 [options]

  --backend <name>      vulkan, dx12, metal, gl or all (default all)
  --screenshot <path>   draw one frame off-screen to a PNG and exit
  --size <w>x<h>        screenshot size (default 1280x720)
  --ticks <n>           steps to run before the screenshot (default 0)
  --move <x>,<y>        movement held during those steps
";

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut backends = wgpu::Backends::all();
    let mut screenshot: Option<PathBuf> = None;
    let (mut width, mut height) = (1280, 720);
    let mut ticks = 0;
    let mut movement = Vec2::ZERO;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--backend" => backends = parse_backend(&value()?)?,
            "--screenshot" => screenshot = Some(PathBuf::from(value()?)),
            "--size" => {
                let size = value()?;
                let (w, h) = size.split_once('x').context("--size is <width>x<height>")?;
                (width, height) = (w.parse()?, h.parse()?);
            }
            "--ticks" => ticks = value()?.parse()?,
            "--move" => {
                let pair = value()?;
                let (x, y) = pair.split_once(',').context("--move is <x>,<y>")?;
                movement = Vec2::new(x.parse()?, y.parse()?);
            }
            "--help" | "-h" => {
                print!("{HELP}");
                return Ok(());
            }
            other => bail!("unknown option {other}\n\n{HELP}"),
        }
    }

    match screenshot {
        Some(path) => app::screenshot(app::Screenshot {
            path,
            backends,
            width,
            height,
            ticks,
            movement,
        }),
        None => app::run(backends, EventLoop::new()?),
    }
}

fn parse_backend(name: &str) -> Result<wgpu::Backends> {
    Ok(match name {
        "all" => wgpu::Backends::all(),
        "gl" | "opengl" | "gles" => wgpu::Backends::GL,
        "vulkan" | "vk" => wgpu::Backends::VULKAN,
        "dx12" | "d3d12" => wgpu::Backends::DX12,
        "metal" => wgpu::Backends::METAL,
        other => bail!("unknown backend {other}"),
    })
}
