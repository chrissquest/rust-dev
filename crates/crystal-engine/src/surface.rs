//! A window's swapchain and the device that draws to it, with no pipelines attached.
//!
//! [`crate::renderer::Renderer`] sets up its own surface together with the 3D
//! pipelines. An app that only draws through [`crate::render2d`] uses this instead:
//! it gets a device, a queue and a frame to draw into, and pays for nothing else.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::renderer::FrameSkip;

/// One acquired surface image. Draw into `view`, then hand it to
/// [`WindowSurface::present`].
pub struct Frame {
    texture: wgpu::SurfaceTexture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

pub struct WindowSurface {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    adapter_name: String,
    backend: wgpu::Backend,
}

impl WindowSurface {
    pub async fn new(window: Arc<Window>, backends: wgpu::Backends) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| anyhow!("could not create a surface: {error}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| anyhow!("no suitable graphics adapter: {error:?}"))?;
        let info = adapter.get_info();

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("crystal-engine"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: wgpu::Trace::default(),
            })
            .await
            .map_err(|error| anyhow!("could not create a device: {error:?}"))?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(capabilities.formats[0]);
        let present_mode = capabilities
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::AutoVsync)
            .unwrap_or(capabilities.present_modes[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            adapter_name: info.name,
            backend: info.backend,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn backend(&self) -> wgpu::Backend {
        self.backend
    }

    /// Call when the window's size changes. A zero size (minimised) is ignored.
    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Acquires the next image. `Err` means there is nothing to draw into this time
    /// and the caller should simply try again next frame.
    pub fn acquire(&mut self) -> std::result::Result<Frame, FrameSkip> {
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                // Still drawable; reconfigure so the next one fits.
                self.surface.configure(&self.device, &self.config);
                texture
            }
            wgpu::CurrentSurfaceTexture::Timeout => return Err(FrameSkip::Timeout),
            wgpu::CurrentSurfaceTexture::Occluded => return Err(FrameSkip::Occluded),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Err(FrameSkip::Outdated);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Err(FrameSkip::Lost);
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(FrameSkip::Validation),
        };

        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Frame {
            width: texture.texture.width(),
            height: texture.texture.height(),
            texture,
            view,
        })
    }

    /// Submits the frame's commands and shows the image.
    pub fn present(&self, frame: Frame, encoder: wgpu::CommandEncoder) {
        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame.texture);
    }
}
