//! `cycle-of-steel` - the game's executable. See the library crate for the layout.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use crystal_engine::glam::Vec2;
use cycle_of_steel::app::{self, Launch, Screenshot};
use cycle_of_steel::assets::AssetSource;
use winit::event_loop::EventLoop;

const USAGE: &str = "\
cycle-of-steel - top-down roguelike shooter

USAGE:
    cycle-of-steel [options]

OPTIONS:
    --assets <folder>       use this assets folder (default: beside the executable,
                            or the COS_ASSETS environment variable)
    --backend <name>        gl | vulkan | dx12 | metal | all (default: all)
    --frames <n>            draw n frames, then exit (smoke test)
    --screenshot <file.png> run without a window, save one frame, and exit
    --size <WxH>            screenshot size in pixels (default: 1280x720)
    --ticks <n>             simulation steps to run before the screenshot (default: 0)
    --move <x,y>            direction held during those steps (default: 0,0)
    --touch <x0,y0,x1,y1>   a finger held during those steps: down at x0,y0 and
                            dragged to x1,y1, in pixels from the top-left
    -h, --help              show this text

KEYS:
    W A S D / arrows        move (or a gamepad's left stick / d-pad)
    F11                     toggle fullscreen
    Escape                  quit
";

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        // wgpu's GLES backend warns once per buffer upload; noise on every launch.
        env_logger::Env::default().default_filter_or("warn,wgpu_hal::gles=error"),
    )
    .init();

    let Some(options) = parse_args()? else {
        print!("{USAGE}");
        return Ok(());
    };

    let assets = AssetSource::folder(options.assets.as_deref())?;
    match options.screenshot {
        Some(path) => app::screenshot(Screenshot {
            path,
            assets,
            backends: options.backends,
            width: options.size.0,
            height: options.size.1,
            ticks: options.ticks,
            movement: options.movement,
            touch: options.touch,
        }),
        None => app::run(
            Launch {
                assets,
                backends: options.backends,
                frames: options.frames,
            },
            EventLoop::new().context("could not create the event loop")?,
        ),
    }
}

struct Options {
    assets: Option<PathBuf>,
    backends: wgpu::Backends,
    frames: Option<u32>,
    screenshot: Option<PathBuf>,
    size: (u32, u32),
    ticks: u32,
    movement: Vec2,
    touch: Option<(Vec2, Vec2)>,
}

fn parse_args() -> Result<Option<Options>> {
    let mut options = Options {
        assets: None,
        backends: wgpu::Backends::all(),
        frames: None,
        screenshot: None,
        size: (1280, 720),
        ticks: 0,
        movement: Vec2::ZERO,
        touch: None,
    };

    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        let mut value = |what: &str| args.next().with_context(|| format!("{argument} needs {what}"));
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--assets" => options.assets = Some(PathBuf::from(value("a folder")?)),
            "--backend" => options.backends = parse_backends(&value("a name")?)?,
            "--frames" => {
                let count = parse_number::<u32>(&value("a count")?, "a frame count")?;
                options.frames = (count > 0).then_some(count);
            }
            "--screenshot" => options.screenshot = Some(PathBuf::from(value("a path")?)),
            "--size" => options.size = parse_size(&value("WxH")?)?,
            "--ticks" => options.ticks = parse_number(&value("a count")?, "a step count")?,
            "--move" => options.movement = parse_direction(&value("x,y")?)?,
            "--touch" => options.touch = Some(parse_touch(&value("x0,y0,x1,y1")?)?),
            other => bail!("unknown option `{other}`\n\n{USAGE}"),
        }
    }
    Ok(Some(options))
}

fn parse_number<T: std::str::FromStr>(text: &str, what: &str) -> Result<T> {
    match text.trim().parse() {
        Ok(number) => Ok(number),
        Err(_) => bail!("`{text}` is not {what}"),
    }
}

/// `1280x720` -> (1280, 720).
fn parse_size(text: &str) -> Result<(u32, u32)> {
    let (width, height) = text
        .split_once(['x', 'X'])
        .with_context(|| format!("`{text}` is not a size like 1280x720"))?;
    let width: u32 = parse_number(width, "a width")?;
    let height: u32 = parse_number(height, "a height")?;
    if width == 0 || height == 0 {
        bail!("`{text}`: a screenshot needs at least one pixel each way");
    }
    Ok((width, height))
}

/// `1,0` -> right; `-1,1` -> up and left.
fn parse_direction(text: &str) -> Result<Vec2> {
    let (x, y) = text
        .split_once(',')
        .with_context(|| format!("`{text}` is not a direction like 1,0"))?;
    Ok(Vec2::new(
        parse_number(x, "a number")?,
        parse_number(y, "a number")?,
    ))
}

/// `200,500,260,440` -> down at (200, 500), dragged to (260, 440).
fn parse_touch(text: &str) -> Result<(Vec2, Vec2)> {
    let numbers = text
        .split(',')
        .map(|part| parse_number::<f32>(part, "a number"))
        .collect::<Result<Vec<f32>>>()?;
    match numbers[..] {
        [x0, y0, x1, y1] => Ok((Vec2::new(x0, y0), Vec2::new(x1, y1))),
        _ => bail!("`{text}` is not a touch like 200,500,260,440"),
    }
}

fn parse_backends(value: &str) -> Result<wgpu::Backends> {
    Ok(match value.trim().to_ascii_lowercase().as_str() {
        "gl" | "opengl" | "gles" => wgpu::Backends::GL,
        "vulkan" | "vk" => wgpu::Backends::VULKAN,
        "dx12" | "d3d12" => wgpu::Backends::DX12,
        "metal" => wgpu::Backends::METAL,
        "all" | "" => wgpu::Backends::all(),
        other => bail!("unknown backend `{other}` (try gl, vulkan, dx12, metal or all)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_directions_parse() {
        assert_eq!(parse_size("1920x1080").unwrap(), (1920, 1080));
        assert!(parse_size("0x10").is_err());
        assert!(parse_size("wide").is_err());
        assert_eq!(parse_direction("-1, 0.5").unwrap(), Vec2::new(-1.0, 0.5));
        assert!(parse_direction("1").is_err());
        assert_eq!(
            parse_touch("200,500,260,440").unwrap(),
            (Vec2::new(200.0, 500.0), Vec2::new(260.0, 440.0))
        );
        assert!(parse_touch("1,2,3").is_err());
    }
}
