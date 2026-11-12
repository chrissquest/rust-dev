//! wgpu drawing for sprites: the 2D counterpart of `renderer`.
//!
//! `Renderer2d` owns no window and no surface. It is given a device and the format
//! of whatever it will draw into, and records passes into an encoder the caller
//! owns. That makes it usable three ways with the same code: on top of
//! [`crate::renderer::Renderer`]'s surface frame, into an off-screen texture
//! ([`Offscreen`], used for screenshots and tests), or into a low-resolution
//! target that a later pass scales up.
//!
//! ```text
//! group 0  binding 0  Camera { view_proj }       uniform, vertex
//! group 1  binding 0  sprite sheet texture       fragment
//! group 1  binding 1  sampler                    fragment
//! vertex buffer 0     Sprite, one per instance   see `sprite::Sprite`
//! ```
//!
//! Drawing a frame:
//!
//! 1. `begin_frame()` once.
//! 2. Fill a [`DrawList`]: sprites are drawn in the order they were added, and a
//!    run of sprites sharing a texture is one draw call.
//! 3. `draw()` once per pass (world, then HUD, ...), each with its own camera.
//!
//! Sprites are alpha-blended *and* depth-tested. Add them back to front (by layer,
//! then by y) and blending is correct; the depth buffer is there so that the same
//! scene keeps working when the camera stops looking straight down.

use std::num::NonZeroU64;

use anyhow::{Result, anyhow};

use crate::math::Mat4;
use crate::renderer::Viewport;
use crate::scene::{Texture, TextureFilter};
use crate::sprite::Sprite;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const CAMERA_UNIFORM_SIZE: u64 = 64;
const INSTANCE_SIZE: u64 = std::mem::size_of::<Sprite>() as u64;
/// Room for this many dynamic sprites before the instance buffer first grows.
const INITIAL_DYNAMIC_CAPACITY: u64 = 1024;

/// A texture uploaded with [`Renderer2d::add_texture`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(u32);

impl TextureId {
    /// An id no texture has, for filling a field before the real one is known.
    /// Sprites drawn with it are skipped.
    pub const PLACEHOLDER: Self = Self(u32::MAX);
}

/// Sprites uploaded once with [`Renderer2d::add_batch`], e.g. a room's tile layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BatchId(u32);

/// What a pass draws into.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub view: &'a wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

/// Camera and clearing for one [`Renderer2d::draw`] call.
#[derive(Clone, Copy, Debug)]
pub struct Pass {
    /// World to clip space, e.g. [`crate::camera2d::Camera2d::view_projection`].
    pub view_projection: Mat4,
    /// Clear the whole target to this colour first; `None` draws over what is there.
    pub clear: Option<[f32; 4]>,
    /// Limit drawing to part of the target; `None` uses all of it.
    pub viewport: Option<Viewport>,
}

/// Everything one pass draws, in order.
#[derive(Default)]
pub struct DrawList {
    sprites: Vec<Sprite>,
    commands: Vec<Command>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Command {
    /// `count` sprites starting at `start` in `DrawList::sprites`.
    Sprites { texture: TextureId, start: u32, count: u32 },
    Batch(BatchId),
}

impl DrawList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Empties the list, keeping its allocations for the next frame.
    pub fn clear(&mut self) {
        self.sprites.clear();
        self.commands.clear();
    }

    pub fn sprite(&mut self, texture: TextureId, sprite: Sprite) {
        self.sprites(texture, std::slice::from_ref(&sprite));
    }

    pub fn sprites(&mut self, texture: TextureId, sprites: &[Sprite]) {
        if sprites.is_empty() {
            return;
        }
        let added = sprites.len() as u32;
        match self.commands.last_mut() {
            // Extend the previous run rather than starting a new draw call.
            Some(Command::Sprites { texture: last, count, .. }) if *last == texture => *count += added,
            _ => self.commands.push(Command::Sprites {
                texture,
                start: self.sprites.len() as u32,
                count: added,
            }),
        }
        self.sprites.extend_from_slice(sprites);
    }

    pub fn batch(&mut self, batch: BatchId) {
        self.commands.push(Command::Batch(batch));
    }

    /// Number of draw calls this list will issue.
    pub fn draw_calls(&self) -> usize {
        self.commands.len()
    }

    /// Number of dynamic sprites (static batches are not counted).
    pub fn sprite_count(&self) -> usize {
        self.sprites.len()
    }
}

struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

struct GpuBatch {
    buffer: wgpu::Buffer,
    count: u32,
    texture: TextureId,
}

struct CameraSlot {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct Renderer2d {
    device: wgpu::Device,
    queue: wgpu::Queue,

    pipeline: wgpu::RenderPipeline,
    camera_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    nearest_sampler: wgpu::Sampler,
    linear_sampler: wgpu::Sampler,

    textures: Vec<Option<GpuTexture>>,
    batches: Vec<Option<GpuBatch>>,

    /// One camera uniform per pass of the frame. Buffer writes all land before the
    /// frame's commands run, so two passes cannot share one buffer.
    cameras: Vec<CameraSlot>,
    cameras_used: usize,

    /// Dynamic sprites of every pass this frame, appended one pass after another.
    dynamic: wgpu::Buffer,
    dynamic_capacity: u64,
    dynamic_used: u64,

    depth: Option<(wgpu::TextureView, (u32, u32))>,
}

impl Renderer2d {
    /// `format` is the colour format of every target this renderer will draw into.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sprite.wgsl").into()),
        });

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("2d camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(CAMERA_UNIFORM_SIZE),
                },
                count: None,
            }],
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite sheet layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(INSTANCE_LAYOUT)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                // A negative size mirrors a sprite, which also reverses its winding.
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                // Equal passes, so sprites at the same height layer in draw order.
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = |label, filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let nearest_sampler = sampler("sprite nearest", wgpu::FilterMode::Nearest);
        let linear_sampler = sampler("sprite linear", wgpu::FilterMode::Linear);

        let dynamic = create_instance_buffer(device, INITIAL_DYNAMIC_CAPACITY);

        Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            camera_layout,
            texture_layout,
            nearest_sampler,
            linear_sampler,
            textures: Vec::new(),
            batches: Vec::new(),
            cameras: Vec::new(),
            cameras_used: 0,
            dynamic,
            dynamic_capacity: INITIAL_DYNAMIC_CAPACITY,
            dynamic_used: 0,
            depth: None,
        }
    }

    /// Uploads a texture. Its `filter` picks hard pixel edges or smooth sampling.
    pub fn add_texture(&mut self, texture: &Texture) -> TextureId {
        let width = texture.width.max(1);
        let height = texture.height.max(1);
        let expected = width as usize * height as usize * 4;
        let mut pixels = texture.rgba.clone();
        pixels.resize(expected, 255);

        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let gpu_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(texture.name.as_str()),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gpu_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = gpu_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = match texture.filter {
            TextureFilter::Nearest => &self.nearest_sampler,
            TextureFilter::Linear => &self.linear_sampler,
        };
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(texture.name.as_str()),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        let id = TextureId(self.textures.len() as u32);
        self.textures.push(Some(GpuTexture { bind_group }));
        id
    }

    /// Uploads sprites that will not change, to be drawn with [`DrawList::batch`].
    pub fn add_batch(&mut self, texture: TextureId, sprites: &[Sprite]) -> BatchId {
        use wgpu::util::DeviceExt;

        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sprite batch"),
            contents: bytemuck::cast_slice(sprites),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let batch = GpuBatch {
            buffer,
            count: sprites.len() as u32,
            texture,
        };

        // Reuse a freed slot so loading room after room does not grow the table.
        match self.batches.iter().position(Option::is_none) {
            Some(slot) => {
                self.batches[slot] = Some(batch);
                BatchId(slot as u32)
            }
            None => {
                self.batches.push(Some(batch));
                BatchId(self.batches.len() as u32 - 1)
            }
        }
    }

    /// Frees a static batch. Drawing its id afterwards draws nothing.
    pub fn remove_batch(&mut self, batch: BatchId) {
        if let Some(slot) = self.batches.get_mut(batch.0 as usize) {
            *slot = None;
        }
    }

    /// Call once per frame, before the frame's first [`Renderer2d::draw`].
    pub fn begin_frame(&mut self) {
        self.cameras_used = 0;
        self.dynamic_used = 0;
    }

    /// Records one render pass drawing `list` into `target`.
    pub fn draw(&mut self, encoder: &mut wgpu::CommandEncoder, target: Target<'_>, pass: &Pass, list: &DrawList) {
        let camera_slot = self.next_camera(pass.view_projection);
        let dynamic_base = self.upload_dynamic(&list.sprites);
        self.ensure_depth(target.width, target.height);
        let depth_view = &self.depth.as_ref().expect("ensure_depth just set it").0;

        let load = match pass.clear {
            Some([r, g, b, a]) => wgpu::LoadOp::Clear(wgpu::Color {
                r: r as f64,
                g: g as f64,
                b: b as f64,
                a: a as f64,
            }),
            None => wgpu::LoadOp::Load,
        };
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("2d"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                // Each pass is its own depth space: a HUD pass must not be hidden by
                // the world it is drawn over.
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        if let Some(viewport) = pass.viewport {
            if viewport.is_empty() {
                return;
            }
            render_pass.set_viewport(
                viewport.x as f32,
                viewport.y as f32,
                viewport.width as f32,
                viewport.height as f32,
                0.0,
                1.0,
            );
            render_pass.set_scissor_rect(viewport.x, viewport.y, viewport.width, viewport.height);
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.cameras[camera_slot].bind_group, &[]);

        for command in &list.commands {
            match *command {
                Command::Sprites { texture, start, count } => {
                    let Some(texture) = self.texture(texture) else {
                        continue;
                    };
                    let first = dynamic_base + start as u64;
                    render_pass.set_bind_group(1, &texture.bind_group, &[]);
                    render_pass.set_vertex_buffer(
                        0,
                        self.dynamic
                            .slice(first * INSTANCE_SIZE..(first + count as u64) * INSTANCE_SIZE),
                    );
                    render_pass.draw(0..6, 0..count);
                }
                Command::Batch(batch) => {
                    let Some(batch) = self.batches.get(batch.0 as usize).and_then(Option::as_ref) else {
                        continue;
                    };
                    let Some(texture) = self.texture(batch.texture) else {
                        continue;
                    };
                    if batch.count == 0 {
                        continue;
                    }
                    render_pass.set_bind_group(1, &texture.bind_group, &[]);
                    render_pass.set_vertex_buffer(0, batch.buffer.slice(..));
                    render_pass.draw(0..6, 0..batch.count);
                }
            }
        }
    }

    fn texture(&self, id: TextureId) -> Option<&GpuTexture> {
        self.textures.get(id.0 as usize).and_then(Option::as_ref)
    }

    /// Writes the pass's camera into the next free slot and returns its index.
    fn next_camera(&mut self, view_projection: Mat4) -> usize {
        let slot = self.cameras_used;
        self.cameras_used += 1;

        if slot == self.cameras.len() {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("2d camera uniform"),
                size: CAMERA_UNIFORM_SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("2d camera"),
                layout: &self.camera_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.cameras.push(CameraSlot { buffer, bind_group });
        }

        self.queue.write_buffer(
            &self.cameras[slot].buffer,
            0,
            bytemuck::cast_slice(&view_projection.to_cols_array()),
        );
        slot
    }

    /// Appends `sprites` to the dynamic buffer and returns the index of the first.
    fn upload_dynamic(&mut self, sprites: &[Sprite]) -> u64 {
        let count = sprites.len() as u64;
        if count == 0 {
            return self.dynamic_used;
        }

        if self.dynamic_used + count > self.dynamic_capacity {
            // Passes already recorded this frame keep the old buffer alive and keep
            // reading from it; this pass and later ones start afresh in a bigger one.
            self.dynamic_capacity = (self.dynamic_capacity * 2).max(count.next_power_of_two());
            self.dynamic = create_instance_buffer(&self.device, self.dynamic_capacity);
            self.dynamic_used = 0;
        }

        let base = self.dynamic_used;
        self.queue
            .write_buffer(&self.dynamic, base * INSTANCE_SIZE, bytemuck::cast_slice(sprites));
        self.dynamic_used += count;
        base
    }

    fn ensure_depth(&mut self, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self.depth.as_ref().is_some_and(|(_, current)| *current == size) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("2d depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.depth = Some((texture.create_view(&wgpu::TextureViewDescriptor::default()), size));
    }
}

static INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 6] = [
    // position
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    },
    // rotation
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32,
        offset: 12,
        shader_location: 1,
    },
    // size
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 16,
        shader_location: 2,
    },
    // pivot
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 24,
        shader_location: 3,
    },
    // uv
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 32,
        shader_location: 4,
    },
    // color
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 48,
        shader_location: 5,
    },
];

const INSTANCE_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: INSTANCE_SIZE,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &INSTANCE_ATTRIBUTES,
};

fn create_instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dynamic sprites"),
        size: capacity * INSTANCE_SIZE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The colour format [`Offscreen`] targets use.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A device with no window or surface, for screenshots, tests and CI.
pub async fn headless_device(backends: wgpu::Backends) -> Result<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|error| anyhow!("no suitable graphics adapter: {error:?}"))?;
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("crystal-engine headless"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::default(),
        })
        .await
        .map_err(|error| anyhow!("could not create a device: {error:?}"))
}

/// A texture to draw into and read back, in [`OFFSCREEN_FORMAT`].
pub struct Offscreen {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Offscreen {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width,
            height,
        }
    }

    pub fn target(&self) -> Target<'_> {
        Target {
            view: &self.view,
            width: self.width,
            height: self.height,
        }
    }

    /// Copies the texture back as tight RGBA8 rows, top row first. Call after the
    /// commands that draw into it have been submitted.
    pub fn read_rgba(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Vec<u8>> {
        // copy_texture_to_buffer needs each row to start on a 256-byte boundary.
        let unpadded_bytes_per_row = self.width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen readback"),
            size: bytes_per_row as u64 * self.height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offscreen readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| anyhow!("could not wait for the readback: {error:?}"))?;
        receiver
            .recv()
            .map_err(|_| anyhow!("readback was dropped"))?
            .map_err(|error| anyhow!("readback failed: {error:?}"))?;

        let mapped = slice
            .get_mapped_range()
            .map_err(|error| anyhow!("could not read the readback buffer: {error:?}"))?;
        let mut pixels = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in 0..self.height {
            let start = (row * bytes_per_row) as usize;
            pixels.extend_from_slice(&mapped[start..start + unpadded_bytes_per_row as usize]);
        }
        drop(mapped);
        readback.unmap();

        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn sprite() -> Sprite {
        Sprite::new(Vec3::ZERO, [1.0, 1.0], [0.0, 0.0, 1.0, 1.0])
    }

    #[test]
    fn sprites_sharing_a_texture_merge_into_one_draw_call() {
        let mut list = DrawList::new();
        list.sprite(TextureId(0), sprite());
        list.sprite(TextureId(0), sprite());
        list.sprite(TextureId(1), sprite());
        list.sprite(TextureId(0), sprite());
        assert_eq!(list.sprite_count(), 4);
        assert_eq!(
            list.commands,
            [
                Command::Sprites { texture: TextureId(0), start: 0, count: 2 },
                Command::Sprites { texture: TextureId(1), start: 2, count: 1 },
                Command::Sprites { texture: TextureId(0), start: 3, count: 1 },
            ]
        );
    }

    #[test]
    fn a_batch_between_sprites_keeps_the_order() {
        let mut list = DrawList::new();
        list.sprite(TextureId(0), sprite());
        list.batch(BatchId(7));
        list.sprite(TextureId(0), sprite());
        assert_eq!(list.draw_calls(), 3);
        assert_eq!(list.commands[1], Command::Batch(BatchId(7)));

        list.clear();
        assert_eq!((list.draw_calls(), list.sprite_count()), (0, 0));
    }
}
