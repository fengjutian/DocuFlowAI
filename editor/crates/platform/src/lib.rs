mod agent;
mod completion;
pub mod editor_view;
pub mod find;
mod input;
mod lsp;

use std::{ops::Range, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use codeweaver_agent_acp::ToolStatus;
use codeweaver_editor_core::Editor;
use codeweaver_lsp::{Client as LspClient, LspEvent, ServerConfig};
use codeweaver_render::{
    Color, FrameScene, Point, Rect, Renderer, RendererError, Size, TextFamily, TextStyle,
};
use codeweaver_syntax::{Language, Syntax};
use codeweaver_text::TextOffset;
use codeweaver_ui::TextMeasurer;
use codeweaver_workspace::{Document, Workspace};
use completion::CompletionPopup;
use editor_view::{
    BodyOverlay, EditorView, FindOverlay, LINE_HEIGHT, MarkSeverity, RendererMeasurer,
};
use find::{FindBar, FindField};
use input::TextInputState;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
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
    /// The editor body: the scroll position and the layout of the text.
    view: EditorView,
    /// Where the mouse is, in logical pixels.
    cursor: Option<Point>,
    /// The find bar, when it is open.
    find: Option<FindBar>,
    /// The syntax tree of the open document, and the highlights it gives.
    syntax: Syntax,
    /// The language server of the open document, if there is one.
    server: Option<LspClient>,
    /// The URI of the open document, as the server knows it.
    document_uri: Option<String>,
    /// What the server has been told about the document.
    server_version: Option<u64>,
    /// The version the server is told next.
    server_generation: i64,
    /// The things the language server found wrong in the document.
    marks: Vec<editor_view::Mark>,
    /// How many of them are errors, and how many are warnings.
    errors: usize,
    warnings: usize,
    /// The completions the server offered, when the list is open.
    completion: Option<CompletionPopup>,
}

impl NativeApplication {
    /// Creates the application, opening `path` when one is given.
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        Self::build(path, None)
    }

    /// Creates the application with a language server of the caller's
    /// choosing.
    ///
    /// Without this, the server of the language of the file is used: the
    /// editor knows `rust-analyzer` and `gopls`, but nothing else.
    pub fn with_language_server(path: Option<PathBuf>, server: ServerConfig) -> Result<Self> {
        Self::build(path, Some(server))
    }

    /// Creates the application, with the language server to use when it is
    /// given.
    fn build(path: Option<PathBuf>, server_config: Option<ServerConfig>) -> Result<Self> {
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

        // The language of the file decides how the text is highlighted.
        let mut syntax = Syntax::new();
        syntax.set_language(
            document
                .as_ref()
                .and_then(|document| Language::from_path(document.path())),
        );

        // The language of the file also decides which language server runs.
        let mut server = None;
        let mut document_uri = None;
        if let Some(document) = document.as_ref()
            && let Some(language) = Language::from_path(document.path())
            && let Some(config) = server_config.clone().or_else(|| lsp::server_for(language))
        {
            let root = lsp::workspace_root(document.path());
            match LspClient::start(&config, &root) {
                Ok(client) => {
                    let uri = codeweaver_lsp::protocol::file_uri(document.path());
                    let _ = client.did_open(&uri, lsp::language_id(language), 1, &editor.text());
                    document_uri = Some(uri);
                    server = Some(client);
                }
                Err(error) => eprintln!("could not start the language server: {error}"),
            }
        }

        // The server has been told about the text that is there now.
        let server_version = server.as_ref().map(|_| editor.version());

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
            view: EditorView::new(),
            cursor: None,
            find: None,
            syntax,
            server,
            document_uri,
            server_version,
            server_generation: 1,
            marks: Vec::new(),
            errors: 0,
            warnings: 0,
            completion: None,
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

    /// The size of the window, in logical pixels.
    fn window_size(&self, scale_factor: f64) -> Size {
        match self.renderer.as_ref() {
            Some(renderer) => renderer.logical_size(scale_factor),
            None => Size::new(0.0, 0.0),
        }
    }

    /// How many lines of the body fit on screen.
    fn visible_lines(&self, scale_factor: f64) -> usize {
        let body = editor_body_rect(self.window_size(scale_factor));
        ((body.size.height / LINE_HEIGHT).floor() as usize).max(1)
    }

    /// Rebuilds the layout of the text on screen.
    ///
    /// The layout is only rebuilt when something it depends on changed, so
    /// a mouse move does not pay for measuring the text again.
    fn rebuild_layout(&mut self, scale_factor: f64) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let size = renderer.logical_size(scale_factor);
        let body = editor_body_rect(size);
        let mut measurer = RendererMeasurer::new(renderer, TextFamily::Monospace);
        self.view.layout(
            self.editor.buffer(),
            body.size.width,
            body.size.height,
            &mut measurer,
        );
    }

    /// Collects what the language server has said.
    ///
    /// The server talks on its own thread, so its answers are collected once
    /// per frame - the same way the agent's output is. Returns whether
    /// something the user can see changed.
    fn pump_lsp(&mut self) -> bool {
        let events = match self.server.as_ref() {
            Some(server) => server.drain_events(),
            None => return false,
        };
        let lists = match self.server.as_ref() {
            Some(server) => server.drain_completions(),
            None => Vec::new(),
        };
        let mut changed = false;
        for event in events {
            match event {
                LspEvent::Diagnostics { uri, diagnostics } => {
                    // The server may talk about other documents; only the one
                    // on screen is drawn.
                    if Some(uri.as_str()) != self.document_uri.as_deref() {
                        continue;
                    }
                    let marks = lsp::marks_for(self.editor.buffer(), &diagnostics);
                    self.errors = marks
                        .iter()
                        .filter(|mark| mark.severity == MarkSeverity::Error)
                        .count();
                    self.warnings = marks
                        .iter()
                        .filter(|mark| mark.severity == MarkSeverity::Warning)
                        .count();
                    self.marks = marks;
                    changed = true;
                }
                LspEvent::Disconnected { reason } => {
                    eprintln!("the language server went away: {reason}");
                    self.status = Some(format!("语言服务器断开了：{reason}"));
                    self.server = None;
                    self.marks.clear();
                    changed = true;
                }
                LspEvent::Message { text } => {
                    self.status = Some(text);
                    changed = true;
                }
            }
        }

        // The completions the server offered, with the ones that do not
        // match the word in front of the caret left out.
        for list in lists {
            let caret = self.editor.caret().0;
            let prefix = completion::word_before(self.editor.buffer(), caret);
            let items = completion::matching(list.items, &prefix);
            let start = completion::word_start(self.editor.buffer(), caret);
            if let Some(popup) = CompletionPopup::new(items, start) {
                self.completion = Some(popup);
                changed = true;
            }
        }
        changed
    }

    /// Tells the language server what the text looks like now.
    ///
    /// The server is told about every edit, so its answers are about the text
    /// that is on screen and not about the text as the file was opened.
    fn tell_server_about_the_change(&mut self) {
        let Some(uri) = self.document_uri.clone() else {
            return;
        };
        let Some(change) = self.editor.last_change() else {
            return;
        };
        let version = self.editor.buffer().version();
        if self.server_version == Some(version) {
            // The server already knows about this text.
            return;
        }
        let changes = lsp::changes_for(self.editor.buffer(), change);
        let text = self.editor.text();
        self.server_version = Some(version);
        self.server_generation += 1;
        let generation = self.server_generation;
        if let Some(server) = self.server.as_ref()
            && let Err(error) = server.did_change(&uri, generation, &text, &changes)
        {
            eprintln!("could not tell the language server about the change: {error}");
        }
    }

    /// Scrolls the body so that the caret is on screen.
    fn keep_caret_visible(&mut self, scale_factor: f64) {
        let total_lines = self.editor.buffer().line_count();
        let visible = self.visible_lines(scale_factor);
        let caret = self.editor.caret().0;
        self.view
            .ensure_caret_visible(self.editor.buffer(), caret, total_lines, visible);
    }

    /// Handles a mouse press in the editor body.
    ///
    /// Ctrl+click adds a caret instead of moving the one that is there.
    fn press_body(&mut self, scale_factor: f64) {
        let Some(cursor) = self.cursor else {
            return;
        };
        let body = editor_body_rect(self.window_size(scale_factor));
        if !body.contains(cursor) {
            return;
        }
        let add = self.modifiers.control_key();
        self.view.press(
            &mut self.editor,
            cursor.x - body.origin.x,
            cursor.y - body.origin.y,
            add,
        );
        self.keep_caret_visible(scale_factor);
    }

    /// Handles the mouse moving while a drag is in progress.
    fn drag_body(&mut self, scale_factor: f64) {
        if !self.view.is_dragging() {
            return;
        }
        let Some(cursor) = self.cursor else {
            return;
        };
        let body = editor_body_rect(self.window_size(scale_factor));
        self.view.drag(
            &mut self.editor,
            cursor.x - body.origin.x,
            cursor.y - body.origin.y,
        );
    }

    /// Moves the caret one visual line up or down.
    ///
    /// When the caret is on the edge of the screen, the body scrolls first,
    /// so the caret can keep moving.
    fn move_caret_vertically(&mut self, scale_factor: f64, delta: isize) {
        if self.view.move_vertical(&mut self.editor, delta) {
            return;
        }
        let total_lines = self.editor.buffer().line_count();
        self.view.scroll(delta, total_lines);
        self.rebuild_layout(scale_factor);
        self.view.move_vertical(&mut self.editor, delta);
    }

    /// Scrolls the body by a number of lines.
    fn scroll_body(&mut self, lines: isize) {
        let total_lines = self.editor.buffer().line_count();
        self.view.scroll(lines, total_lines);
    }

    /// Scrolls the body the way the wheel was turned.
    ///
    /// A positive delta means the wheel was turned away from the user, which
    /// shows the text above.
    fn scroll_body_by_wheel(&mut self, delta: f32) {
        self.scroll_body(-(delta as isize));
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

    /// Handles the keys that the language server takes.
    ///
    /// The list of completions takes the keys that move inside it, and
    /// Ctrl+Space asks the server what could come next.
    fn handle_lsp_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        if self.completion.is_some() {
            let mut completion = self.completion.take().expect("the list is open");
            match key {
                Key::Named(NamedKey::Escape) => {
                    // The list closes and the editor takes the keyboard back.
                    return true;
                }
                Key::Named(NamedKey::ArrowDown) => {
                    completion.select_next();
                    self.completion = Some(completion);
                    return true;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    completion.select_previous();
                    self.completion = Some(completion);
                    return true;
                }
                Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) => {
                    if let Err(error) = completion.apply(&mut self.editor) {
                        eprintln!("could not put the completion in: {error}");
                    }
                    return true;
                }
                _ => {
                    // The key was not one of the list: the list closes and
                    // the key goes on to the editor.
                }
            }
        }

        // Ctrl+Space asks the server what could come next.
        if self.modifiers.control_key() && is_space(key) {
            self.request_completion();
            return true;
        }
        false
    }

    /// Asks the server what could come next at the caret.
    fn request_completion(&mut self) {
        let Some(uri) = self.document_uri.clone() else {
            return;
        };
        let caret = self.editor.caret().0;
        let position = lsp::position_of(self.editor.buffer(), caret);
        let outcome = self
            .server
            .as_ref()
            .map(|server| server.request_completion(&uri, position));
        match outcome {
            Some(Ok(())) => {}
            Some(Err(error)) => {
                eprintln!("could not ask the server for completions: {error}");
                self.status = Some(format!("补全请求失败：{error}"));
            }
            None => self.status = Some("没有语言服务器".to_owned()),
        }
    }

    /// Opens the find bar when the user asks for it.
    ///
    /// Ctrl+F finds, Ctrl+H finds and replaces.
    fn handle_find_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        if !self.modifiers.control_key() {
            return false;
        }
        let Key::Character(character) = key else {
            return false;
        };
        let letter = character
            .as_ref()
            .chars()
            .next()
            .map(|letter| letter.to_ascii_lowercase());
        match letter {
            Some('f') => {
                self.open_find(false);
                true
            }
            Some('h') => {
                self.open_find(true);
                true
            }
            _ => false,
        }
    }

    /// Gives a key to the find bar while it is open.
    ///
    /// Returns `true` when the key belonged to the find bar. The bar is taken
    /// out of the application while it is handled, so that it can be closed.
    fn handle_find_bar_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        let Some(mut find) = self.find.take() else {
            return false;
        };

        let mut handled = true;
        // Whether the bar stays open.
        let mut keep = true;
        match key {
            Key::Named(NamedKey::Escape) => {
                // The bar closes and the editor takes the keyboard back.
                keep = false;
            }
            Key::Named(NamedKey::Enter) => {
                if self.modifiers.control_key() && self.modifiers.alt_key() {
                    let count = find.replace_all(&mut self.editor);
                    self.status = Some(format!("替换了 {count} 处"));
                } else if self.modifiers.alt_key() {
                    let count = find.select_all_matches(&mut self.editor);
                    self.status = Some(format!("选中了 {count} 处"));
                } else if self.modifiers.shift_key() {
                    find.previous_match();
                } else {
                    find.next_match();
                }
            }
            Key::Named(NamedKey::Tab) => find.toggle_field(),
            Key::Named(NamedKey::Backspace) => find.backspace(&self.editor),
            Key::Named(NamedKey::Space) => find.type_text(&self.editor, " "),
            Key::Character(character) if !self.modifiers.control_key() => {
                let character = character.as_ref();
                if character.chars().any(char::is_control) {
                    // The key did not produce text of its own; the text of
                    // the event is what the user typed.
                    if let Some(typed) = text.filter(|typed| !typed.chars().any(char::is_control)) {
                        find.type_text(&self.editor, typed);
                    } else {
                        handled = false;
                    }
                } else {
                    find.type_text(&self.editor, character);
                }
            }
            _ => {
                handled = false;
            }
        }
        if keep {
            self.find = Some(find);
        }
        handled
    }

    /// Opens the find bar and searches for what the editor has selected.
    fn open_find(&mut self, replacing: bool) {
        let mut find = self.find.take().unwrap_or_default();
        find.open(&mut self.editor, replacing);
        self.find = Some(find);
    }

    /// Opens the find bar and searches for the given text.
    ///
    /// This is what the editor does when it is started with a search, so the
    /// find bar can be looked at without a keyboard.
    pub fn open_find_with(&mut self, query: &str) {
        let mut find = self.find.take().unwrap_or_default();
        find.open(&mut self.editor, false);
        find.type_text(&self.editor, query);
        self.find = Some(find);
    }

    /// Asks the language server what could come next at the caret.
    ///
    /// This is what the editor does when it is started with `--complete`, so
    /// that the list of completions can be looked at without a keyboard.
    pub fn open_completion(&mut self) {
        self.request_completion();
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
        // What the language server found wrong is counted, so that the user
        // knows there is something to look at.
        if self.errors > 0 || self.warnings > 0 {
            status.push_str(&format!("  ·  ✕ {}  ⚠ {}", self.errors, self.warnings));
        }
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

    /// Called when the event loop is about to wait.
    ///
    /// The language server answers on its own thread, so the window has to
    /// wake up from time to time to hear what it said. Without this, the
    /// diagnostics of a file that was opened would only show up when the
    /// user typed something.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.pump_lsp()
            && let Some(window) = self.window.as_ref()
        {
            window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + Duration::from_millis(100),
        ));
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
                        // While the find bar is open, the composed text goes
                        // into the bar instead of the document.
                        if let Some(find) = self.find.as_mut() {
                            find.type_text(&self.editor, &text);
                        } else if let Err(error) = self.editor.insert(&text) {
                            eprintln!("failed to commit IME text: {error}");
                        }
                        self.input.finish_commit();
                    }
                }
                window.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let scale_factor = window.scale_factor();
                self.cursor = Some(Point {
                    x: position.x as f32 / scale_factor as f32,
                    y: position.y as f32 / scale_factor as f32,
                });
                self.drag_body(scale_factor);
                window.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    let scale_factor = window.scale_factor();
                    match state {
                        ElementState::Pressed => self.press_body(scale_factor),
                        ElementState::Released => self.view.release(),
                    }
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(lines, _) => lines as isize,
                    MouseScrollDelta::PixelDelta(pixels) => {
                        (pixels.x / LINE_HEIGHT as f64) as isize
                    }
                };
                if lines != 0 {
                    self.scroll_body_by_wheel(lines as f32);
                    window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.handle_agent_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                if self.handle_find_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // While the find bar is open, the keys go to the bar.
                if self.handle_find_bar_key(&event.logical_key.as_ref(), event.text.as_deref()) {
                    window.request_redraw();
                    return;
                }
                // The list of completions and the request for one belong to
                // the language server.
                if self.handle_lsp_key(&event.logical_key.as_ref()) {
                    self.tell_server_about_the_change();
                    window.request_redraw();
                    return;
                }
                let scale_factor = window.scale_factor();
                // The layout is measured from the text on screen, so it has
                // to be current before the caret is moved.
                self.rebuild_layout(scale_factor);
                let result = match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Backspace) => self.editor.backspace(),
                    Key::Named(NamedKey::Delete) => self.editor.delete_forward(),
                    Key::Named(NamedKey::ArrowLeft) if self.modifiers.shift_key() => {
                        self.editor.extend_left();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowRight) if self.modifiers.shift_key() => {
                        self.editor.extend_right();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowUp) if self.modifiers.shift_key() => {
                        self.view.extend_vertical(&mut self.editor, -1);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowDown) if self.modifiers.shift_key() => {
                        self.view.extend_vertical(&mut self.editor, 1);
                        Ok(())
                    }
                    Key::Named(NamedKey::Home) if self.modifiers.shift_key() => {
                        self.view.extend_to_line_edge(&mut self.editor, false);
                        Ok(())
                    }
                    Key::Named(NamedKey::End) if self.modifiers.shift_key() => {
                        self.view.extend_to_line_edge(&mut self.editor, true);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        self.editor.move_left();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        self.editor.move_right();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowUp) if self.modifiers.alt_key() => {
                        self.view.add_cursor_vertically(&mut self.editor, -1);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowDown) if self.modifiers.alt_key() => {
                        self.view.add_cursor_vertically(&mut self.editor, 1);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.move_caret_vertically(scale_factor, -1);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        self.move_caret_vertically(scale_factor, 1);
                        Ok(())
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.editor.collapse_to_primary();
                        Ok(())
                    }
                    Key::Named(NamedKey::Home) => {
                        self.view.move_to_line_edge(&mut self.editor, false);
                        Ok(())
                    }
                    Key::Named(NamedKey::End) => {
                        self.view.move_to_line_edge(&mut self.editor, true);
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
                // The text may have changed, so the layout is measured again
                // and the caret is brought back on screen.
                self.rebuild_layout(scale_factor);
                self.keep_caret_visible(scale_factor);
                self.tell_server_about_the_change();
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
                // The language server answers on its own thread, so its
                // answers are collected the same way.
                if self.pump_lsp() {
                    window.request_redraw();
                }
                // The language server answers on its own thread, so its
                // findings are collected here too.
                if self.pump_lsp() {
                    window.request_redraw();
                }
                // The status line is read before the renderer is borrowed.
                let status_line = self.status_line();
                let scale_factor = window.scale_factor();
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                let scene = build_scene(
                    renderer,
                    renderer.logical_size(scale_factor),
                    &self.editor,
                    &self.input,
                    &mut self.view,
                    &mut self.syntax,
                    Chrome {
                        agent: self.agent.as_ref(),
                        agent_error: self.agent_error.as_deref(),
                        status_line,
                        find: self.find.as_ref(),
                        marks: &self.marks,
                        completion: self.completion.as_ref(),
                    },
                );
                match renderer.render(&scene, scale_factor) {
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

/// The parts of the frame that are not the text body.
struct Chrome<'a> {
    agent: Option<&'a AgentSession>,
    agent_error: Option<&'a str>,
    status_line: String,
    /// The find bar, when it is open.
    find: Option<&'a FindBar>,
    /// The things the language server found wrong in the document.
    marks: &'a [editor_view::Mark],
    /// The list of completions, when it is open.
    completion: Option<&'a CompletionPopup>,
}

/// The matches of a search that was not run.
const NO_MATCHES: &[Range<TextOffset>] = &[];

/// Whether a key is the space bar.
///
/// The window system may say that the space is a named key or that it is a
/// character, and both are the space.
fn is_space<T: AsRef<str>>(key: &Key<T>) -> bool {
    match key {
        Key::Named(NamedKey::Space) => true,
        Key::Character(character) => character.as_ref() == " ",
        _ => false,
    }
}

fn build_scene(
    renderer: &mut Renderer,
    size: Size,
    editor_state: &Editor,
    input: &TextInputState,
    view: &mut EditorView,
    syntax: &mut Syntax,
    chrome: Chrome<'_>,
) -> FrameScene {
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let body = editor_body_rect(size);
    let mut scene = FrameScene::default();

    // The matches the find bar found, and the one it is on.
    let (matches, current_match) = match chrome.find {
        Some(find) => (find.matches(), Some(find.current_index())),
        None => (NO_MATCHES, None),
    };
    let overlay = FindOverlay {
        // The find bar floats over the body, so the text under it is left
        // out.
        bar: chrome
            .find
            .map(|find| find_bar_rect(body, find.replacing())),
        matches,
        current: current_match,
    };

    {
        // Only the renderer knows how the text is shaped, so the layout is
        // measured through it.
        let mut measurer = RendererMeasurer::new(renderer, TextFamily::Monospace);
        view.layout(
            editor_state.buffer(),
            body.size.width,
            body.size.height,
            &mut measurer,
        );

        // The tree of the text is parsed again when the text changed, and
        // the highlights are made for the text that is on screen.
        let change = editor_state.last_change();
        syntax.update(editor_state.buffer(), change);
        let visible = view.visible_range();
        let highlights = syntax.highlights(editor_state.buffer(), visible);

        scene.push_quad(
            Rect::new(0.0, 0.0, 40.0, size.height),
            Color::rgb(0.075, 0.086, 0.11),
            viewport,
        );
        scene.push_quad(body, Color::rgb(0.075, 0.086, 0.11), viewport);
        scene.push_quad(
            Rect::new(body.origin.x, body.origin.y, 3.0, body.size.height),
            Color::rgb(0.30, 0.58, 0.96),
            viewport,
        );
        let heading = format!(
            "CodeWeaver · Buffer v{} · IME {} · Undo {} · Redo {} · 光标 {}",
            editor_state.version(),
            if input.ime_enabled() { "ON" } else { "READY" },
            editor_state.can_undo(),
            editor_state.can_redo(),
            editor_state.cursor_count(),
        );
        scene.push_text(
            heading,
            Rect::new(56.0, 16.0, size.width - 72.0, 28.0),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.78, 0.82, 0.90),
                font_size: 15.0,
                line_height: 22.0,
                family: TextFamily::SansSerif,
            },
        );
        // The status line sits under the heading.
        scene.push_text(
            chrome.status_line,
            Rect::new(56.0, 38.0, size.width - 72.0, 22.0),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.55, 0.60, 0.70),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );

        view.draw(
            &mut scene,
            editor_state,
            input.preedit(),
            body,
            BodyOverlay {
                find: overlay,
                highlights,
                marks: chrome.marks,
            },
            &mut measurer,
        );
    }

    push_agent_panel(&mut scene, size, chrome.agent, chrome.agent_error);

    // The find bar is drawn in the sans-serif face, so it gets a measurer of
    // its own.
    let mut ui_measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);
    push_find_bar(&mut scene, size, body, chrome.find, &mut ui_measurer);

    // The list of completions floats under the caret.
    if let Some(popup) = chrome.completion
        && let Some(point) = view.point_of(editor_state.caret().0)
    {
        completion::draw(
            &mut scene,
            Rect::new(0.0, 0.0, size.width, size.height),
            body,
            popup,
            point,
            &mut ui_measurer,
        );
    }
    scene
}

/// The area the agent panel is drawn in.
fn agent_panel_rect(size: Size) -> Rect {
    let width = (size.width * 0.34).clamp(280.0, 520.0);
    Rect::new(size.width - width, 0.0, width, size.height)
}

/// The area the text body is drawn in.
fn editor_body_rect(size: Size) -> Rect {
    let panel = agent_panel_rect(size);
    let left = 56.0;
    let right = panel.origin.x - 16.0;
    Rect::new(
        left,
        52.0,
        (right - left).max(1.0),
        (size.height - 84.0).max(1.0),
    )
}

/// The place of the find bar in the corner of the editor body.
///
/// The bar is flush with the right edge of the body, so that the text it
/// covers is exactly the text under it.
fn find_bar_rect(body: Rect, replacing: bool) -> Rect {
    let width = 420.0f32.min(body.size.width.max(1.0));
    let height = if replacing { 86.0 } else { 60.0 };
    Rect::new(
        body.origin.x + body.size.width - width,
        body.origin.y + 8.0,
        width,
        height,
    )
}

/// Draws the find bar in the corner of the editor body.
fn push_find_bar(
    scene: &mut FrameScene,
    size: Size,
    body: Rect,
    find: Option<&FindBar>,
    measurer: &mut dyn TextMeasurer,
) {
    let Some(find) = find else {
        return;
    };
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let bar = find_bar_rect(body, find.replacing());
    let width = bar.size.width;
    let height = bar.size.height;

    scene.push_quad(bar, Color::rgb(0.10, 0.12, 0.16), viewport);
    scene.push_quad(
        Rect::new(bar.origin.x, bar.origin.y, bar.size.width, 1.0),
        Color::rgb(0.22, 0.25, 0.31),
        viewport,
    );

    let field_width = (bar.size.width - 104.0).max(1.0);
    let query_field = Rect::new(bar.origin.x + 12.0, bar.origin.y + 10.0, field_width, 24.0);
    push_find_field(
        scene,
        viewport,
        query_field,
        find.query(),
        find.field() == FindField::Query,
        "查找",
        measurer,
    );
    if find.replacing() {
        let replacement_field =
            Rect::new(bar.origin.x + 12.0, bar.origin.y + 40.0, field_width, 24.0);
        push_find_field(
            scene,
            viewport,
            replacement_field,
            find.replacement(),
            find.field() == FindField::Replacement,
            "替换为",
            measurer,
        );
    }

    // How many matches were found.
    let status = Rect::new(
        bar.origin.x + 12.0 + field_width + 8.0,
        bar.origin.y + 12.0,
        (width - field_width - 32.0).max(1.0),
        20.0,
    );
    scene.push_text(
        find.status(),
        status,
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.72, 0.76, 0.84),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );
    let hints = Rect::new(
        bar.origin.x + 12.0,
        bar.origin.y + height - 22.0,
        (width - 24.0).max(1.0),
        18.0,
    );
    scene.push_text(
        "Enter 下一个 · Shift+Enter 上一个 · Tab 切换 · Alt+Enter 全选匹配",
        hints,
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.45, 0.50, 0.58),
            font_size: 12.0,
            line_height: 16.0,
            family: TextFamily::SansSerif,
        },
    );
}

/// Draws one field of the find bar.
fn push_find_field(
    scene: &mut FrameScene,
    viewport: Rect,
    field: Rect,
    text: &str,
    active: bool,
    placeholder: &str,
    measurer: &mut dyn TextMeasurer,
) {
    scene.push_quad(field, Color::rgb(0.06, 0.07, 0.09), viewport);
    scene.push_quad(
        Rect::new(field.origin.x, field.origin.y, field.size.width, 1.0),
        if active {
            Color::rgb(0.30, 0.58, 0.96)
        } else {
            Color::rgb(0.18, 0.20, 0.25)
        },
        viewport,
    );
    let empty = text.is_empty();
    let shown = if empty { placeholder } else { text };
    let width = measurer.measure(shown, 14.0).width;
    scene.push_text(
        shown.to_owned(),
        Rect::new(
            field.origin.x + 6.0,
            field.origin.y + 2.0,
            (field.size.width - 12.0).max(1.0),
            (field.size.height - 4.0).max(1.0),
        ),
        TextStyle {
            clip: field,
            color: if empty {
                Color::rgb(0.38, 0.42, 0.50)
            } else {
                Color::rgb(0.86, 0.89, 0.94)
            },
            font_size: 14.0,
            line_height: 20.0,
            family: TextFamily::SansSerif,
        },
    );
    if active {
        // The caret stands after the text that was typed.
        scene.push_quad(
            Rect::new(
                field.origin.x + 6.0 + width + 1.0,
                field.origin.y + 4.0,
                1.5,
                (field.size.height - 8.0).max(1.0),
            ),
            Color::rgb(0.30, 0.58, 0.96),
            field,
        );
    }
}

/// Draws the agent panel on the right of the window.
fn push_agent_panel(
    scene: &mut FrameScene,
    size: Size,
    agent: Option<&AgentSession>,
    agent_error: Option<&str>,
) {
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let panel = agent_panel_rect(size);
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
            family: TextFamily::SansSerif,
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
            family: TextFamily::SansSerif,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use codeweaver_text::TextOffset;
    use codeweaver_ui::MonospaceMeasurer;

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

    /// An application with a laid out body, for the checks that need one.
    fn app_with_body(text: &str) -> NativeApplication {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.editor.load_text(text);
        let mut measurer = MonospaceMeasurer::new(10.0);
        app.view
            .layout(app.editor.buffer(), 1000.0, 200.0, &mut measurer);
        app
    }

    #[test]
    fn the_wheel_turned_towards_the_user_scrolls_down() {
        let mut app = app_with_body("one\ntwo\nthree\nfour\n");
        // The wheel reports a negative delta when it is turned towards the
        // user.
        app.scroll_body_by_wheel(-3.0);
        assert_eq!(app.view.view.first_line, 3);
        app.scroll_body_by_wheel(2.0);
        assert_eq!(app.view.view.first_line, 1);
    }

    #[test]
    fn the_body_never_scrolls_past_the_end_of_the_document() {
        let mut app = app_with_body("one\ntwo\n");
        app.scroll_body_by_wheel(-100.0);
        // The document has three lines, the last one empty.
        assert_eq!(app.view.view.first_line, 2);
    }

    #[test]
    fn the_caret_moves_a_visual_line_at_a_time() {
        let mut app = app_with_body("first\nsecond\n");
        app.editor.select_range(TextOffset(3), TextOffset(3));
        app.move_caret_vertically(1.0, 1);
        // The caret keeps its column: three characters into the next line.
        assert_eq!(app.editor.caret().0, 9);
        app.move_caret_vertically(1.0, -1);
        assert_eq!(app.editor.caret().0, 3);
    }

    #[test]
    fn the_caret_stays_on_screen_when_it_leaves_the_view() {
        let mut app = app_with_body(&"line\n".repeat(20));
        app.editor.select_range(TextOffset(99), TextOffset(99));
        app.keep_caret_visible(1.0);
        assert!(
            app.view.view.first_line > 0,
            "the view must follow the caret"
        );
    }

    #[test]
    fn adding_a_caret_below_gives_every_caret_a_neighbour() {
        let mut app = app_with_body("one\ntwo\nthree\n");
        app.editor.select_range(TextOffset(1), TextOffset(1));
        assert!(app.view.add_cursor_vertically(&mut app.editor, 1));
        assert_eq!(app.editor.cursor_count(), 2);
        // The new caret keeps the column: the second character of the line
        // below.
        assert_eq!(app.editor.carets(), vec![TextOffset(1), TextOffset(5)]);
    }

    #[test]
    fn adding_a_caret_at_the_edge_of_the_document_does_nothing() {
        let mut app = app_with_body("one\ntwo\n");
        // The last line of the document has no line below it.
        app.editor.select_range(TextOffset(8), TextOffset(8));
        assert!(!app.view.add_cursor_vertically(&mut app.editor, 1));
        assert_eq!(app.editor.cursor_count(), 1);
    }

    #[test]
    fn typing_writes_at_every_caret() {
        let mut app = app_with_body("one\ntwo\n");
        app.editor.select_range(TextOffset(1), TextOffset(1));
        app.view.add_cursor_vertically(&mut app.editor, 1);
        app.editor.insert("X").unwrap();
        assert_eq!(app.editor.text(), "oXne\ntXwo\n");
    }

    #[test]
    fn holding_shift_extends_the_selection_to_the_line_below() {
        let mut app = app_with_body("one\ntwo\n");
        app.editor.select_range(TextOffset(1), TextOffset(1));
        assert!(app.view.extend_vertical(&mut app.editor, 1));
        assert_eq!(
            app.editor.selection().range(app.editor.buffer()),
            TextOffset(1)..TextOffset(5)
        );
    }

    /// An application with the find bar open.
    fn app_with_find(text: &str) -> NativeApplication {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.editor.load_text(text);
        app.modifiers = ModifiersState::CONTROL;
        app.handle_find_key(&character("f"));
        app.modifiers = ModifiersState::default();
        app
    }

    /// A key that carries a character.
    fn character(text: &str) -> Key<&'_ str> {
        Key::Character(text)
    }

    /// A key that has a name, like Enter or Escape.
    fn named(key: NamedKey) -> Key<&'static str> {
        Key::Named(key)
    }

    #[test]
    fn the_find_key_opens_the_find_bar() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_find_key(&character("f")));
        assert!(app.find.is_some());
        // Ctrl+H replaces as well as finds.
        assert!(app.handle_find_key(&character("h")));
        assert!(app.find.as_ref().expect("the find bar is open").replacing());
    }

    #[test]
    fn the_find_bar_takes_the_keys_while_it_is_open() {
        let mut app = app_with_find("one two one");
        app.handle_find_bar_key(&character("o"), Some("o"));
        // The text goes into the query, not into the document.
        assert_eq!(app.editor.text(), "one two one");
        let find = app.find.as_ref().expect("the find bar is open");
        assert_eq!(find.query(), "o");
        assert_eq!(find.match_count(), 3);
    }

    #[test]
    fn escape_closes_the_find_bar() {
        let mut app = app_with_find("one two one");
        assert!(app.handle_find_bar_key(&named(NamedKey::Escape), None));
        assert!(app.find.is_none());
    }

    #[test]
    fn selecting_every_match_gives_every_match_a_caret() {
        let mut app = app_with_find("one two one");
        app.handle_find_bar_key(&character("o"), Some("o"));
        app.modifiers = ModifiersState::ALT;
        app.handle_find_bar_key(&named(NamedKey::Enter), None);
        assert_eq!(app.editor.cursor_count(), 3);
    }

    #[test]
    fn the_next_match_walks_through_the_matches() {
        let mut app = app_with_find("one two one");
        app.handle_find_bar_key(&character("o"), Some("o"));
        let find = app.find.as_ref().expect("the find bar is open");
        assert_eq!(find.current_index(), 0);
        app.handle_find_bar_key(&named(NamedKey::Enter), None);
        let find = app.find.as_ref().expect("the find bar is open");
        assert_eq!(find.current_index(), 1);
    }
}
