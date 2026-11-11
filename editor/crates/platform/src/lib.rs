mod input;

use std::sync::Arc;

use anyhow::{Context, Result};
use codeweaver_editor_core::Editor;
use codeweaver_render::{Color, FrameScene, Rect, Renderer, RendererError, TextStyle};
use input::TextInputState;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, Ime, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{ImePurpose, Window, WindowAttributes, WindowId},
};

const WINDOW_TITLE: &str = "CodeWeaver";

#[derive(Default)]
pub struct NativeApplication {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    editor: Editor,
    input: TextInputState,
    modifiers: ModifiersState,
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
                    Ime::Commit(text) => {
                        if let Err(error) = self.editor.insert(&text) {
                            eprintln!("failed to commit IME text: {error}");
                        }
                        self.input.finish_commit();
                    }
                }
                window.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let result = match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Backspace) => self.editor.backspace(),
                    Key::Named(NamedKey::ArrowLeft) => {
                        self.editor.move_left();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        self.editor.move_right();
                        Ok(())
                    }
                    Key::Named(NamedKey::Enter) => self.editor.insert("\n"),
                    Key::Character(character)
                        if self.modifiers.control_key() && character.eq_ignore_ascii_case("z") =>
                    {
                        self.editor.undo()
                    }
                    Key::Character(character)
                        if self.modifiers.control_key() && character.eq_ignore_ascii_case("y") =>
                    {
                        self.editor.redo()
                    }
                    _ if self.input.preedit().is_empty()
                        && !self.modifiers.control_key()
                        && let Some(text) = event.text
                        && !text.chars().any(char::is_control) =>
                    {
                        self.editor.insert(&text)
                    }
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    eprintln!("editor input transaction failed: {error}");
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                let scene = build_demo_scene(
                    renderer.logical_size(window.scale_factor()),
                    &self.editor,
                    &self.input,
                );
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

fn build_demo_scene(
    size: codeweaver_render::Size,
    editor_state: &Editor,
    input: &TextInputState,
) -> FrameScene {
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
    let heading = format!(
        "CodeWeaver · Buffer v{} · IME {} · Undo {} · Redo {}",
        editor_state.snapshot().version(),
        if input.ime_enabled() { "ON" } else { "READY" },
        editor_state.can_undo(),
        editor_state.can_redo(),
    );
    scene.push_text(
        heading,
        Rect::new(56.0, 16.0, size.width - 72.0, 28.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.78, 0.82, 0.90),
            font_size: 15.0,
            line_height: 22.0,
        },
    );
    let caret = editor_state.selection().head.0;
    let mut text = String::with_capacity(editor_state.text().len() + input.preedit().len());
    text.push_str(&editor_state.text()[..caret]);
    text.push_str(input.preedit());
    text.push_str(&editor_state.text()[caret..]);
    if text.is_empty() {
        text = "点击窗口输入；支持中文 IME，Ctrl+Z 撤销，Ctrl+Y 重做".to_owned();
    }
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
