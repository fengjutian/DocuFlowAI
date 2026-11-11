mod input;

use std::sync::Arc;

use anyhow::{Context, Result};
use codeweaver_render::{Color, FrameScene, Rect, Renderer, RendererError, TextStyle};
use input::TextInputState;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, Ime, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey},
    window::{ImePurpose, Window, WindowAttributes, WindowId},
};

const WINDOW_TITLE: &str = "CodeWeaver";

#[derive(Default)]
pub struct NativeApplication {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    input: TextInputState,
}

impl NativeApplication {
    pub fn run(mut self) -> Result<()> {
        let event_loop = EventLoop::new().context("failed to create the native event loop")?;
        event_loop
            .run_app(&mut self)
            .context("native event loop failed")
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attributes = WindowAttributes::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(LogicalSize::new(640.0, 480.0));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("failed to create the main window")?,
        );
        let renderer = pollster::block_on(Renderer::new(window.clone()))
            .context("failed to initialize wgpu")?;
        window.set_ime_allowed(true);
        window.set_ime_purpose(ImePurpose::Normal);
        window.request_redraw();

        self.window = Some(window);
        self.renderer = Some(renderer);
        Ok(())
    }
}

impl ApplicationHandler for NativeApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(error) = self.create_window(event_loop)
        {
            eprintln!("CodeWeaver could not start: {error:#}");
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size);
                }
                window.request_redraw();
            }
            WindowEvent::Ime(ime) => {
                match ime {
                    Ime::Enabled => self.input.enable_ime(true),
                    Ime::Disabled => self.input.enable_ime(false),
                    Ime::Preedit(text, selection) => self.input.set_preedit(text, selection),
                    Ime::Commit(text) => self.input.commit(&text),
                }
                window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if event.logical_key == Key::Named(NamedKey::Backspace) {
                    self.input.delete_previous_scalar();
                } else if self.input.preedit().is_empty()
                    && let Some(text) = event.text
                    && !text.chars().any(char::is_control)
                {
                    self.input.commit(&text);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                let scene =
                    build_demo_scene(renderer.logical_size(window.scale_factor()), &self.input);
                match renderer.render(&scene, window.scale_factor()) {
                    Ok(()) => {}
                    Err(RendererError::Surface(wgpu::SurfaceError::Lost)) => {
                        renderer.resize(window.inner_size());
                        window.request_redraw();
                    }
                    Err(RendererError::Surface(wgpu::SurfaceError::OutOfMemory)) => {
                        eprintln!("GPU is out of memory; exiting");
                        event_loop.exit();
                    }
                    Err(error) => {
                        eprintln!("frame rendering failed: {error}");
                        window.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

fn build_demo_scene(size: codeweaver_render::Size, input: &TextInputState) -> FrameScene {
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let editor = Rect::new(56.0, 52.0, (size.width - 80.0).max(1.0), 180.0);
    let content_clip = Rect::new(
        editor.origin.x + 16.0,
        editor.origin.y + 16.0,
        (editor.size.width - 32.0).max(1.0),
        (editor.size.height - 32.0).max(1.0),
    );
    let mut scene = FrameScene::default();
    scene.push_quad(
        Rect::new(0.0, 0.0, 40.0, size.height),
        Color::rgb(0.075, 0.086, 0.11),
        viewport,
    );
    scene.push_quad(editor, Color::rgb(0.075, 0.086, 0.11), viewport);
    scene.push_quad(
        Rect::new(editor.origin.x, editor.origin.y, 3.0, editor.size.height),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );
    scene.push_text(
        "CodeWeaver · wgpu FrameScene",
        Rect::new(56.0, 16.0, size.width - 72.0, 28.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.78, 0.82, 0.90),
            font_size: 15.0,
            line_height: 22.0,
        },
    );
    let text = if input.display_text().is_empty() {
        "点击窗口并输入；支持中文输入法 preedit / commit".to_owned()
    } else {
        input.display_text()
    };
    scene.push_text(
        text,
        content_clip,
        TextStyle {
            clip: content_clip,
            color: Color::rgb(0.88, 0.90, 0.94),
            font_size: 18.0,
            line_height: 28.0,
        },
    );
    if !input.preedit().is_empty() {
        scene.push_quad(
            Rect::new(
                content_clip.origin.x,
                content_clip.origin.y + 27.0,
                180.0,
                2.0,
            ),
            Color::rgb(0.96, 0.72, 0.30),
            content_clip,
        );
    }
    scene
}
