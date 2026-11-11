//! wgpu resources and the draw loop.
//!
//! Binding layout, for reference:
//!
//! ```text
//! group 0  binding 0  Camera { view_proj }                  uniform, vertex
//! group 1  binding 0  Object { model, base_color, params,
//!                              joints[128] }                uniform, vertex+fragment
//! group 1  binding 1  base colour texture                   fragment
//! group 1  binding 2  sampler                               fragment
//! ```
//!
//! One bind group per draw item means textures and materials are resolved once
//! at load time; per frame only the uniform buffers change.

use std::num::NonZeroU64;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use bytemuck::{Pod, Zeroable};
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::camera::OrbitCamera;
use crate::math::Mat4;
use crate::scene::{Primitive, Scene, Texture, TextureFilter};

pub const MAX_JOINTS: usize = 128;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const CAMERA_UNIFORM_SIZE: u64 = 64;
/// model (64) + base colour (16) + params (16) + joint matrices (128 * 64).
const OBJECT_UNIFORM_SIZE: u64 = 64 + 16 + 16 + (MAX_JOINTS as u64 * 64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameSkip {
    Timeout,
    Occluded,
    Outdated,
    Lost,
    Validation,
}

/// The sub-rectangle of the surface the 3D scene draws into, in physical pixels.
/// Everything outside it keeps the clear colour, which is what lets a UI panel share
/// the window with the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// One acquired surface image, plus the view both the scene and the UI draw into.
pub struct SurfaceFrame {
    texture: wgpu::SurfaceTexture,
    pub view: wgpu::TextureView,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    joints: [f32; 4],
    weights: [f32; 4],
}

static VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 12,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 24,
        shader_location: 2,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 32,
        shader_location: 3,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 48,
        shader_location: 4,
    },
];

impl Vertex {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    };
}

struct GpuPrimitive {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

struct DrawItem {
    node: usize,
    mesh: usize,
    primitive: usize,
    skin: Option<usize>,
    material: Option<usize>,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,

    depth_view: wgpu::TextureView,
    depth_size: (u32, u32),

    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    object_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    wireframe_pipeline: Option<wgpu::RenderPipeline>,
    wireframe: bool,

    linear_sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    fallback_view: wgpu::TextureView,
    texture_views: Vec<wgpu::TextureView>,

    meshes: Vec<Vec<Option<GpuPrimitive>>>,
    draw_items: Vec<DrawItem>,

    /// Reused staging buffer for uniform uploads; sized for one object.
    scratch: Vec<f32>,

    adapter_name: String,
    backend: wgpu::Backend,
}

impl Renderer {
    pub async fn new(window: Arc<Window>, scene: &Scene, backends: wgpu::Backends) -> Result<Self> {
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
        let wireframe_supported = adapter
            .features()
            .contains(wgpu::Features::POLYGON_MODE_LINE);
        let required_features = if wireframe_supported {
            wgpu::Features::POLYGON_MODE_LINE
        } else {
            wgpu::Features::empty()
        };

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("crystal-engine"),
                required_features,
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

        let (depth_view, depth_size) = create_depth(&device, config.width, config.height);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/model.wgsl").into()),
        });

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
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

        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(OBJECT_UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&object_layout)],
            immediate_size: 0,
        });

        let pipeline = create_pipeline(
            &device,
            &pipeline_layout,
            &shader,
            format,
            wgpu::PolygonMode::Fill,
        );
        let wireframe_pipeline = wireframe_supported.then(|| {
            create_pipeline(
                &device,
                &pipeline_layout,
                &shader,
                format,
                wgpu::PolygonMode::Line,
            )
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: CAMERA_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let fallback_view = create_solid_texture(&device, &queue, "fallback", [255, 255, 255, 255]);

        let mut renderer = Self {
            surface,
            device,
            queue,
            config,
            size,
            depth_view,
            depth_size,
            camera_buffer,
            camera_bind_group,
            object_layout,
            pipeline,
            wireframe_pipeline,
            wireframe: false,
            linear_sampler,
            nearest_sampler,
            fallback_view,
            texture_views: Vec::new(),
            meshes: Vec::new(),
            draw_items: Vec::new(),
            scratch: Vec::with_capacity(16 + 8 + MAX_JOINTS * 16),
            adapter_name: info.name,
            backend: info.backend,
        };

        renderer.build_resources(scene);
        Ok(renderer)
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn backend(&self) -> wgpu::Backend {
        self.backend
    }

    pub fn supports_wireframe(&self) -> bool {
        self.wireframe_pipeline.is_some()
    }

    pub fn set_wireframe(&mut self, enabled: bool) {
        self.wireframe = enabled && self.wireframe_pipeline.is_some();
    }

    pub fn toggle_wireframe(&mut self) -> bool {
        self.wireframe = !self.wireframe && self.wireframe_pipeline.is_some();
        self.wireframe
    }

    /// Recreates every GPU-side resource for a newly loaded scene.
    pub fn set_scene(&mut self, scene: &Scene) {
        self.build_resources(scene);
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.size = size;
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);

        let (view, depth_size) = create_depth(&self.device, size.width, size.height);
        self.depth_view = view;
        self.depth_size = depth_size;
    }

    pub fn render(
        &mut self,
        scene: &Scene,
        world: &[Mat4],
        camera: &OrbitCamera,
    ) -> std::result::Result<(), FrameSkip> {
        let frame = self.begin_frame()?;
        let viewport = Viewport::full(self.config.width, self.config.height);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        self.encode_scene(&mut encoder, &frame, scene, world, camera, viewport);

        self.end_frame(frame, encoder);
        Ok(())
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

    pub fn size(&self) -> winit::dpi::PhysicalSize<u32> {
        self.size
    }

    /// Acquires the next surface image. `Err(FrameSkip::*)` means there is nothing to
    /// draw this frame and the caller should just try again.
    pub fn begin_frame(&mut self) -> std::result::Result<SurfaceFrame, FrameSkip> {
        if self.depth_size != (self.config.width, self.config.height) {
            let (view, depth_size) =
                create_depth(&self.device, self.config.width, self.config.height);
            self.depth_view = view;
            self.depth_size = depth_size;
        }

        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                self.resize(self.size);
                texture
            }
            wgpu::CurrentSurfaceTexture::Timeout => return Err(FrameSkip::Timeout),
            wgpu::CurrentSurfaceTexture::Occluded => return Err(FrameSkip::Occluded),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.resize(self.size);
                return Err(FrameSkip::Outdated);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.resize(self.size);
                return Err(FrameSkip::Lost);
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(FrameSkip::Validation),
        };

        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        Ok(SurfaceFrame { texture, view })
    }

    /// Records the scene into `frame`, limited to `viewport`. The colour attachment is
    /// cleared across the whole surface first, so the area outside the viewport is
    /// background rather than last frame's pixels.
    pub fn encode_scene(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &SurfaceFrame,
        scene: &Scene,
        world: &[Mat4],
        camera: &OrbitCamera,
        viewport: Viewport,
    ) {
        let aspect = viewport.width.max(1) as f32 / viewport.height.max(1) as f32;
        let view_projection = camera.view_projection(aspect);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&view_projection.to_cols_array()),
        );
        self.write_object_uniforms(scene, world);

        self.encode_pass(encoder, &frame.view, &self.depth_view, viewport);
    }

    pub fn end_frame(&self, frame: SurfaceFrame, encoder: wgpu::CommandEncoder) {
        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame.texture);
    }

    /// Renders one frame into an off-screen texture and returns it as tight
    /// RGBA8 rows. Used by `--screenshot`; needs no surface, so it works with
    /// an invisible window and on CI.
    pub fn render_offscreen(
        &mut self,
        scene: &Scene,
        world: &[Mat4],
        camera: &OrbitCamera,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>> {
        let width = width.max(1);
        let height = height.max(1);

        let aspect = width as f32 / height as f32;
        let view_projection = camera.view_projection(aspect);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&view_projection.to_cols_array()),
        );
        self.write_object_uniforms(scene, world);

        let (depth_view, _) = create_depth(&self.device, width, height);

        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        // copy_texture_to_buffer requires each row to start on a 256-byte
        // boundary; write_texture does not, which is why the upload path above
        // can be tight and this one cannot.
        let unpadded_bytes_per_row = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback"),
            size: bytes_per_row as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("screenshot"),
            });
        self.encode_pass(
            &mut encoder,
            &target_view,
            &depth_view,
            Viewport::full(width, height),
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| anyhow!("could not wait for the screenshot readback: {error:?}"))?;
        receiver
            .recv()
            .map_err(|_| anyhow!("screenshot readback was dropped"))?
            .map_err(|error| anyhow!("screenshot readback failed: {error:?}"))?;

        let mapped = slice
            .get_mapped_range()
            .map_err(|error| anyhow!("could not read the screenshot buffer: {error:?}"))?;

        // Vulkan and D3D12 hand out BGRA surface formats on Windows. The
        // renderer does not care, but a PNG does.
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );

        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * bytes_per_row) as usize;
            let end = start + unpadded_bytes_per_row as usize;
            let row_bytes = &mapped[start..end];

            if bgra {
                for pixel in row_bytes.chunks_exact(4) {
                    pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            } else {
                pixels.extend_from_slice(row_bytes);
            }
        }
        drop(mapped);
        readback.unmap();

        Ok(pixels)
    }

    fn encode_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        viewport: Viewport,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.055,
                        g: 0.062,
                        b: 0.075,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        if viewport.is_empty() {
            return;
        }
        pass.set_viewport(
            viewport.x as f32,
            viewport.y as f32,
            viewport.width as f32,
            viewport.height as f32,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(viewport.x, viewport.y, viewport.width, viewport.height);

        match self.wireframe_pipeline.as_ref().filter(|_| self.wireframe) {
            Some(pipeline) => pass.set_pipeline(pipeline),
            None => pass.set_pipeline(&self.pipeline),
        }
        pass.set_bind_group(0, &self.camera_bind_group, &[]);

        for item in &self.draw_items {
            let Some(primitive) = self
                .meshes
                .get(item.mesh)
                .and_then(|mesh| mesh.get(item.primitive))
                .and_then(Option::as_ref)
            else {
                continue;
            };

            pass.set_bind_group(1, &item.bind_group, &[]);
            pass.set_vertex_buffer(0, primitive.vertex_buffer.slice(..));
            pass.set_index_buffer(primitive.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..primitive.index_count, 0, 0..1);
        }
    }

    fn build_resources(&mut self, scene: &Scene) {
        let mut texture_views = Vec::with_capacity(scene.textures.len());
        for texture in &scene.textures {
            texture_views.push(upload_texture(&self.device, &self.queue, texture));
        }
        self.texture_views = texture_views;

        let mut meshes = Vec::with_capacity(scene.meshes.len());
        for mesh in &scene.meshes {
            let mut primitives = Vec::with_capacity(mesh.primitives.len());
            for primitive in &mesh.primitives {
                primitives.push(upload_primitive(&self.device, primitive));
            }
            meshes.push(primitives);
        }
        self.meshes = meshes;

        let mut draw_items = Vec::new();
        for (node_index, node) in scene.nodes.iter().enumerate() {
            let Some(mesh_index) = node.mesh else {
                continue;
            };
            let Some(gpu_primitives) = self.meshes.get(mesh_index) else {
                continue;
            };

            for (primitive_index, gpu_primitive) in gpu_primitives.iter().enumerate() {
                if gpu_primitive.is_none() {
                    continue;
                }
                let primitive_material =
                    scene.meshes[mesh_index].primitives[primitive_index].material;

                let uniform_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("object uniform"),
                    size: OBJECT_UNIFORM_SIZE,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });

                let material = primitive_material.and_then(|index| scene.materials.get(index));
                let texture_index = material.and_then(|material| material.base_color_texture);
                let texture_view = texture_index
                    .and_then(|index| self.texture_views.get(index))
                    .unwrap_or(&self.fallback_view);
                let filter = texture_index
                    .and_then(|index| scene.textures.get(index))
                    .map(|texture| texture.filter)
                    .unwrap_or(TextureFilter::Linear);
                let sampler = match filter {
                    TextureFilter::Linear => &self.linear_sampler,
                    TextureFilter::Nearest => &self.nearest_sampler,
                };

                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("object"),
                    layout: &self.object_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(texture_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                });

                draw_items.push(DrawItem {
                    node: node_index,
                    mesh: mesh_index,
                    primitive: primitive_index,
                    skin: node.skin,
                    material: primitive_material,
                    uniform_buffer,
                    bind_group,
                });
            }
        }
        self.draw_items = draw_items;
    }

    fn write_object_uniforms(&mut self, scene: &Scene, world: &[Mat4]) {
        let mut values = std::mem::take(&mut self.scratch);

        for item in &self.draw_items {
            let model = world.get(item.node).copied().unwrap_or(Mat4::IDENTITY);

            let material = item.material.and_then(|index| scene.materials.get(index));
            let base_color = material
                .map(|material| material.base_color)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let alpha_mask = material
                .map(|material| material.alpha_mask)
                .unwrap_or(false);

            values.clear();
            values.extend_from_slice(&model.to_cols_array());
            values.extend_from_slice(&base_color);
            values.extend_from_slice(&[if alpha_mask { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]);

            match item.skin {
                Some(skin_index) => {
                    let joints = scene.joint_matrices(world, skin_index, item.node);
                    for joint in 0..MAX_JOINTS {
                        let matrix = joints.get(joint).copied().unwrap_or(Mat4::IDENTITY);
                        values.extend_from_slice(&matrix.to_cols_array());
                    }
                }
                None => {
                    for _ in 0..MAX_JOINTS {
                        values.extend_from_slice(&Mat4::IDENTITY.to_cols_array());
                    }
                }
            }

            self.queue.write_buffer(
                &item.uniform_buffer,
                0,
                bytemuck::cast_slice(values.as_slice()),
            );
        }

        self.scratch = values;
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    polygon_mode: wgpu::PolygonMode,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("model pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(Vertex::LAYOUT)],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            // Low-poly assets often have inconsistent winding and intentionally
            // single-sided trim; drawing both faces is the friendlier default.
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_depth(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::TextureView, (u32, u32)) {
    let width = width.max(1);
    let height = height.max(1);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    (
        texture.create_view(&wgpu::TextureViewDescriptor::default()),
        (width, height),
    )
}

fn upload_primitive(device: &wgpu::Device, primitive: &Primitive) -> Option<GpuPrimitive> {
    use wgpu::util::DeviceExt;

    if primitive.positions.is_empty() || primitive.indices.len() < 3 {
        return None;
    }

    let vertices = build_vertices(primitive);
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vertices"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("indices"),
        contents: bytemuck::cast_slice(&primitive.indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    Some(GpuPrimitive {
        vertex_buffer,
        index_buffer,
        index_count: primitive.indices.len() as u32,
    })
}

fn build_vertices(primitive: &Primitive) -> Vec<Vertex> {
    let max_joint = (MAX_JOINTS - 1) as f32;

    (0..primitive.vertex_count())
        .map(|index| {
            let mut joints = primitive.joints.get(index).copied().unwrap_or([0.0; 4]);
            for joint in &mut joints {
                *joint = joint.clamp(0.0, max_joint);
            }

            Vertex {
                position: primitive.positions[index],
                normal: primitive
                    .normals
                    .get(index)
                    .copied()
                    .unwrap_or([0.0, 1.0, 0.0]),
                uv: primitive.uvs.get(index).copied().unwrap_or([0.0, 0.0]),
                joints,
                weights: primitive
                    .weights
                    .get(index)
                    .copied()
                    .unwrap_or([1.0, 0.0, 0.0, 0.0]),
            }
        })
        .collect()
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &Texture,
) -> wgpu::TextureView {
    let width = texture.width.max(1);
    let height = texture.height.max(1);
    let expected = width as usize * height as usize * 4;

    let mut pixels = texture.rgba.clone();
    if pixels.len() < expected {
        pixels.resize(expected, 255);
    }

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let gpu_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(texture.name.as_str()),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
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

    gpu_texture.create_view(&wgpu::TextureViewDescriptor::default())
}

fn create_solid_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    name: &str,
    rgba: [u8; 4],
) -> wgpu::TextureView {
    let texture = Texture {
        name: name.to_string(),
        width: 1,
        height: 1,
        rgba: rgba.to_vec(),
        filter: TextureFilter::Linear,
    };
    upload_texture(device, queue, &texture)
}
