mod agent;
mod input;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use codeweaver_agent_acp::ToolStatus;
use codeweaver_editor_core::Editor;
use codeweaver_render::{Color, FrameScene, Rect, Renderer, RendererError, TextStyle};
use codeweaver_workspace::{Document, Workspace};
use input::TextInputState;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, Ime, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{ImePurpose, Window, WindowAttributes, WindowId},
};

use agent::AgentSession;
pub use agent::EndpointSettings;

const WINDOW_TITLE: &str = "CodeWeaver";

pub struct NativeApplication {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    editor: Editor,
    input: TextInputState,
    modifiers: ModifiersState,
    /// The agent session, started when the user asks for it.
    agent: Option<AgentSession>,
    /// Why the agent session could not be started, if that happened.
    agent_error: Option<String>,
    /// The workspace services. The UI never touches the file system itself.
    workspace: Workspace,
    /// The file that is open, if any.
    document: Option<Document>,
    /// The last save or load error, shown in the status line.
    status: Option<String>,
    /// How to start the agent endpoint, when it is configured explicitly.
    endpoint: Option<EndpointSettings>,
}

impl NativeApplication {
    /// Creates the application, opening `path` when one is given.
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        let mut editor = Editor::new("");
        let workspace = Workspace::detached();
        let mut document = None;

        if let Some(path) = path {
            let opened = workspace
                .open(&path)
                .map_err(|error| anyhow::anyhow!("could not open {}: {error}", path.display()))?;
            editor.load_text(opened.saved_text());
            document = Some(opened);
        }

        Ok(NativeApplication {
            window: None,
            renderer: None,
            editor,
            input: TextInputState::default(),
            modifiers: ModifiersState::default(),
            agent: None,
            agent_error: None,
            workspace,
            document,
            status: None,
            endpoint: None,
        })
    }

    /// Sets how the agent endpoint is started.
    ///
    /// Without this, the endpoint is taken from the environment. Setting it
    /// explicitly lets a caller point the editor at a different endpoint
    /// without touching the process environment.
    pub fn set_endpoint(&mut self, settings: EndpointSettings) {
        self.endpoint = Some(settings);
    }

    pub fn run(mut self) -> Result<()> {
        let event_loop = EventLoop::new().context("failed to create the native event loop")?;
        event_loop
            .run_app(&mut self)
            .context("native event loop failed")
    }

    /// The editor's buffer.
    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// The editor's buffer, for the callers that edit it.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    /// The file that is open, if any.
    pub fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }

    /// The agent's transcript so far.
    pub fn agent_transcript(&self) -> String {
        self.agent
            .as_ref()
            .map(|session| session.panel().transcript.clone())
            .unwrap_or_default()
    }

    /// Collects the agent's output. Returns `true` when something arrived.
    pub fn pump_agent(&mut self) -> bool {
        match self.agent.as_mut() {
            Some(session) => session.pump(),
            None => false,
        }
    }

    /// Handles the keys that drive the agent.
    ///
    /// Returns `true` when the key belonged to the agent, so the caller does
    /// not also feed it to the editor.
    fn handle_agent_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        let control = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        if !control && !alt {
            return false;
        }

        let Key::Character(character) = key else {
            return false;
        };
        let Some(letter) = character.as_ref().chars().next() else {
            return false;
        };
        let letter = letter.to_ascii_lowercase();

        // Ctrl+Alt+A allows the pending permission, Ctrl+Alt+N denies it.
        if control && alt {
            let option = match letter {
                'a' => Some("allow"),
                'n' => Some("reject"),
                _ => None,
            };
            if let (Some(option), Some(session)) = (option, self.agent.as_mut()) {
                if let Err(error) = session.answer_permission(option) {
                    eprintln!("could not answer the permission request: {error}");
                }
                return true;
            }
            return false;
        }

        if !control {
            return false;
        }

        match letter {
            // Ctrl+M starts a turn with the buffer's text as the prompt.
            'm' => {
                self.start_agent_turn();
                true
            }
            // Ctrl+S saves the open file.
            's' => {
                self.save_document();
                true
            }
            // Ctrl+. cancels the running turn.
            '.' => {
                if let Some(session) = self.agent.as_mut()
                    && let Err(error) = session.cancel()
                {
                    eprintln!("could not cancel the turn: {error}");
                }
                true
            }
            _ => false,
        }
    }

    /// The status line: the open file, whether it changed, and the last
    /// message.
    fn status_line(&self) -> String {
        let mut status = match self.document.as_ref() {
            Some(document) => {
                let dirty = if document.is_dirty(&self.editor.text()) {
                    " ●"
                } else {
                    ""
                };
                format!("{}{dirty}", document.display_path())
            }
            None => "untitled".to_owned(),
        };
        if let Some(message) = self.status.as_ref() {
            status.push_str(&format!("  ·  {message}"));
        }
        status
    }

    /// Writes the buffer back to the open file.
    fn save_document(&mut self) {
        let Some(mut document) = self.document.take() else {
            self.status = Some("没有打开的文件".to_owned());
            return;
        };

        let text = self.editor.text();
        match self.workspace.save(&mut document, &text) {
            Ok(()) => {
                self.status = Some(format!("已保存 {}", document.display_path()));
            }
            Err(error) => {
                self.status = Some(format!("保存失败：{error}"));
                eprintln!("could not save the document: {error}");
            }
        }
        self.document = Some(document);
    }

    /// Starts an agent turn with the editor's current text as the prompt.
    pub fn start_agent_turn(&mut self) {
        let prompt = self.editor.text();
        if prompt.trim().is_empty() {
            self.agent_error =
                Some("the buffer is empty, so there is nothing to ask about".to_owned());
            return;
        }

        if self.agent.is_none() {
            let Some(mut settings) = self.endpoint.clone().or_else(EndpointSettings::from_env)
            else {
                self.agent_error = Some(
                    "the agent endpoint is not configured; set CODEWEAVER_ACP_COMMAND and CODEWEAVER_ACP_ARGS"
                        .to_owned(),
                );
                return;
            };
            // The agent works in the directory of the open file, so its
            // context is the file the user is actually looking at.
            if let Some(document) = self.document.as_ref()
                && let Some(directory) = document.path().parent()
            {
                settings.cwd = directory.to_path_buf();
            }
            match AgentSession::start(&settings) {
                Ok(session) => {
                    self.agent = Some(session);
                    self.agent_error = None;
                }
                Err(error) => {
                    self.agent_error = Some(format!("the agent endpoint did not start: {error}"));
                    return;
                }
            }
        }

        if let Some(session) = self.agent.as_mut()
            && let Err(error) = session.start_prompt(&prompt)
        {
            eprintln!("could not start the turn: {error}");
        }
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
        // The window handle is cloned so that no borrow of `self` is alive
        // while the agent keys are handled.
        let Some(window) = self.window.clone() else {
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
                if self.handle_agent_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
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
                // The agent's output is collected once per frame, so a reply
                // appears while the agent is still producing it.
                if let Some(session) = self.agent.as_mut()
                    && session.pump()
                {
                    window.request_redraw();
                }
                // The status line is read before the renderer is borrowed.
                let status_line = self.status_line();
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                let scene = build_demo_scene(
                    renderer.logical_size(window.scale_factor()),
                    &self.editor,
                    &self.input,
                    self.agent.as_ref(),
                    self.agent_error.as_deref(),
                    status_line,
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
    agent: Option<&AgentSession>,
    agent_error: Option<&str>,
    status_line: String,
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
        editor_state.version(),
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
    // The status line sits under the heading.
    scene.push_text(
        status_line,
        Rect::new(56.0, 38.0, size.width - 72.0, 22.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.55, 0.60, 0.70),
            font_size: 13.0,
            line_height: 18.0,
        },
    );
    let mut text = String::with_capacity(editor_state.text().len() + input.preedit().len());
    text.push_str(&editor_state.text_before_caret());
    text.push_str(input.preedit());
    text.push_str(&editor_state.text_after_caret());
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

    push_agent_panel(&mut scene, size, agent, agent_error);
    scene
}

/// Draws the agent panel on the right of the window.
fn push_agent_panel(
    scene: &mut FrameScene,
    size: codeweaver_render::Size,
    agent: Option<&AgentSession>,
    agent_error: Option<&str>,
) {
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let panel_width = (size.width * 0.34).clamp(280.0, 520.0);
    let panel = Rect::new(size.width - panel_width, 0.0, panel_width, size.height);
    let content = Rect::new(
        panel.origin.x + 16.0,
        56.0,
        (panel.size.width - 32.0).max(1.0),
        (panel.size.height - 72.0).max(1.0),
    );

    scene.push_quad(panel, Color::rgb(0.055, 0.063, 0.082), viewport);
    scene.push_quad(
        Rect::new(panel.origin.x, 0.0, 1.0, panel.size.height),
        Color::rgb(0.16, 0.18, 0.22),
        viewport,
    );

    let heading = agent::session_summary(agent);
    let heading = if agent.is_some_and(|session| session.is_running()) {
        format!("{heading} · working")
    } else {
        heading
    };
    scene.push_text(
        heading,
        Rect::new(
            panel.origin.x + 16.0,
            16.0,
            (panel.size.width - 32.0).max(1.0),
            28.0,
        ),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.72, 0.78, 0.88),
            font_size: 14.0,
            line_height: 20.0,
        },
    );

    let body = match agent {
        Some(session) => {
            let panel = session.panel();
            let mut body = panel.transcript.clone();
            for tool in &panel.tools {
                body.push_str(&format!(
                    "\n[{}] {}",
                    match tool.status {
                        ToolStatus::InProgress => "running",
                        ToolStatus::Completed => "done",
                        ToolStatus::Failed => "failed",
                    },
                    tool.title
                ));
            }
            if let Some(pending) = &panel.pending_permission {
                body.push_str(&format!(
                    "\n需要授权：{}（Ctrl+Alt+A 允许 / Ctrl+Alt+N 拒绝）",
                    pending.title
                ));
            }
            if body.is_empty() {
                "Ctrl+M 把当前内容交给 Agent；Ctrl+. 取消本轮".to_owned()
            } else {
                body
            }
        }
        None => match agent_error {
            Some(error) => format!("Agent 不可用：{error}"),
            None => "Agent 未启动。Ctrl+M 把当前内容交给 Agent".to_owned(),
        },
    };

    scene.push_text(
        body,
        content,
        TextStyle {
            clip: content,
            color: Color::rgb(0.84, 0.87, 0.92),
            font_size: 14.0,
            line_height: 20.0,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch file inside the crate's own directory.
    fn scratch(name: &str) -> PathBuf {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
        fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory.join(name)
    }

    #[test]
    fn opens_a_file_into_the_buffer() {
        let path = scratch("open.txt");
        fs::write(&path, "line one\nline two\n").unwrap();

        let app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        assert_eq!(app.editor.text(), "line one\nline two\n");
        assert_eq!(app.editor.caret().0, 0);
    }

    #[test]
    fn saving_writes_the_buffer_back_to_the_file() {
        let path = scratch("save.txt");
        fs::write(&path, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        app.editor.insert("edited ").unwrap();
        assert!(
            app.status_line().contains('●'),
            "the file must show as changed"
        );

        app.save_document();

        assert_eq!(fs::read_to_string(&path).unwrap(), "edited original\n");
        assert!(
            !app.status_line().contains('●'),
            "a saved file must not show as changed"
        );
        assert!(app.status_line().contains("已保存"));
    }

    #[test]
    fn saving_refuses_to_overwrite_a_file_that_changed_on_disk() {
        let path = scratch("changed.txt");
        fs::write(&path, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        app.editor.insert("mine ").unwrap();

        // Something else writes the file after the editor opened it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, "somebody else wrote this\n").unwrap();

        app.save_document();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "somebody else wrote this\n",
            "the outside change must survive"
        );
        assert!(app.status_line().contains("保存失败"));
    }

    #[test]
    fn the_status_line_names_the_open_file() {
        let path = scratch("status.txt");
        fs::write(&path, "x\n").unwrap();
        let app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        assert!(app.status_line().contains(&path.display().to_string()));
    }

    #[test]
    fn an_application_without_a_file_starts_on_an_empty_document() {
        let app = NativeApplication::new(None).expect("the application starts");
        assert_eq!(app.editor.text(), "");
        assert!(app.status_line().contains("untitled"));
    }
}
