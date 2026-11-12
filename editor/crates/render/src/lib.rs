mod scene;

use std::{borrow::Cow, collections::HashMap, sync::Arc};

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
    shaper: TextShaper,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: TextRenderer,
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

/// Measures text the way the renderer draws it.
///
/// Measuring needs the font system and the shaping, but no GPU. Keeping that
/// apart from the renderer means the measurements can be taken on their own —
/// by the benchmarks, and by anything that only needs to know where the
/// characters of a run of text are.
///
/// Shaping a line is the most expensive thing the editor does: a line of
/// thirty characters costs about as much as a frame is allowed to take. The
/// text on screen does not change from frame to frame, so every shaped run is
/// kept and reused.
pub struct TextShaper {
    font_system: FontSystem,
    shapes: ShapeCache,
}

/// How many shaped runs are kept at the same time.
///
/// The editor only draws the lines that are on screen, so a few thousand
/// runs are more than enough. When the cache is full the run that has not
/// been used for the longest time is dropped.
const SHAPE_CACHE_CAPACITY: usize = 2048;

/// The key a shaped run is cached under.
///
/// The text is part of the key: two runs with the same text, size and family
/// are shaped the same way.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ShapeKey {
    text: String,
    font_size: u32,
    line_height: u32,
    family: TextFamily,
    width: Option<u32>,
    height: Option<u32>,
}

/// A shaped run of text: the glyphs to draw and where the bytes are.
struct ShapedText {
    /// The buffer the glyphs are drawn from.
    buffer: Buffer,
    /// The x offset of every byte boundary, in the units the text was shaped
    /// at.
    boundaries: Vec<f32>,
    /// The width of the run.
    width: f32,
}

impl ShapedText {
    /// The buffer the glyphs are drawn from.
    fn buffer(&self) -> &Buffer {
        &self.buffer
    }
}

/// A handle to a shaped run in the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShapeId(usize);

/// One entry of the shape cache.
struct CacheEntry {
    key: ShapeKey,
    shaped: ShapedText,
    /// When the entry was last used, so the oldest one can be dropped.
    used: u64,
}

/// The shaped runs the editor has already paid for.
struct ShapeCache {
    entries: Vec<Option<CacheEntry>>,
    index: HashMap<ShapeKey, usize>,
    free: Vec<usize>,
    clock: u64,
}

impl ShapeCache {
    fn new() -> Self {
        ShapeCache {
            entries: Vec::new(),
            index: HashMap::new(),
            free: Vec::new(),
            clock: 0,
        }
    }

    /// Returns the run for `key`, shaping it when it is not cached yet.
    fn get_or_shape(&mut self, key: ShapeKey, shape: impl FnOnce() -> ShapedText) -> ShapeId {
        self.clock += 1;
        let clock = self.clock;

        if let Some(&index) = self.index.get(&key) {
            if let Some(entry) = self.entries[index].as_mut() {
                entry.used = clock;
            }
            return ShapeId(index);
        }

        let shaped = shape();
        let index = match self.free.pop() {
            Some(index) => {
                self.entries[index] = Some(CacheEntry {
                    key: key.clone(),
                    shaped,
                    used: clock,
                });
                index
            }
            None => {
                if self.entries.len() >= SHAPE_CACHE_CAPACITY {
                    self.drop_the_oldest();
                }
                self.entries.push(Some(CacheEntry {
                    key: key.clone(),
                    shaped,
                    used: clock,
                }));
                self.entries.len() - 1
            }
        };
        self.index.insert(key, index);
        ShapeId(index)
    }

    /// The shaped run a handle points at.
    fn get(&self, id: ShapeId) -> &ShapedText {
        &self
            .entries
            .get(id.0)
            .and_then(|entry| entry.as_ref())
            .expect("the run was cached")
            .shaped
    }

    /// Drops the run that has not been used for the longest time.
    fn drop_the_oldest(&mut self) {
        let oldest = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.as_ref().map(|entry| (entry.used, index)))
            .min_by_key(|(used, _)| *used)
            .map(|(_, index)| index);
        if let Some(index) = oldest
            && let Some(entry) = self.entries[index].take()
        {
            self.index.remove(&entry.key);
            self.free.push(index);
        }
    }
}

impl Default for TextShaper {
    fn default() -> Self {
        Self::new()
    }
}

impl TextShaper {
    pub fn new() -> Self {
        TextShaper {
            font_system: FontSystem::new(),
            shapes: ShapeCache::new(),
        }
    }

    /// Measures a run of text at `font_size`.
    ///
    /// The measurement uses the same shaping path as the drawing, so a caret
    /// placed with these numbers lands exactly on the glyph.
    pub fn measure(&mut self, text: &str, font_size: f32, family: TextFamily) -> MeasuredText {
        // The line height only moves the text up and down; the horizontal
        // positions the caret needs do not depend on it.
        let line_height = font_size * 1.3;
        let key = ShapeKey {
            text: text.to_owned(),
            font_size: font_size.to_bits(),
            line_height: line_height.to_bits(),
            family,
            width: None,
            height: None,
        };
        let TextShaper {
            font_system,
            shapes,
        } = self;
        let id = shapes.get_or_shape(key, || {
            shape_text(
                font_system,
                text,
                font_size,
                line_height,
                family,
                None,
                None,
            )
        });
        let shaped = shapes.get(id);
        MeasuredText {
            boundaries: shaped.boundaries.clone(),
            width: shaped.width,
        }
    }

    /// Drops every shaped run.
    ///
    /// The shapes are only wrong when the font or the size changes, and then
    /// all of them are wrong at once.
    pub fn clear_cache(&mut self) {
        self.shapes = ShapeCache::new();
    }

    /// Shapes a run of text the way the drawing does, and keeps it.
    ///
    /// The size of the run is part of the shape: the drawing gives a run a
    /// width so the renderer does not wrap the text a second time.
    pub fn shape(
        &mut self,
        text: &str,
        font_size: f32,
        line_height: f32,
        family: TextFamily,
        width: Option<f32>,
        height: Option<f32>,
    ) -> ShapeId {
        let key = ShapeKey {
            text: text.to_owned(),
            font_size: font_size.to_bits(),
            line_height: line_height.to_bits(),
            family,
            width: width.map(f32::to_bits),
            height: height.map(f32::to_bits),
        };
        let TextShaper {
            font_system,
            shapes,
        } = self;
        shapes.get_or_shape(key, || {
            shape_text(
                font_system,
                text,
                font_size,
                line_height,
                family,
                width,
                height,
            )
        })
    }
}

/// Shapes a run of text and works out where its byte boundaries are.
fn shape_text(
    font_system: &mut FontSystem,
    text: &str,
    font_size: f32,
    line_height: f32,
    family: TextFamily,
    width: Option<f32>,
    height: Option<f32>,
) -> ShapedText {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    if let Some(width) = width {
        buffer.set_size(font_system, Some(width), height);
    }
    buffer.set_text(
        font_system,
        text,
        &Attrs::new().family(match family {
            TextFamily::SansSerif => Family::SansSerif,
            TextFamily::Monospace => Family::Monospace,
        }),
        Shaping::Advanced,
    );
    buffer.shape_until_scroll(font_system, false);

    // The x offset of the start of every character cluster.
    let mut marks: Vec<Option<f32>> = vec![None; text.len() + 1];
    let mut run_width: f32 = 0.0;
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            if glyph.start < marks.len() {
                marks[glyph.start] = Some(glyph.x);
            }
        }
        // Every run of a line reports the width of the whole line, so the
        // runs are not added up: a line with two scripts has two runs, and
        // adding them would make the line twice as wide as it is.
        run_width = run_width.max(run.line_w);
    }
    if let Some(last) = marks.last_mut() {
        *last = Some(run_width);
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

    ShapedText {
        buffer,
        boundaries,
        width: run_width,
    }
}

impl Renderer {
    /// Measures a run of text the way the renderer would draw it.
    ///
    /// The measurement uses the same shaping path as the drawing, so a caret
    /// placed with these numbers lands exactly on the glyph.
    pub fn measure_text(&mut self, text: &str, font_size: f32, family: TextFamily) -> MeasuredText {
        self.text.shaper.measure(text, font_size, family)
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
            shaper: TextShaper::new(),
            swash_cache: SwashCache::new(),
            atlas,
            viewport,
            renderer,
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

        // Every run is shaped once and then kept: the text on screen does not
        // change from frame to frame, and shaping a line is the most
        // expensive thing the editor does.
        let mut ids = Vec::with_capacity(scene.glyph_runs.len());
        for run in &scene.glyph_runs {
            ids.push(self.text.shaper.shape(
                &run.text,
                run.font_size * scale,
                run.line_height * scale,
                run.family,
                Some(run.bounds.size.width * scale),
                Some(run.bounds.size.height * scale),
            ));
        }

        let TextState {
            shaper,
            swash_cache,
            atlas,
            viewport,
            renderer,
            ..
        } = &mut self.text;
        let TextShaper {
            font_system,
            shapes,
        } = shaper;
        let areas: Vec<_> = scene
            .glyph_runs
            .iter()
            .zip(&ids)
            .map(|(run, id)| TextArea {
                buffer: shapes.get(*id).buffer(),
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
        renderer.prepare(
            &self.device,
            &self.queue,
            font_system,
            atlas,
            viewport,
            areas,
            swash_cache,
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

#[cfg(test)]
mod tests {
    use super::*;

    const FONT_SIZE: f32 = 16.0;

    #[test]
    fn the_width_of_a_line_is_the_sum_of_its_characters() {
        let mut shaper = TextShaper::new();
        // A line with two scripts is shaped as more than one run, and every
        // run of a line reports the width of the whole line. Adding the runs
        // up would make the line twice as wide as it is.
        let text = "abc 中文";
        let measured = shaper.measure(text, FONT_SIZE, TextFamily::Monospace);
        let mut characters = 0.0;
        for character in text.chars() {
            characters += shaper
                .measure(&character.to_string(), FONT_SIZE, TextFamily::Monospace)
                .width;
        }
        assert!(
            (measured.width - characters).abs() < 0.5,
            "the line is {} wide, its characters are {characters} wide",
            measured.width
        );
    }

    #[test]
    fn a_measured_line_reports_where_every_byte_is() {
        let mut shaper = TextShaper::new();
        let measured = shaper.measure("abc", FONT_SIZE, TextFamily::Monospace);
        // One more boundary than the text has bytes.
        assert_eq!(measured.boundaries.len(), 4);
        assert_eq!(measured.boundaries[0], 0.0);
        assert!(measured.boundaries[3] > measured.boundaries[2]);
        assert!((measured.boundaries[3] - measured.width).abs() < 0.001);
    }

    #[test]
    fn the_bytes_of_one_character_share_its_start_offset() {
        let mut shaper = TextShaper::new();
        let measured = shaper.measure("中", FONT_SIZE, TextFamily::Monospace);
        // "中" is three bytes.
        assert_eq!(measured.boundaries.len(), 4);
        assert_eq!(measured.boundaries[0], 0.0);
        assert_eq!(measured.boundaries[1], 0.0);
        assert_eq!(measured.boundaries[2], 0.0);
        assert!(measured.boundaries[3] > 0.0);
    }

    #[test]
    fn a_measured_line_does_not_change_when_it_is_measured_again() {
        let mut shaper = TextShaper::new();
        let first = shaper.measure("abc", FONT_SIZE, TextFamily::Monospace);
        let second = shaper.measure("abc", FONT_SIZE, TextFamily::Monospace);
        assert_eq!(first, second);
    }

    #[test]
    fn an_empty_line_has_no_width() {
        let mut shaper = TextShaper::new();
        let measured = shaper.measure("", FONT_SIZE, TextFamily::Monospace);
        assert_eq!(measured.width, 0.0);
        assert_eq!(measured.boundaries, vec![0.0]);
    }
}
