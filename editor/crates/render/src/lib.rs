mod scene;

use std::{borrow::Cow, sync::Arc};

use bytemuck::{Pod, Zeroable};
use glyphon::{
    Attrs, Buffer, Cache, Color as TextColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
pub use scene::{Color, FrameScene, GlyphRun, Point, Quad, Rect, Size, TextFamily, TextStyle};
use thiserror::Error;
use winit::{dpi::PhysicalSize, window::Window};

const RECT_SHADER: &str = include_str!("rect.wgsl");

#[derive(Debug, Error)]
pub enum RendererError {
    #[error("failed to create the GPU surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("no compatible GPU adapter was found")]
    AdapterNotFound,
    #[error("failed to create the GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("the surface does not expose a supported texture format")]
    SurfaceFormatNotFound,
    #[error("failed to acquire the next surface texture: {0}")]
    Surface(#[from] wgpu::SurfaceError),
    #[error("failed to prepare the text atlas: {0}")]
    PrepareText(#[from] glyphon::PrepareError),
    #[error("failed to render text from the atlas: {0}")]
    RenderText(#[from] glyphon::RenderError),
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QuadInstance {
    rect: [f32; 4],
    color: [f32; 4],
    clip: [f32; 4],
    viewport: [f32; 2],
    _padding: [f32; 2],
}

struct TextState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: TextRenderer,
    buffers: Vec<Buffer>,
}

/// Owns all GPU resources for one window and consumes immutable `FrameScene`s.
pub struct Renderer {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    rect_pipeline: wgpu::RenderPipeline,
    quad_buffer: wgpu::Buffer,
    quad_capacity: usize,
    text: TextState,
}

/// The measured shape of a run of text.
///
/// The renderer is the only place that knows how the text is actually shaped,
/// so the editor asks it where every character ends up instead of guessing
/// from a font size. Guessing is how a caret ends up between two characters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeasuredText {
    /// The x offset of every byte boundary, in logical pixels.
    ///
    /// The vector has one more entry than the text has bytes: the last entry
    /// is the width of the whole run. The bytes inside one character share
    /// the character's start offset.
    pub boundaries: Vec<f32>,
    /// The width of the run.
    pub width: f32,
}

impl MeasuredText {
    /// The x offset of a byte offset within the measured text.
    pub fn offset_at(&self, byte_offset: usize) -> f32 {
        self.boundaries
            .get(byte_offset)
            .copied()
            .unwrap_or(self.width)
    }
}

impl Renderer {
    /// Measures a run of text the way the renderer would draw it.
    ///
    /// The measurement uses the same shaping path as the drawing, so a caret
    /// placed with these numbers lands exactly on the glyph.
    pub fn measure_text(&mut self, text: &str, font_size: f32, family: TextFamily) -> MeasuredText {
        if text.is_empty() {
            return MeasuredText {
                boundaries: vec![0.0],
                width: 0.0,
            };
        }

        let mut buffer = Buffer::new(
            &mut self.text.font_system,
            Metrics::new(font_size, font_size * 1.3),
        );
        buffer.set_text(
            &mut self.text.font_system,
            text,
            &Attrs::new().family(match family {
                TextFamily::SansSerif => Family::SansSerif,
                TextFamily::Monospace => Family::Monospace,
            }),
            Shaping::Advanced,
        );
        let layout = buffer.layout_runs();

        // The x offset of the start of every character cluster.
        let mut marks: Vec<Option<f32>> = vec![None; text.len() + 1];
        let mut width = 0.0;
        for run in layout {
            for glyph in run.glyphs {
                if glyph.start < marks.len() {
                    marks[glyph.start] = Some(glyph.x);
                }
            }
            width += run.line_w;
        }
        if let Some(last) = marks.last_mut() {
            *last = Some(width);
        }

        // The bytes inside one character have no glyph of their own; they
        // share the character's start offset.
        let mut last = 0.0;
        let mut boundaries = Vec::with_capacity(marks.len());
        for mark in marks {
            let offset = match mark {
                Some(offset) => {
                    last = offset;
                    offset
                }
                None => last,
            };
            boundaries.push(offset);
        }

        MeasuredText { boundaries, width }
    }
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Result<Self, RendererError> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .map_err(|_| RendererError::AdapterNotFound)?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("codeweaver-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(RendererError::SurfaceFormatNotFound)?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let rect_pipeline = create_rect_pipeline(&device, format);
        let quad_capacity = 64;
        let quad_buffer = create_quad_buffer(&device, quad_capacity);
        let cache = Cache::new(&device);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let viewport = Viewport::new(&device, &cache);
        let renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let text = TextState {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            atlas,
            viewport,
            renderer,
            buffers: Vec::new(),
        };

        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            size,
            rect_pipeline,
            quad_buffer,
            quad_capacity,
            text,
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn logical_size(&self, scale_factor: f64) -> Size {
        Size::new(
            self.size.width as f32 / scale_factor as f32,
            self.size.height as f32 / scale_factor as f32,
        )
    }

    pub fn render(&mut self, scene: &FrameScene, scale_factor: f64) -> Result<(), RendererError> {
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }
        let scale = scale_factor as f32;
        let instances: Vec<_> = scene
            .quads
            .iter()
            .filter_map(|quad| quad_instance(quad, scale, self.size))
            .collect();
        self.upload_quads(&instances);
        self.prepare_text(scene, scale)?;

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("codeweaver-frame-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("codeweaver-scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(scene.clear.into()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if !instances.is_empty() {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, self.quad_buffer.slice(..));
                pass.draw(0..6, 0..instances.len() as u32);
            }
            self.text
                .renderer
                .render(&self.text.atlas, &self.text.viewport, &mut pass)?;
        }
        self.queue.submit([encoder.finish()]);
        output.present();
        self.text.atlas.trim();
        Ok(())
    }

    fn upload_quads(&mut self, instances: &[QuadInstance]) {
        if instances.len() > self.quad_capacity {
            self.quad_capacity = instances.len().next_power_of_two();
            self.quad_buffer = create_quad_buffer(&self.device, self.quad_capacity);
        }
        if !instances.is_empty() {
            self.queue
                .write_buffer(&self.quad_buffer, 0, bytemuck::cast_slice(instances));
        }
    }

    fn prepare_text(&mut self, scene: &FrameScene, scale: f32) -> Result<(), RendererError> {
        self.text.viewport.update(
            &self.queue,
            Resolution {
                width: self.size.width,
                height: self.size.height,
            },
        );
        self.text.buffers.clear();
        for run in &scene.glyph_runs {
            let mut buffer = Buffer::new(
                &mut self.text.font_system,
                Metrics::new(run.font_size * scale, run.line_height * scale),
            );
            buffer.set_size(
                &mut self.text.font_system,
                Some(run.bounds.size.width * scale),
                Some(run.bounds.size.height * scale),
            );
            buffer.set_text(
                &mut self.text.font_system,
                &run.text,
                &Attrs::new().family(match run.family {
                    TextFamily::SansSerif => Family::SansSerif,
                    TextFamily::Monospace => Family::Monospace,
                }),
                Shaping::Advanced,
            );
            buffer.shape_until_scroll(&mut self.text.font_system, false);
            self.text.buffers.push(buffer);
        }
        let areas: Vec<_> = scene
            .glyph_runs
            .iter()
            .zip(&self.text.buffers)
            .map(|(run, buffer)| TextArea {
                buffer,
                left: run.bounds.origin.x * scale,
                top: run.bounds.origin.y * scale,
                scale: 1.0,
                bounds: TextBounds {
                    left: (run.clip.origin.x * scale) as i32,
                    top: (run.clip.origin.y * scale) as i32,
                    right: (run.clip.max_x() * scale) as i32,
                    bottom: (run.clip.max_y() * scale) as i32,
                },
                default_color: TextColor::rgba(
                    (run.color.r * 255.0) as u8,
                    (run.color.g * 255.0) as u8,
                    (run.color.b * 255.0) as u8,
                    (run.color.a * 255.0) as u8,
                ),
                custom_glyphs: &[],
            })
            .collect();
        self.text.renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.text.font_system,
            &mut self.text.atlas,
            &self.text.viewport,
            areas,
            &mut self.text.swash_cache,
        )?;
        Ok(())
    }
}

fn quad_instance(quad: &Quad, scale: f32, viewport: PhysicalSize<u32>) -> Option<QuadInstance> {
    let viewport_size = [viewport.width as f32, viewport.height as f32];
    let clip = quad.clip.intersection(Rect::new(
        0.0,
        0.0,
        viewport.width as f32 / scale,
        viewport.height as f32 / scale,
    ))?;
    Some(QuadInstance {
        rect: [
            quad.bounds.origin.x * scale,
            quad.bounds.origin.y * scale,
            quad.bounds.size.width * scale,
            quad.bounds.size.height * scale,
        ],
        color: quad.color.into(),
        clip: [
            clip.origin.x * scale,
            clip.origin.y * scale,
            clip.max_x() * scale,
            clip.max_y() * scale,
        ],
        viewport: viewport_size,
        _padding: [0.0; 2],
    })
}

fn create_quad_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("codeweaver-quad-instances"),
        size: (capacity * std::mem::size_of::<QuadInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_rect_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("codeweaver-rect-shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(RECT_SHADER)),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("codeweaver-rect-pipeline-layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("codeweaver-rect-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<QuadInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x2
                ],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}
