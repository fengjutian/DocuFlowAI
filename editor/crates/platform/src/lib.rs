mod agent;
mod completion;
mod context;
mod diff_review;
pub mod editor_view;
pub mod find;
mod input;
mod lsp;
mod pane;
mod paths;
mod settings;
mod shell;

use std::{ops::Range, path::{Path, PathBuf}, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use codeweaver_agent_acp::ToolStatus;
use codeweaver_agent_ui::{ContextItem, HunkDecision};
use codeweaver_editor_core::Editor;
use codeweaver_lsp::{Client as LspClient, LspEvent, ServerConfig, protocol::TextEdit};
use codeweaver_render::{
    Color, FrameScene, Point, Rect, Renderer, RendererError, Size, TextFamily, TextStyle,
};
use codeweaver_syntax::{Highlight, Language};
use codeweaver_text::TextOffset;
use codeweaver_ui::TextMeasurer;
use codeweaver_workspace::{ChangeDetector, Document, ExternalChange, FileTree, Workspace};
use codeweaver_workbench::{close_decision, CloseDecision, Tab, TabBar, TabId};
use completion::CompletionPopup;
use editor_view::{
    BodyOverlay, EditorView, FindOverlay, LINE_HEIGHT, MarkSeverity, RendererMeasurer,
};
use find::{FindBar, FindField};
use input::TextInputState;
use pane::EditorPane;
use settings::Settings;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, ModifiersState, NamedKey, NativeKey},
    window::{ImePurpose, Window, WindowAttributes, WindowId},
};

use agent::AgentSession;
pub use agent::EndpointSettings;

/// The sizes at which the highlighter gives up.
///
/// The policy is a product decision, so it is re-exported: the caller of the
/// editor decides how big a file may be before the editor stops colouring it.
pub use codeweaver_syntax::HighlightPolicy;

const WINDOW_TITLE: &str = "CodeWeaver";

/// The height of the strip the tabs are drawn in.
const TAB_BAR_HEIGHT: f32 = 34.0;
/// The width of the panel the file tree is drawn in.
const SIDEBAR_WIDTH: f32 = 220.0;

/// The question the window is asking the user.
///
/// The window asks a question when the answer is not the editor's to give:
/// whether a file with unsaved changes may be closed, and what to do about a
/// file that changed on disk while it was open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// A tab with unsaved changes is about to be closed.
    CloseDirty {
        /// The tab that is about to be closed.
        tab: codeweaver_workbench::TabId,
    },
    /// A file changed on disk while it was open.
    ChangedOnDisk {
        /// The tab whose file changed.
        tab: codeweaver_workbench::TabId,
        /// Whether the file is gone rather than changed.
        removed: bool,
    },
}

impl Prompt {
    /// The question, in words, and the keys that answer it.
    fn text(&self) -> String {
        match self {
            Prompt::CloseDirty { .. } => {
                "这个标签有未保存的修改。\nS 保存并关闭 · D 不保存直接关闭 · Esc 取消".to_owned()
            }
            Prompt::ChangedOnDisk { removed, .. } => {
                if *removed {
                    "这个文件在磁盘上被删除了。\nR 关闭这个标签 · I 先不管".to_owned()
                } else {
                    "这个文件在磁盘上被改过了。\nR 重新加载（丢弃我的修改） · S 保留我的修改（覆盖磁盘） · I 先不管"
                        .to_owned()
                }
            }
        }
    }
}

pub struct NativeApplication {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    input: TextInputState,
    modifiers: ModifiersState,
    /// The agent session, started when the user asks for it.
    agent: Option<AgentSession>,
    /// Why the agent session could not be started, if that happened.
    agent_error: Option<String>,
    /// The workspace services. The UI never touches the file system itself.
    workspace: Workspace,
    /// The last save or load error, shown in the status line.
    status: Option<String>,
    /// How to start the agent endpoint, when it is configured explicitly.
    endpoint: Option<EndpointSettings>,
    /// Where the mouse is, in logical pixels.
    cursor: Option<Point>,
    /// The find bar, when it is open.
    find: Option<FindBar>,
    /// The language server of the open document, if there is one.
    server: Option<LspClient>,
    /// The version the server is told next.
    server_generation: i64,
    /// The completions the server offered, when the list is open.
    completion: Option<CompletionPopup>,
    /// The tabs that are open.
    tabs: TabBar,
    /// The pane of every tab, in the order of the tabs.
    panes: Vec<EditorPane>,
    /// The tab that is on screen.
    active: usize,
    /// The files of the workspace, as a tree.
    tree: Option<FileTree>,
    /// The files that are watched for changes outside the editor.
    detector: ChangeDetector,
    /// The question the window is asking the user, if it is asking one.
    prompt: Option<Prompt>,
    /// The settings panel, when it is open.
    settings: Option<Settings>,
    /// The formatting request that is on its way, and the version of the
    /// text it is about.
    ///
    /// A request that is on its way is not sent again, and an answer that
    /// comes back for a text that has changed since then is not put into the
    /// buffer.
    formatting: Option<u64>,
    /// The change the agent proposed for the file that is open, and the
    /// version of the buffer it was computed against.
    ///
    /// The version is what decides whether the change still fits: a change
    /// that was computed against an older version of the text has to be
    /// merged rather than applied.
    pending_diff: Option<PendingDiff>,
    /// The block of the proposed change the keys act on.
    selected_hunk: usize,
    /// The piece of context the keys act on.
    selected_context: usize,
    /// The conflicts between the buffer and the proposed change.
    ///
    /// They are settled one at a time; the merged text is written once they
    /// all are.
    conflicts: Option<PendingConflict>,
}

/// A change that conflicts with what the buffer holds.
struct PendingConflict {
    /// The path the change is about.
    path: String,
    /// The merge, with its conflicts.
    merge: codeweaver_diff::Merge,
    /// The conflict the keys act on.
    selected: usize,
    /// What the user chose for each conflict.
    choices: Vec<codeweaver_diff::Choice>,
}

/// A change the agent proposed for the file that is open.
struct PendingDiff {
    /// The unified diff the agent sent.
    patch: String,
    /// The version of the buffer when the change arrived.
    base_version: u64,
    /// The text of the buffer when the change arrived.
    base_text: String,
    /// The path the change is about.
    path: String,
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
        let workspace = Workspace::detached();
        let mut tabs = TabBar::new();
        let mut panes: Vec<EditorPane> = Vec::new();
        let mut server = None;
        let mut detector = ChangeDetector::new();
        let mut tree = None;

        match path {
            Some(path) => {
                let opened = workspace
                    .open(&path)
                    .map_err(|error| anyhow::anyhow!("could not open {}: {error}", path.display()))?;
                // The language of the file decides how the text is highlighted.
                let language = Language::from_path(opened.path());
                let tab = tabs.open(path.clone(), shell::title_of(&path));
                let mut pane = EditorPane::opened(tab, opened, language);
                let text = pane.editor.text();

                // The language of the file also decides which language server runs.
                if let Some(language) = language
                    && let Some(config) = server_config.clone().or_else(|| lsp::server_for(language))
                {
                    let root = lsp::workspace_root(&path);
                    match LspClient::start(&config, &root) {
                        Ok(client) => {
                            let uri = codeweaver_lsp::protocol::file_uri(&path);
                            let _ = client.did_open(&uri, lsp::language_id(language), 1, &text);
                            pane.document_uri = Some(uri);
                            server = Some(client);
                        }
                        Err(error) => eprintln!("could not start the language server: {error}"),
                    }
                }
                // The file is watched from the start, so a change behind the
                // editor's back is noticed and not silently overwritten.
                if let Some(document) = pane.document.as_ref() {
                    detector.watch(document.path(), document.fingerprint());
                }
                // The tree of files shows the folder the open file is in.
                tree = path.parent().map(|root| FileTree::new(root.to_path_buf()));
                panes.push(pane);
            }
            None => {
                // An empty editor still has a tab to draw and to click on.
                let tab = tabs.open_scratch("untitled");
                panes.push(EditorPane::scratch(tab, None));
            }
        }

        // The server has been told about the text that is there now.
        if server.is_some() {
            panes[0].server_version = Some(panes[0].editor.version());
        }

        Ok(NativeApplication {
            window: None,
            renderer: None,
            input: TextInputState::default(),
            modifiers: ModifiersState::default(),
            agent: None,
            agent_error: None,
            workspace,
            status: None,
            endpoint: None,
            cursor: None,
            find: None,
            server,
            server_generation: 1,
            completion: None,
            tabs,
            panes,
            active: 0,
            tree,
            detector,
            prompt: None,
            settings: None,
            formatting: None,
            pending_diff: None,
            selected_hunk: 0,
            selected_context: 0,
            conflicts: None,
        })
    }

    /// The pane that is on screen.
    #[cfg(test)]
    fn pane_mut(&mut self) -> &mut EditorPane {
        &mut self.panes[self.active]
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
        &self.panes[self.active].editor
    }

    /// The editor's buffer, for the callers that edit it.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.panes[self.active].editor
    }

    /// The view of the text on screen.
    pub fn view(&self) -> &EditorView {
        &self.panes[self.active].view
    }

    /// The view of the text on screen, for the callers that move it.
    pub fn view_mut(&mut self) -> &mut EditorView {
        &mut self.panes[self.active].view
    }

    /// Sets the sizes at which the highlighter gives up.
    ///
    /// A file that is over the limit is not parsed and not coloured, but it
    /// is still edited, saved and searched. The policy applies to every file
    /// that is open, and to the files that are opened later.
    pub fn set_highlight_policy(&mut self, policy: HighlightPolicy) {
        for pane in &mut self.panes {
            pane.set_policy(policy);
        }
    }

    /// The file that is open, if any.
    pub fn document(&self) -> Option<&Document> {
        self.panes[self.active].document.as_ref()
    }

    /// The file that is open, for the callers that change it.
    pub fn document_mut(&mut self) -> Option<&mut Document> {
        self.panes[self.active].document.as_mut()
    }

    /// Opens a file in a tab.
    ///
    /// A file that is already open is not opened twice: the tab that is
    /// already there comes to the front instead.
    fn open_file(&mut self, path: &Path) -> bool {
        let before = self.tabs.len();
        let tab = self.tabs.open(path.to_path_buf(), shell::title_of(path));
        let index = self.tabs
            .index_of(tab)
            .expect("the tab was just opened, so it is in the list");
        if self.tabs.len() == before {
            // The file was already open.
            self.activate_tab(index);
            return true;
        }

        let opened = match self.workspace.open(path) {
            Ok(document) => document,
            Err(error) => {
                self.status = Some(format!("打不开 {}：{error}", path.display()));
                self.tabs.close(tab);
                return false;
            }
        };
        let language = Language::from_path(opened.path());
        let pane = EditorPane::opened(tab, opened, language);
        // The file is watched, so a change behind the editor's back is
        // noticed instead of being overwritten.
        if let Some(document) = pane.document.as_ref() {
            self.detector.watch(document.path(), document.fingerprint());
        }
        self.panes.push(pane);
        self.show_tree_of(path);
        self.activate_tab(index);
        true
    }

    /// Brings a tab to the front.
    fn activate_tab(&mut self, index: usize) {
        if index >= self.panes.len() || index == self.active {
            return;
        }
        self.tabs.activate_index(index);
        self.active = index;
        // The language server is told about the document that is now on
        // screen; it knows nothing about the ones that were closed.
        self.tell_server_about_the_new_document();
        // The change waiting for the user is the one about the file that is
        // now in front.
        self.note_new_diff();
    }

    /// Closes a tab.
    fn close_tab(&mut self, index: usize) {
        if index >= self.panes.len() {
            return;
        }
        let tab = self.tabs.tabs()[index].id;
        if let Some(document) = self.panes[index].document.as_ref() {
            self.detector.forget(document.path());
        }
        self.panes.remove(index);
        self.tabs.close(tab);

        if self.panes.is_empty() {
            // The last tab was closed: an empty editor takes its place, so
            // there is always something to type into.
            let tab = self.tabs.open_scratch("untitled");
            self.panes.push(EditorPane::scratch(tab, None));
            self.active = 0;
            return;
        }
        // The workbench decides which tab comes to the front.
        let active = self.tabs.active_index().unwrap_or(0);
        self.active = active.min(self.panes.len() - 1);
    }

    /// Closes the tab with the given id, wherever it is.
    fn close_tab_by_id(&mut self, tab: TabId) {
        if let Some(index) = self.tabs.index_of(tab) {
            self.close_tab(index);
        }
    }

    /// Asks the user whether a tab with unsaved changes may be closed.
    ///
    /// A tab without unsaved changes is closed right away; one with them is
    /// a question for the user, because throwing their work away is not the
    /// editor's decision to make.
    fn request_close(&mut self, index: usize) {
        self.sync_dirty_flags();
        let Some(tab) = self.tabs.tabs().get(index) else {
            return;
        };
        match close_decision(tab) {
            CloseDecision::Close => self.close_tab(index),
            CloseDecision::Ask => {
                self.prompt = Some(Prompt::CloseDirty { tab: tab.id });
            }
        }
    }

    /// Brings the dirty flags of the tabs up to date.
    fn sync_dirty_flags(&mut self) {
        for index in 0..self.panes.len() {
            let Some(tab) = self.tabs.tabs().get(index) else {
                continue;
            };
            let id = tab.id;
            let dirty = self.panes[index].is_dirty();
            if tab.dirty != dirty {
                self.tabs.mark_dirty(id, dirty);
            }
        }
    }

    /// Writes the buffer of a tab back to its file.
    fn save_tab(&mut self, tab: TabId) {
        let Some(index) = self.tabs.index_of(tab) else {
            return;
        };
        let Some(mut document) = self.panes[index].document.take() else {
            self.status = Some("这个标签还没有文件".to_owned());
            return;
        };
        let text = self.panes[index].editor.text();
        match self.workspace.save(&mut document, &text) {
            Ok(()) => {
                self.status = Some(format!("已保存 {}", document.display_path()));
                // The file is watched from its new state.
                self.detector.watch(document.path(), document.fingerprint());
            }
            Err(error) => {
                self.status = Some(format!("保存失败：{error}"));
                eprintln!("could not save the document: {error}");
                // The file changed behind the editor's back: the user is
                // asked what to do with it.
                if let codeweaver_workspace::WorkspaceError::ChangedOnDisk { path } = &error {
                    self.prompt = Some(Prompt::ChangedOnDisk {
                        tab,
                        removed: false,
                    });
                    let _ = path;
                }
            }
        }
        self.panes[index].document = Some(document);
    }

    /// Tells the language server about the document that is on screen.
    fn tell_server_about_the_new_document(&mut self) {
        let Some(server) = self.server.as_ref() else {
            return;
        };
        let pane = &mut self.panes[self.active];
        let Some(path) = pane.document.as_ref().map(|document| document.path().to_path_buf())
        else {
            pane.document_uri = None;
            pane.server_version = None;
            return;
        };
        let Some(language) = Language::from_path(&path) else {
            return;
        };
        let uri = codeweaver_lsp::protocol::file_uri(&path);
        let version = pane.editor.version();
        let text = pane.editor.text();
        pane.document_uri = Some(uri.clone());
        pane.server_version = Some(version);
        self.server_generation += 1;
        let generation = self.server_generation;
        let _ = server.did_open(&uri, lsp::language_id(language), generation, &text);
    }

    /// Collects the answer of the highlighter.
    ///
    /// The highlighter runs on a thread of its own, so the window keeps
    /// drawing while the answer is being made. The answer belongs to a
    /// version of the text: an answer that arrives after the text changed
    /// again is about a text that is gone, and is dropped.
    fn pump_syntax(&mut self) -> bool {
        let pane = &mut self.panes[self.active];
        let version = pane.editor.version();
        let visible = pane.view.visible_range();
        let bytes = pane.editor.buffer().len().0;
        let lines = pane.editor.buffer().line_count();

        // A file that is too big is not parsed at all. The decision is made
        // here, on the thread that draws the picture, so the text of a huge
        // file is never copied for a parse that will not happen: the cost of
        // typing in a big file does not grow with the file.
        if !pane.policy.allows(bytes, lines) {
            pane.asked = Some((version, visible));
            let changed = !pane.degraded || !pane.highlights.is_empty();
            pane.degraded = true;
            pane.highlights.clear();
            return changed;
        }

        if pane.asked.as_ref() != Some(&(version, visible.clone())) {
            let text = pane.editor.text();
            pane.syntax.request(version, text, visible.clone(), lines);
            pane.asked = Some((version, visible));
        }
        let Some(outcome) = pane.syntax.poll() else {
            return false;
        };
        if outcome.version != pane.editor.version() {
            // The text changed while the highlighter was working.
            return false;
        }
        let changed = outcome.highlights != pane.highlights || outcome.degraded != pane.degraded;
        pane.highlights = outcome.highlights;
        pane.degraded = outcome.degraded;
        changed
    }

    /// Looks at the files that are open and notices the ones that changed.
    ///
    /// The change is not applied: the user is asked what to do with it. A
    /// file that is overwritten without asking is a file somebody's work is
    /// lost in.
    fn poll_disk(&mut self) -> bool {
        let changes = self.detector.poll();
        if changes.is_empty() {
            return false;
        }
        for change in changes {
            let (path, removed) = match change {
                ExternalChange::Modified { path } => (path, false),
                ExternalChange::Removed { path } => (path, true),
            };
            let Some(index) = self
                .panes
                .iter()
                .position(|pane| pane.document.as_ref().is_some_and(|d| d.path() == path))
            else {
                continue;
            };
            let tab = self.panes[index].tab;
            self.prompt = Some(Prompt::ChangedOnDisk { tab, removed });
        }
        true
    }

    /// Makes the file tree show the directory of a file.
    ///
    /// The tree follows the file that was opened: a tree of the folder of the
    /// file in front of the user is the tree they are working in.
    fn show_tree_of(&mut self, path: &Path) {
        let Some(root) = path.parent() else {
            return;
        };
        let follows = self
            .tree
            .as_ref()
            .map(|tree| tree.root() == root)
            .unwrap_or(false);
        if !follows {
            self.tree = Some(FileTree::new(root.to_path_buf()));
        }
    }

    /// Handles the keys of the question the window is asking.
    ///
    /// While the window is asking, it takes every key: a question that can be
    /// answered by typing into the document is not a question.
    fn handle_prompt_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        let Some(prompt) = self.prompt else {
            return false;
        };
        let letter = match key {
            Key::Character(character) => character
                .as_ref()
                .chars()
                .next()
                .map(|letter| letter.to_ascii_lowercase()),
            _ => None,
        };
        match (prompt, letter) {
            (Prompt::CloseDirty { tab }, Some('s')) => {
                self.prompt = None;
                self.save_tab(tab);
                self.close_tab_by_id(tab);
            }
            (Prompt::CloseDirty { tab }, Some('d')) => {
                self.prompt = None;
                self.close_tab_by_id(tab);
            }
            (Prompt::ChangedOnDisk { tab, removed }, Some('r')) => {
                self.prompt = None;
                if removed {
                    self.close_tab_by_id(tab);
                } else {
                    self.reload_tab(tab);
                }
            }
            (Prompt::ChangedOnDisk { tab, .. }, Some('s')) => {
                self.prompt = None;
                self.save_over_tab(tab);
            }
            // Anything else leaves the question without an answer, which is
            // the same as saying "not now".
            _ => {
                self.prompt = None;
            }
        }
        true
    }

    /// Opens the settings panel.
    ///
    /// The panel starts on the values that are in force now, so what the user
    /// sees is what the editor is doing.
    fn open_settings(&mut self) {
        let policy = self.panes[self.active].policy;
        self.settings = Some(Settings::new(policy));
    }

    /// Handles the keys of the settings panel.
    ///
    /// While the panel is open it takes every key: a setting that can be
    /// changed by typing into the document is not a setting.
    ///
    /// Returns `true` when the key belonged to the panel, so the caller does
    /// not also feed it to the editor.
    fn handle_settings_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        if self.settings.is_none() {
            // Ctrl+, opens the panel.
            if self.modifiers.control_key() && is_comma(key) {
                self.open_settings();
                return true;
            }
            return false;
        }

        // The panel closes on Enter and on Escape.
        if matches!(
            key,
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Escape)
        ) {
            self.settings = None;
            return true;
        }

        let Some(settings) = self.settings.as_mut() else {
            return true;
        };
        let policy = match key {
            Key::Named(NamedKey::ArrowUp) => {
                settings.move_selection(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                settings.move_selection(1);
                None
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if settings.change_value(-1) {
                    Some(settings.policy())
                } else {
                    None
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                if settings.change_value(1) {
                    Some(settings.policy())
                } else {
                    None
                }
            }
            _ => None,
        };
        // A new value is put in force at once: a setting that has to be
        // confirmed before it counts is a setting nobody changes.
        if let Some(policy) = policy {
            self.set_highlight_policy(policy);
            self.status = Some(format!(
                "高亮上限：{} 字节 / {} 行",
                policy.max_bytes, policy.max_lines
            ));
        }
        true
    }

    /// Reads the file of a tab again, throwing away the changes in the buffer.
    fn reload_tab(&mut self, tab: TabId) {
        let Some(index) = self.tabs.index_of(tab) else {
            return;
        };
        let Some(path) = self.panes[index]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())
        else {
            return;
        };
        match self.workspace.open(&path) {
            Ok(document) => {
                let text = document.saved_text().to_owned();
                self.panes[index].editor.load_text(text);
                self.panes[index].document = Some(document);
                self.status = Some(format!("已重新加载 {}", path.display()));
            }
            Err(error) => {
                self.status = Some(format!("重新加载失败：{error}"));
            }
        }
    }

    /// Writes the buffer of a tab over the file, even though the file changed.
    ///
    /// The user was asked and said so; this is the answer to that question.
    fn save_over_tab(&mut self, tab: TabId) {
        let Some(index) = self.tabs.index_of(tab) else {
            return;
        };
        let Some(path) = self.panes[index]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())
        else {
            return;
        };
        let text = self.panes[index].editor.text();
        // The file is read again first, so that the fingerprint the workspace
        // checks against is the one of the file that is on disk now.
        match self.workspace.open(&path) {
            Ok(mut document) => match self.workspace.save(&mut document, &text) {
                Ok(()) => {
                    self.panes[index].document = Some(document);
                    self.detector
                        .watch(self.panes[index].document.as_ref().unwrap().path(), self.panes[index].document.as_ref().unwrap().fingerprint());
                    self.status = Some(format!("已覆盖 {}", path.display()));
                }
                Err(error) => {
                    self.status = Some(format!("保存失败：{error}"));
                }
            },
            Err(error) => {
                self.status = Some(format!("保存失败：{error}"));
            }
        }
    }

    /// Brings the tab that was clicked to the front.
    fn press_tab_bar(&mut self, point: Point, size: Size) {
        let strip = {
            let Some(renderer) = self.renderer.as_mut() else {
                return;
            };
            let mut measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);
            tab_strip(self.tabs.tabs(), self.active, size, &mut measurer)
        };
        self.activate_tab_at(point, &strip);
    }

    /// Brings the tab under the point to the front.
    fn activate_tab_at(&mut self, point: Point, strip: &shell::TabStrip) {
        if let Some(id) = strip.hit(point)
            && let Some(index) = self.tabs.index_of(id)
        {
            self.activate_tab(index);
        }
    }

    /// Opens the file that was clicked, or opens and closes the directory.
    fn press_file_tree(&mut self, point: Point, size: Size) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let rows = shell::tree_rows(tree.nodes(), sidebar_rect(size));
        let Some(row) = shell::tree_hit(&rows, point) else {
            return;
        };
        if row.directory {
            tree.toggle(&row.path);
            return;
        }
        self.open_file(&row.path);
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
        let arrived = match self.agent.as_mut() {
            Some(session) => session.pump(),
            None => false,
        };
        if arrived {
            self.note_new_diff();
        }
        arrived
    }

    /// Records the baseline of a change the agent just proposed.
    ///
    /// The version of the buffer when the change arrives is what the change
    /// is compared against later. It has to be read when the change arrives,
    /// because a moment later the buffer may have moved on and the baseline
    /// would be wrong.
    fn note_new_diff(&mut self) {
        let open_paths = self.open_paths();

        // The baseline of a change is the buffer as it is when the change
        // arrives: that is the text the change is compared against when the
        // user approves it. It is recorded once per file, for every file the
        // change is about and that is open.
        if let Some(session) = self.agent.as_mut() {
            let mut baselines: Vec<(String, u64, String)> = Vec::new();
            for proposal in session.panel().pending_diffs.iter() {
                if proposal.base_version.is_some() {
                    continue;
                }
                let Some(resolved) = paths::resolve_file(&proposal.path, &open_paths) else {
                    continue;
                };
                let Some(index) = self.panes.iter().position(|pane| {
                    pane.document
                        .as_ref()
                        .is_some_and(|document| document.display_path() == resolved)
                }) else {
                    continue;
                };
                let pane = &self.panes[index];
                baselines.push((
                    proposal.path.clone(),
                    pane.editor.version(),
                    pane.editor.text(),
                ));
            }
            for (path, version, text) in baselines {
                session.panel_mut().set_diff_base(&path, version, text);
            }
        }

        // The change waiting for the user is the one about the file that is
        // open.
        let Some(path) = self.open_proposal() else {
            return;
        };
        if self
            .pending_diff
            .as_ref()
            .is_some_and(|pending| pending.path == path)
        {
            return;
        }
        // The patch is read from the tool call the change came with.
        let Some(patch) = self.patch_for(&path) else {
            return;
        };
        self.note_pending_diff(path, patch);
    }

    /// The unified diff the agent sent for a path.
    fn patch_for(&self, path: &str) -> Option<String> {
        let session = self.agent.as_ref()?;
        session.panel().patches.get(path).cloned()
    }

    /// Rebuilds the list of context the user can pick from.
    ///
    /// The list is rebuilt whenever the state it describes changes: a tab
    /// was opened, the selection moved, the file that is open changed.
    pub fn refresh_agent_context(&mut self) {
        let Some(session) = self.agent.as_mut() else {
            return;
        };
        let current = self
            .panes
            .get(self.active)
            .and_then(|pane| pane.document.as_ref())
            .map(|document| document.display_path())
            .zip(
                self.panes
                    .get(self.active)
                    .map(|pane| pane.editor.text()),
            );
        let selection = self
            .panes
            .get(self.active)
            .and_then(|pane| context::selection_text(&pane.editor))
            .zip(
                self.panes
                    .get(self.active)
                    .and_then(|pane| pane.document.as_ref())
                    .map(|document| document.display_path()),
            );
        let tabs: Vec<(String, String)> = self
            .panes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.active)
            .filter_map(|(_, pane)| {
                let document = pane.document.as_ref()?;
                Some((document.display_path(), pane.editor.text()))
            })
            .collect();

        let current = current
            .as_ref()
            .map(|(path, text)| (path.as_str(), text.as_str()));
        let selection = selection
            .as_ref()
            .map(|(path, text)| (path.as_str(), text.as_str()));
        let tab_refs: Vec<(&str, &str)> = tabs
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();

        let items = context::build_context(current, selection, &tab_refs);
        session.panel_mut().set_context(items);
    }

    /// Chooses or unchooses one piece of context.
    pub fn toggle_agent_context(&mut self, index: usize) {
        let Some(session) = self.agent.as_mut() else {
            return;
        };
        let id = session
            .panel()
            .context
            .get(index)
            .map(|item| item.id.clone());
        if let Some(id) = id {
            session.panel_mut().toggle_context(&id);
        }
    }

    /// Picks the next piece of context in or out.
    ///
    /// The context list is short and the user is looking at the panel, so
    /// one key that walks through the list is enough to choose what goes
    /// with the prompt.
    fn cycle_agent_context(&mut self) -> bool {
        let Some(session) = self.agent.as_ref() else {
            return false;
        };
        let count = session.panel().context.len();
        if count == 0 {
            return false;
        }
        let next = (self.selected_context + 1) % count;
        self.selected_context = next;
        self.toggle_agent_context(next);
        true
    }

    /// The path of the file that is open, as the agent names it.
    fn open_path(&self) -> Option<String> {
        self.panes
            .get(self.active)
            .and_then(|pane| pane.document.as_ref())
            .map(|document| document.display_path())
    }

    /// The paths of the files that are open.
    fn open_paths(&self) -> Vec<String> {
        self.panes
            .iter()
            .filter_map(|pane| pane.document.as_ref())
            .map(|document| document.display_path())
            .collect()
    }

    /// The change the agent proposed for the file that is open.
    ///
    /// The agent names a file the way it sees it — relative to the project,
    /// with forward slashes — while the editor knows it by the path it opened
    /// it with. The two names are put together here, so that a change
    /// proposed for `src/app.rs` is found for the file the editor has open.
    fn open_proposal(&self) -> Option<String> {
        let open = self.open_path()?;
        let paths = self.open_paths();
        let session = self.agent.as_ref()?;
        let proposed: Vec<String> = session
            .panel()
            .pending_diffs
            .iter()
            .map(|proposal| proposal.path.clone())
            .collect();
        paths::proposal_for_open_file(&proposed, &open, &paths)
    }

    /// Decides about one block of the proposed change.
    ///
    /// Returns whether there was a change to decide about: a key that means
    /// nothing when no change is waiting is left to the editor.
    fn decide_hunk(&mut self, index: usize, decision: HunkDecision) -> bool {
        let Some(path) = self.open_proposal() else {
            return false;
        };
        let Some(session) = self.agent.as_mut() else {
            return false;
        };
        let Some(proposal) = session.panel().diff_for(&path) else {
            return false;
        };
        if index >= proposal.hunks.len() {
            return false;
        }
        session.panel_mut().decide_hunk(&path, index, decision);
        true
    }

    /// Picks the block the next keys act on.
    fn select_hunk(&mut self, index: usize) -> bool {
        let Some(path) = self.open_proposal() else {
            return false;
        };
        let Some(session) = self.agent.as_mut() else {
            return false;
        };
        let Some(proposal) = session.panel().diff_for(&path) else {
            return false;
        };
        if index >= proposal.hunks.len() {
            return false;
        }
        self.selected_hunk = index;
        true
    }

    /// Decides about the block that is picked.
    fn decide_selected_hunk(&mut self, decision: HunkDecision) -> bool {
        self.decide_hunk(self.selected_hunk, decision)
    }

    /// Decides about every block of the proposed change.
    fn decide_whole_file(&mut self, decision: HunkDecision) -> bool {
        let Some(path) = self.open_proposal() else {
            return false;
        };
        let Some(session) = self.agent.as_mut() else {
            return false;
        };
        if session.panel().diff_for(&path).is_none() {
            return false;
        }
        session.panel_mut().decide_all_hunks(&path, decision);
        true
    }

    /// Records the change the agent proposed for the file that is open.
    ///
    /// The version of the buffer at this moment is the baseline: it is what
    /// the change is compared against when the user approves it. When the
    /// panel already holds a baseline — it was recorded when the change
    /// arrived — that one is used.
    fn note_pending_diff(&mut self, path: String, patch: String) {
        let Some(pane) = self.panes.get(self.active) else {
            return;
        };
        let current_version = pane.editor.version();
        let current_text = pane.editor.text();
        let baseline = self
            .agent
            .as_ref()
            .and_then(|session| session.panel().diff_for(&path))
            .and_then(|proposal| Some((proposal.base_version?, proposal.base_text.clone()?)));
        let (base_version, base_text) = baseline.unwrap_or((current_version, current_text));
        self.pending_diff = Some(PendingDiff {
            patch,
            base_version,
            base_text,
            path,
        });
    }

    /// Applies the approved blocks of the pending change to the buffer.
    ///
    /// The change is written as one transaction, so one undo takes the whole
    /// of it back. When the buffer moved on since the change was proposed,
    /// the two are merged; a merge that still disagrees is not written, and
    /// the conflicts are shown instead.
    fn apply_pending_diff(&mut self) {
        let Some(pending) = self.pending_diff.take() else {
            return;
        };
        // A change is written into the file that is open. A change about
        // another file waits for that file to come to the front.
        let open = self.open_path();
        let open_paths = self.open_paths();
        if paths::resolve_file(&pending.path, &open_paths).as_deref() != open.as_deref() {
            self.pending_diff = Some(pending);
            self.status = Some("这个改动不属于当前打开的文件".to_owned());
            return;
        }
        let Some(session) = self.agent.as_mut() else {
            return;
        };
        let approved = session.panel().approved_hunks(&pending.path);
        if approved.is_empty() {
            return;
        }
        let Some(proposal) = session.panel().diff_for(&pending.path).cloned() else {
            return;
        };

        let Some(pane) = self.panes.get(self.active) else {
            return;
        };
        let current_text = pane.editor.text();
        let current_version = pane.editor.version();

        let outcome = diff_review::apply_proposal(
            &proposal,
            &pending.patch,
            &pending.base_text,
            pending.base_version,
            &current_text,
            current_version,
            &approved,
        );

        let message = match outcome {
            diff_review::ApplyOutcome::Applied {
                text,
                hunks,
                added,
                removed,
                ..
            } => {
                // The text is written as one transaction, so one undo takes
                // the whole change back.
                let pane = &mut self.panes[self.active];
                let length = pane.editor.buffer().len();
                let range = TextOffset(0)..length;
                if pane.editor.replace_ranges(vec![range], &text).is_err() {
                    return;
                }
                // The tab now says that it has changes that are not on disk.
                self.sync_dirty_flags();
                format!("已应用 {hunks} 个变更块（+{added} -{removed}）")
            }
            diff_review::ApplyOutcome::DoesNotFit { line, expected, found, .. } => {
                format!("第 {line} 行对不上：期望「{expected}」，实际「{found}」")
            }
            diff_review::ApplyOutcome::Conflict { merge } => {
                // The two disagree. The text is not written: the conflicts
                // are settled one at a time, and the merged text is written
                // once they all are.
                let count = diff_review::conflict_count(&merge);
                self.conflicts = Some(PendingConflict {
                    path: pending.path.clone(),
                    merge,
                    selected: 0,
                    choices: Vec::new(),
                });
                format!("缓冲区已经变了，{count} 处冲突待处理")
            }
        };

        let session = self.agent.as_mut().expect("the session is there");
        session.panel_mut().record_applied(message.clone());
        self.status = Some(message);
    }

    /// Settles one conflict: the buffer's version or the agent's.
    ///
    /// The last conflict settles the whole thing: the merged text is
    /// written into the buffer as one transaction.
    fn settle_conflict(&mut self, choice: codeweaver_diff::Choice) -> bool {
        let Some(pending) = self.conflicts.as_mut() else {
            return false;
        };
        let index = pending.selected;
        if index >= pending.merge.conflicts.len() {
            return false;
        }
        pending.choices.push(choice);
        pending.selected += 1;

        let settled = pending.selected >= pending.merge.conflicts.len();
        let path = pending.path.clone();
        if !settled {
            return true;
        }

        let text = diff_review::merged_text(&pending.merge, &pending.choices);
        let conflicts = self.conflicts.take();
        let _ = conflicts;

        let pane = &mut self.panes[self.active];
        let length = pane.editor.buffer().len();
        let range = TextOffset(0)..length;
        if pane.editor.replace_ranges(vec![range], &text).is_err() {
            return true;
        }
        // The tab now says that it has changes that are not on disk.
        self.sync_dirty_flags();

        let message = format!("冲突已合并，写入 {path}");
        let session = self.agent.as_mut().expect("the session is there");
        session.panel_mut().record_applied(message.clone());
        self.status = Some(message);
        true
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
        let pane = &mut self.panes[self.active];
        pane.view.layout(
            pane.editor.buffer(),
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
                    if Some(uri.as_str()) != self.panes[self.active].document_uri.as_deref() {
                        continue;
                    }
                    let marks = lsp::marks_for(self.panes[self.active].editor.buffer(), &diagnostics);
                    self.panes[self.active].errors = marks
                        .iter()
                        .filter(|mark| mark.severity == MarkSeverity::Error)
                        .count();
                    self.panes[self.active].warnings = marks
                        .iter()
                        .filter(|mark| mark.severity == MarkSeverity::Warning)
                        .count();
                    self.panes[self.active].marks = marks;
                    changed = true;
                }
                LspEvent::Disconnected { reason } => {
                    eprintln!("the language server went away: {reason}");
                    self.status = Some(format!("语言服务器断开了：{reason}"));
                    self.server = None;
                    self.panes[self.active].marks.clear();
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
            let caret = self.panes[self.active].editor.caret().0;
            let prefix = completion::word_before(self.panes[self.active].editor.buffer(), caret);
            let items = completion::matching(list.items, &prefix);
            let start = completion::word_start(self.panes[self.active].editor.buffer(), caret);
            if let Some(popup) = CompletionPopup::new(items, start) {
                self.completion = Some(popup);
                changed = true;
            }
        }

        // The formatting answers are put into the buffer. The edits of one
        // answer are one change of the document.
        let formattings = match self.server.as_ref() {
            Some(server) => server.drain_formattings(),
            None => Vec::new(),
        };
        for answer in formattings {
            match answer {
                Ok(edits) => {
                    // The answer is only about the text that was there when
                    // it was asked for. A text that changed in the meantime
                    // is a different text, and the edits do not fit it.
                    let Some(sent_at) = self.formatting.take() else {
                        continue;
                    };
                    if self.apply_formatting(edits, sent_at) {
                        changed = true;
                    }
                }
                Err(error) => {
                    self.formatting = None;
                    eprintln!("could not format the file: {error}");
                    self.status = Some(format!("格式化失败：{error}"));
                    changed = true;
                }
            }
        }
        changed
    }

    /// Tells the language server what the text looks like now.
    ///
    /// The server is told about every edit, so its answers are about the text
    /// that is on screen and not about the text as the file was opened.
    fn tell_server_about_the_change(&mut self) {
        let Some(uri) = self.panes[self.active].document_uri.clone() else {
            return;
        };
        let Some(change) = self.panes[self.active].editor.last_change() else {
            return;
        };
        let version = self.panes[self.active].editor.buffer().version();
        if self.panes[self.active].server_version == Some(version) {
            // The server already knows about this text.
            return;
        }
        let changes = lsp::changes_for(self.panes[self.active].editor.buffer(), change);
        let text = self.panes[self.active].editor.text();
        self.panes[self.active].server_version = Some(version);
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
        let pane = &self.panes[self.active];
        let total_lines = pane.editor.buffer().line_count();
        let visible = self.visible_lines(scale_factor);
        let caret = pane.editor.caret().0;
        let pane = &mut self.panes[self.active];
        pane.view
            .ensure_caret_visible(pane.editor.buffer(), caret, total_lines, visible);
    }

    /// Handles a mouse press in the editor body.
    ///
    /// Ctrl+click adds a caret instead of moving the one that is there.
    fn press_body(&mut self, scale_factor: f64) {
        let Some(cursor) = self.cursor else {
            return;
        };
        let size = self.window_size(scale_factor);
        // The tab strip and the file tree are around the text: a click there
        // is not a click in the document.
        if tab_bar_rect(size).contains(cursor) {
            self.press_tab_bar(cursor, size);
            return;
        }
        if sidebar_rect(size).contains(cursor) {
            self.press_file_tree(cursor, size);
            return;
        }
        let body = editor_body_rect(size);
        if !body.contains(cursor) {
            return;
        }
        let add = self.modifiers.control_key();
        let pane = &mut self.panes[self.active];
        pane.view.press(
            &mut pane.editor,
            cursor.x - body.origin.x,
            cursor.y - body.origin.y,
            add,
        );
        self.keep_caret_visible(scale_factor);
    }

    /// Handles the mouse moving while a drag is in progress.
    fn drag_body(&mut self, scale_factor: f64) {
        if !self.panes[self.active].view.is_dragging() {
            return;
        }
        let Some(cursor) = self.cursor else {
            return;
        };
        let body = editor_body_rect(self.window_size(scale_factor));
        let pane = &mut self.panes[self.active];
        pane.view.drag(
            &mut pane.editor,
            cursor.x - body.origin.x,
            cursor.y - body.origin.y,
        );
    }

    /// Moves the caret one visual line up or down.
    ///
    /// When the caret is on the edge of the screen, the body scrolls first,
    /// so the caret can keep moving.
    fn move_caret_vertically(&mut self, scale_factor: f64, delta: isize) {
        {
            let pane = &mut self.panes[self.active];
            if pane.view.move_vertical(&mut pane.editor, delta) {
                return;
            }
        }
        let total_lines = self.panes[self.active].editor.buffer().line_count();
        self.panes[self.active].view.scroll(delta, total_lines);
        self.rebuild_layout(scale_factor);
        let pane = &mut self.panes[self.active];
        pane.view.move_vertical(&mut pane.editor, delta);
    }

    /// Scrolls the body by a number of lines.
    fn scroll_body(&mut self, lines: isize) {
        let total_lines = self.panes[self.active].editor.buffer().line_count();
        self.panes[self.active].view.scroll(lines, total_lines);
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

        // Ctrl+Alt+Enter writes the approved blocks into the buffer.
        if control && alt && matches!(key, Key::Named(NamedKey::Enter)) {
            if self.pending_diff.is_some() {
                self.apply_pending_diff();
                return true;
            }
            return false;
        }

        let Some(letter) = key_letter(key) else {
            return false;
        };
        let letter = letter.to_ascii_lowercase();

        // Ctrl+Alt+A allows the pending permission, Ctrl+Alt+N denies it.
        //
        // A permission and a proposed change never wait at the same time: the
        // agent asks for permission before it proposes anything, and the
        // answer to a permission is the one thing it is waiting for. So the
        // same two keys serve both, and the one that is actually waiting
        // gets them.
        if control && alt {
            let waiting_for_permission = self
                .agent
                .as_ref()
                .and_then(|session| session.panel().pending_permission.as_ref())
                .is_some();
            if waiting_for_permission {
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
        }

        // The keys that decide about a proposed change. They only act when
        // there is a change waiting for the open file; a key that means
        // nothing is left to the editor.
        if control && alt {
            // Ctrl+Alt+1..9 pick the block the next keys act on.
            let handled = match letter {
                '1'..='9' => {
                    let index = letter.to_digit(10).unwrap_or(1) as usize - 1;
                    self.select_hunk(index)
                }
                // Ctrl+Alt+0 approves the whole file, Ctrl+Alt+Q rejects it.
                '0' => self.decide_whole_file(HunkDecision::Approved),
                'q' => self.decide_whole_file(HunkDecision::Rejected),
                // Ctrl+Alt+C picks the next piece of context in and out.
                'c' => self.cycle_agent_context(),
                // Ctrl+Alt+K keeps the buffer's side of a conflict,
                // Ctrl+Alt+J takes the agent's.
                'k' => self.settle_conflict(codeweaver_diff::Choice::Ours),
                'j' => self.settle_conflict(codeweaver_diff::Choice::Theirs),
                // Ctrl+Alt+A approves the picked block, Ctrl+Alt+N rejects
                // it. The same two keys answer a permission request, because
                // approving a change and allowing a tool feel the same to
                // the hand.
                'a' => self.decide_selected_hunk(HunkDecision::Approved),
                'n' => self.decide_selected_hunk(HunkDecision::Rejected),
                _ => false,
            };
            if handled {
                return true;
            }
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
                if let Some(tab) = self.tabs.active().map(|tab| tab.id) {
                    self.save_tab(tab);
                    self.sync_dirty_flags();
                }
                true
            }
            // Ctrl+W closes the tab that is in front.
            'w' => {
                self.request_close(self.active);
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
                    if let Err(error) = completion.apply(&mut self.panes[self.active].editor) {
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
        let Some(uri) = self.panes[self.active].document_uri.clone() else {
            return;
        };
        let caret = self.panes[self.active].editor.caret().0;
        let position = lsp::position_of(self.panes[self.active].editor.buffer(), caret);
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

    /// Asks the server to format the file that is open.
    ///
    /// The answer comes later: it is put into the buffer by
    /// [`Self::apply_formatting`], when the window collects what the server
    /// has to say.
    fn request_format(&mut self) {
        if self.formatting.is_some() {
            // A request is already on its way. Asking again would make the
            // server work twice for one press of the key.
            return;
        }
        let Some(uri) = self.panes[self.active].document_uri.clone() else {
            self.status = Some("这个文件没有语言服务器，格式化不了".to_owned());
            return;
        };
        let outcome = self
            .server
            .as_ref()
            .map(|server| server.request_formatting(&uri, 4, true));
        match outcome {
            Some(Ok(())) => {
                // The version is the version of the text the request is
                // about: an answer that comes back for a text that has
                // changed since then does not fit the text that is there
                // now.
                self.formatting = Some(self.panes[self.active].editor.version());
                self.status = Some("正在格式化…".to_owned());
            }
            Some(Err(error)) => {
                eprintln!("could not ask the server to format the file: {error}");
                self.status = Some(format!("格式化请求失败：{error}"));
            }
            None => self.status = Some("没有语言服务器，格式化不了".to_owned()),
        }
    }

    /// Puts the edits of a formatting answer into the buffer.
    ///
    /// The edits are one change of the document: they go in as one
    /// transaction, so one undo takes the whole formatting back.
    ///
    /// `sent_at` is the version of the text the request was made about: an
    /// answer for a text that has changed since then does not fit the text
    /// that is there now, and it is not put in.
    ///
    /// Returns whether the text changed.
    fn apply_formatting(&mut self, edits: Vec<TextEdit>, sent_at: u64) -> bool {
        if sent_at != self.panes[self.active].editor.version() {
            self.status = Some("文件在格式化期间变了，格式化结果没有用上".to_owned());
            return false;
        }
        let Some(changes) =
            lsp::formatting_changes(self.panes[self.active].editor.buffer(), &edits)
        else {
            self.status = Some("格式化结果对不上这个文件，没有改".to_owned());
            return false;
        };
        if changes.is_empty() {
            self.status = Some("这个文件已经排好了版".to_owned());
            return false;
        }
        if let Err(error) = self.panes[self.active].editor.replace_edits(changes) {
            eprintln!("could not put the formatting in: {error}");
            self.status = Some(format!("格式化结果写不进去：{error}"));
            return false;
        }
        // The tab now says that it has changes that are not on disk, and the
        // server has to hear about the new text: its answers are about the
        // text that is on screen and not about the text as the file was
        // opened.
        self.sync_dirty_flags();
        self.tell_server_about_the_change();
        self.status = Some("已格式化".to_owned());
        true
    }

    /// Handles the key that asks the language server to format the file.
    ///
    /// Alt+Shift+F formats, the way it does in the other editors. Ctrl+Shift+F
    /// is left for finding in files.
    fn handle_format_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        if !(self.modifiers.alt_key() && self.modifiers.shift_key()) {
            return false;
        }
        let Some(letter) = key_letter(key) else {
            return false;
        };
        if !letter.eq_ignore_ascii_case(&'f') {
            return false;
        }
        self.request_format();
        true
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
                    let count = find.replace_all(&mut self.panes[self.active].editor);
                    self.status = Some(format!("替换了 {count} 处"));
                } else if self.modifiers.alt_key() {
                    let count = find.select_all_matches(&mut self.panes[self.active].editor);
                    self.status = Some(format!("选中了 {count} 处"));
                } else if self.modifiers.shift_key() {
                    find.previous_match();
                } else {
                    find.next_match();
                }
            }
            Key::Named(NamedKey::Tab) => find.toggle_field(),
            Key::Named(NamedKey::Backspace) => find.backspace(&self.panes[self.active].editor),
            Key::Named(NamedKey::Space) => find.type_text(&self.panes[self.active].editor, " "),
            Key::Character(character) if !self.modifiers.control_key() => {
                let character = character.as_ref();
                if character.chars().any(char::is_control) {
                    // The key did not produce text of its own; the text of
                    // the event is what the user typed.
                    if let Some(typed) = text.filter(|typed| !typed.chars().any(char::is_control)) {
                        find.type_text(&self.panes[self.active].editor, typed);
                    } else {
                        handled = false;
                    }
                } else {
                    find.type_text(&self.panes[self.active].editor, character);
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
        find.open(&mut self.panes[self.active].editor, replacing);
        self.find = Some(find);
    }

    /// Opens the find bar and searches for the given text.
    ///
    /// This is what the editor does when it is started with a search, so the
    /// find bar can be looked at without a keyboard.
    pub fn open_find_with(&mut self, query: &str) {
        let mut find = self.find.take().unwrap_or_default();
        find.open(&mut self.panes[self.active].editor, false);
        find.type_text(&self.panes[self.active].editor, query);
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
        let mut status = match self.panes[self.active].document.as_ref() {
            Some(document) => {
                let dirty = if document.is_dirty(&self.panes[self.active].editor.text()) {
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
        if self.panes[self.active].errors > 0 || self.panes[self.active].warnings > 0 {
            status.push_str(&format!("  ·  ✕ {}  ⚠ {}", self.panes[self.active].errors, self.panes[self.active].warnings));
        }
        // A file that is too big is not coloured, and the user is told why:
        // a document that quietly loses its colours looks broken.
        if self.panes[self.active].degraded {
            let policy = self.panes[self.active].policy;
            status.push_str(&format!(
                "  ·  降级模式：文件超过 {} 字节或 {} 行，不做语法高亮",
                policy.max_bytes, policy.max_lines
            ));
        }
        if let Some(message) = self.status.as_ref() {
            status.push_str(&format!("  ·  {message}"));
        }
        status
    }

    /// Starts an agent turn with the editor's current text as the prompt.
    ///
    /// The text the user chose as context travels with the prompt as
    /// separate blocks, so the agent knows which is the request and which is
    /// the material.
    pub fn start_agent_turn(&mut self) {
        let prompt = self.panes[self.active].editor.text();
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
            if let Some(document) = self.panes[self.active].document.as_ref()
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

        self.refresh_agent_context();
        let context: Vec<ContextItem> = self
            .agent
            .as_ref()
            .map(|session| {
                session
                    .panel()
                    .selected_context()
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        if let Some(session) = self.agent.as_mut()
            && let Err(error) = session.start_prompt_with_context(&prompt, &context)
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
        // The agent answers on its own thread, so the window has to wake up
        // from time to time to hear what it said. Without this, the panel
        // stays as it was until the user happened to move the mouse.
        if self.pump_agent()
            && let Some(window) = self.window.as_ref()
        {
            window.request_redraw();
        }
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
                            find.type_text(&self.panes[self.active].editor, &text);
                        } else if let Err(error) = self.panes[self.active].editor.insert(&text) {
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
                        ElementState::Released => self.panes[self.active].view.release(),
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
                // While the window is asking a question, it takes every key.
                if self.handle_prompt_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // The settings panel takes every key while it is open, and
                // Ctrl+, opens it.
                if self.handle_settings_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // Alt+Shift+F asks the language server to format the file.
                if self.handle_format_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
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
                    Key::Named(NamedKey::Backspace) => self.panes[self.active].editor.backspace(),
                    Key::Named(NamedKey::Delete) => self.panes[self.active].editor.delete_forward(),
                    Key::Named(NamedKey::ArrowLeft) if self.modifiers.shift_key() => {
                        self.panes[self.active].editor.extend_left();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowRight) if self.modifiers.shift_key() => {
                        self.panes[self.active].editor.extend_right();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowUp) if self.modifiers.shift_key() => {
                        let pane = &mut self.panes[self.active];
                        pane.view.extend_vertical(&mut pane.editor, -1);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowDown) if self.modifiers.shift_key() => {
                        let pane = &mut self.panes[self.active];
                        pane.view.extend_vertical(&mut pane.editor, 1);
                        Ok(())
                    }
                    Key::Named(NamedKey::Home) if self.modifiers.shift_key() => {
                        let pane = &mut self.panes[self.active];
                        pane.view.extend_to_line_edge(&mut pane.editor, false);
                        Ok(())
                    }
                    Key::Named(NamedKey::End) if self.modifiers.shift_key() => {
                        let pane = &mut self.panes[self.active];
                        pane.view.extend_to_line_edge(&mut pane.editor, true);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        self.panes[self.active].editor.move_left();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        self.panes[self.active].editor.move_right();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowUp) if self.modifiers.alt_key() => {
                        let pane = &mut self.panes[self.active];
                        pane.view.add_cursor_vertically(&mut pane.editor, -1);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowDown) if self.modifiers.alt_key() => {
                        let pane = &mut self.panes[self.active];
                        pane.view.add_cursor_vertically(&mut pane.editor, 1);
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
                        self.panes[self.active].editor.collapse_to_primary();
                        Ok(())
                    }
                    Key::Named(NamedKey::Home) => {
                        let pane = &mut self.panes[self.active];
                        pane.view.move_to_line_edge(&mut pane.editor, false);
                        Ok(())
                    }
                    Key::Named(NamedKey::End) => {
                        let pane = &mut self.panes[self.active];
                        pane.view.move_to_line_edge(&mut pane.editor, true);
                        Ok(())
                    }
                    Key::Named(NamedKey::Enter) => self.panes[self.active].editor.insert("\n"),
                    Key::Character(character)
                        if self.modifiers.control_key() && character.eq_ignore_ascii_case("z") =>
                    {
                        self.panes[self.active].editor.undo()
                    }
                    Key::Character(character)
                        if self.modifiers.control_key() && character.eq_ignore_ascii_case("y") =>
                    {
                        self.panes[self.active].editor.redo()
                    }
                    _ if self.input.preedit().is_empty()
                        && !self.modifiers.control_key()
                        && let Some(text) = event.text
                        && !text.chars().any(char::is_control) =>
                    {
                        self.panes[self.active].editor.insert(&text)
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
                // The tab now says that it has changes that are not on disk.
                self.sync_dirty_flags();
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                // The agent's output is collected once per frame, so a reply
                // appears while the agent is still producing it. Collecting it
                // here is also what records the baseline of a change the
                // agent proposes.
                if self.pump_agent() {
                    window.request_redraw();
                }
                // The language server answers on its own thread, so its
                // answers are collected the same way.
                if self.pump_lsp() {
                    window.request_redraw();
                }
                // The highlighter answers on its own thread as well. The
                // answer belongs to a version of the text: an answer that
                // arrives after the text changed again is dropped.
                if self.pump_syntax() {
                    window.request_redraw();
                }
                // The files on disk are looked at now and then, so a file
                // that changed behind the editor's back is noticed.
                if self.poll_disk() {
                    window.request_redraw();
                }
                // The status line is read before the renderer is borrowed.
                let status_line = self.status_line();
                let scale_factor = window.scale_factor();
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                let pane = &mut self.panes[self.active];
                let prompt_text = self.prompt.as_ref().map(|prompt| prompt.text());
                let conflict = self.conflicts.as_ref().map(|pending| ConflictView {
                    total: pending.merge.conflicts.len(),
                    selected: pending.selected,
                    items: pending
                        .merge
                        .conflicts
                        .iter()
                        .map(|conflict| ConflictItem {
                            ours: conflict.ours.join(" / "),
                            theirs: conflict.theirs.join(" / "),
                        })
                        .collect(),
                });
                let scene = build_scene(
                    renderer,
                    renderer.logical_size(scale_factor),
                    &pane.editor,
                    &self.input,
                    &mut pane.view,
                    &pane.highlights,
                    Chrome {
                        agent: self.agent.as_ref(),
                        agent_error: self.agent_error.as_deref(),
                        status_line,
                        find: self.find.as_ref(),
                        marks: &pane.marks,
                        completion: self.completion.as_ref(),
                        tabs: self.tabs.tabs(),
                        active: self.active,
                        tree: self.tree.as_ref(),
                        prompt: prompt_text.as_deref(),
                        settings: self.settings.as_ref(),
                        conflicts: conflict,
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

/// The letter a key stands for, when it stands for one.
///
/// A machine with a Chinese input method does not name the keys that are
/// pressed with Ctrl and Alt: the layout has no letters for that
/// combination, and the keys arrive unidentified. The virtual key code is the
/// same on every layout, so the letter is taken from there. The shortcuts
/// then work the same way wherever the editor runs.
fn key_letter<T: AsRef<str>>(key: &Key<T>) -> Option<char> {
    match key {
        Key::Character(character) => character.as_ref().chars().next(),
        Key::Unidentified(NativeKey::Windows(code)) => {
            let letter = char::from_u32(u32::from(*code))?;
            // Only the letters and the digits are shortcuts; the other codes
            // are keys like Enter, which are named by the window.
            letter.is_ascii_alphanumeric().then_some(letter)
        }
        _ => None,
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
    /// The tabs that are open.
    tabs: &'a [Tab],
    /// The tab that is on screen.
    active: usize,
    /// The files of the workspace.
    tree: Option<&'a FileTree>,
    /// The question the window is asking, if it is asking one.
    prompt: Option<&'a str>,
    /// The settings panel, when it is open.
    settings: Option<&'a Settings>,
    /// The conflicts between the buffer and a proposed change.
    conflicts: Option<ConflictView>,
}

/// What the panel shows about the conflicts.
struct ConflictView {
    /// How many conflicts there are.
    total: usize,
    /// Which one the keys act on, counting from zero.
    selected: usize,
    /// The two sides of each conflict.
    items: Vec<ConflictItem>,
}

/// The two sides of one conflict.
struct ConflictItem {
    /// What the buffer has.
    ours: String,
    /// What the agent proposed.
    theirs: String,
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

/// Whether the key is the comma, whatever the keyboard layout says.
///
/// The comma opens the settings panel, and a machine with a Chinese input
/// method does not name the keys that are pressed with Ctrl: the virtual key
/// code is the same on every layout, so it is the second opinion.
fn is_comma<T: AsRef<str>>(key: &Key<T>) -> bool {
    match key {
        Key::Character(character) => character.as_ref() == ",",
        // VK_OEM_COMMA, the key the comma is on.
        Key::Unidentified(NativeKey::Windows(code)) => *code == 0xBC,
        _ => false,
    }
}

fn build_scene(
    renderer: &mut Renderer,
    size: Size,
    editor_state: &Editor,
    input: &TextInputState,
    view: &mut EditorView,
    highlights: &[Highlight],
    chrome: Chrome<'_>,
) -> FrameScene {
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let body = editor_body_rect(size);
    let mut scene = FrameScene::default();

    // A panel that is drawn over the window hides what is under it.
    //
    // The renderer draws every rectangle first and every text after it, so a
    // text that lies under the panel would be drawn *on* it. The window under
    // the panel is therefore left out of the picture, not covered by it.
    if let Some(settings) = chrome.settings {
        push_settings(&mut scene, viewport, size, settings);
        return scene;
    }

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

        // The highlights were made on the highlighter's own thread; they are
        // the ones that belong to the text that is on screen.
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
            Rect::new(SIDEBAR_WIDTH + 16.0, TAB_BAR_HEIGHT + 6.0, size.width - 72.0, 28.0),
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
            Rect::new(
                SIDEBAR_WIDTH + 16.0,
                TAB_BAR_HEIGHT + 26.0,
                size.width - 72.0,
                22.0,
            ),
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

    push_agent_panel(
        &mut scene,
        size,
        chrome.agent,
        chrome.agent_error,
        chrome.conflicts.as_ref(),
    );

    // The find bar is drawn in the sans-serif face, so it gets a measurer of
    // its own.
    let mut ui_measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);

    // The strip of tabs and the tree of files are part of the window, not of
    // the text: they are drawn around it. The strip is laid out here so that
    // the place of a tab is worked out the same way twice — once to draw it,
    // once to click it.
    let strip = tab_strip(chrome.tabs, chrome.active, size, &mut ui_measurer);
    push_tab_bar(&mut scene, viewport, tab_bar_rect(size), &strip.items);
    if let Some(tree) = chrome.tree {
        let rows = shell::tree_rows(tree.nodes(), sidebar_rect(size));
        push_file_tree(&mut scene, viewport, sidebar_rect(size), &rows);
    }

    // The question the window is asking is drawn last, over everything.
    if let Some(prompt) = chrome.prompt {
        push_prompt(&mut scene, viewport, size, prompt);
    }

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

/// The area the tab strip is drawn in.
fn tab_bar_rect(size: Size) -> Rect {
    let panel = agent_panel_rect(size);
    Rect::new(
        0.0,
        0.0,
        (panel.origin.x - 16.0).max(1.0),
        TAB_BAR_HEIGHT,
    )
}

/// Works out where the tabs are drawn.
///
/// The drawing and the clicking both go through here: a tab that is drawn in
/// one place and clicked in another is a tab that cannot be clicked.
fn tab_strip(
    tabs: &[Tab],
    active: usize,
    size: Size,
    measurer: &mut dyn TextMeasurer,
) -> shell::TabStrip {
    let dirty: Vec<bool> = tabs.iter().map(|tab| tab.dirty).collect();
    shell::TabStrip::layout(tabs, &dirty, active, tab_bar_rect(size), measurer)
}

/// The area the file tree is drawn in.
fn sidebar_rect(size: Size) -> Rect {
    Rect::new(
        0.0,
        TAB_BAR_HEIGHT,
        SIDEBAR_WIDTH,
        (size.height - TAB_BAR_HEIGHT - 32.0).max(1.0),
    )
}

/// The area the text body is drawn in.
fn editor_body_rect(size: Size) -> Rect {
    let panel = agent_panel_rect(size);
    let left = SIDEBAR_WIDTH + 16.0;
    let right = panel.origin.x - 16.0;
    Rect::new(
        left,
        TAB_BAR_HEIGHT + 44.0,
        (right - left).max(1.0),
        (size.height - TAB_BAR_HEIGHT - 76.0).max(1.0),
    )
}

/// Draws the tab strip.
///
/// The tab that is on screen is lighter than the others, and a tab with
/// unsaved changes carries a dot: a tab that is not marked is a tab whose
/// changes the user cannot see.
fn push_tab_bar(
    scene: &mut FrameScene,
    viewport: Rect,
    strip: Rect,
    items: &[shell::TabItem],
) {
    scene.push_quad(strip, Color::rgb(0.10, 0.12, 0.16), viewport);
    scene.push_quad(
        Rect::new(strip.origin.x, strip.origin.y + strip.size.height - 1.0, strip.size.width, 1.0),
        Color::rgb(0.20, 0.23, 0.29),
        viewport,
    );

    for item in items {
        let rect = item.rect;
        let background = if item.active {
            Color::rgb(0.16, 0.19, 0.25)
        } else {
            Color::rgb(0.12, 0.14, 0.18)
        };
        scene.push_quad(rect, background, viewport);
        if item.active {
            scene.push_quad(
                Rect::new(rect.origin.x, rect.origin.y, rect.size.width, 2.0),
                Color::rgb(0.30, 0.58, 0.96),
                viewport,
            );
        }
        let text = Rect::new(
            rect.origin.x + shell::TAB_PADDING,
            rect.origin.y + 7.0,
            (rect.size.width - shell::TAB_PADDING * 2.0).max(1.0),
            20.0,
        );
        scene.push_text(
            item.title.clone(),
            text,
            TextStyle {
                clip: viewport,
                color: if item.active {
                    Color::rgb(0.88, 0.90, 0.94)
                } else {
                    Color::rgb(0.58, 0.62, 0.70)
                },
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
        if item.dirty {
            scene.push_text(
                "●",
                Rect::new(
                    rect.origin.x + rect.size.width - shell::TAB_PADDING - 4.0,
                    rect.origin.y + 6.0,
                    16.0,
                    20.0,
                ),
                TextStyle {
                    clip: viewport,
                    color: Color::rgb(0.30, 0.58, 0.96),
                    font_size: 11.0,
                    line_height: 16.0,
                    family: TextFamily::SansSerif,
                },
            );
        }
    }
}

/// Draws the file tree.
///
/// A directory that is open shows a down arrow, a closed one a right arrow;
/// the depth of a row is shown by how far it is indented.
fn push_file_tree(
    scene: &mut FrameScene,
    viewport: Rect,
    panel: Rect,
    rows: &[shell::TreeRow],
) {
    scene.push_quad(panel, Color::rgb(0.10, 0.12, 0.16), viewport);
    scene.push_quad(
        Rect::new(
            panel.origin.x + panel.size.width - 1.0,
            panel.origin.y,
            1.0,
            panel.size.height,
        ),
        Color::rgb(0.20, 0.23, 0.29),
        viewport,
    );
    for row in rows {
        let indent = panel.origin.x + 8.0 + row.depth as f32 * shell::INDENT;
        let marker = if row.directory {
            if row.expanded { "▾" } else { "▸" }
        } else {
            ""
        };
        let label = if marker.is_empty() {
            row.name.clone()
        } else {
            format!("{marker} {}", row.name)
        };
        scene.push_text(
            label,
            Rect::new(
                indent,
                row.rect.origin.y + 3.0,
                (panel.origin.x + panel.size.width - indent - 8.0).max(1.0),
                18.0,
            ),
            TextStyle {
                clip: viewport,
                color: if row.directory {
                    Color::rgb(0.76, 0.80, 0.88)
                } else {
                    Color::rgb(0.60, 0.64, 0.72)
                },
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    }
}

/// Draws the question the window is asking.
///
/// The question is drawn over everything else, because a question that can be
/// missed is not a question.
fn push_prompt(scene: &mut FrameScene, viewport: Rect, size: Size, text: &str) {
    let width = (size.width * 0.6).clamp(320.0, 720.0);
    let height = 132.0;
    let box_rect = Rect::new(
        (size.width - width) / 2.0,
        (size.height - height) / 2.0,
        width,
        height,
    );
    // The window behind the question is dimmed, so the question is the thing
    // the eye goes to.
    scene.push_quad(viewport, Color::rgb(0.02, 0.03, 0.05), viewport);
    scene.push_quad(box_rect, Color::rgb(0.13, 0.15, 0.20), viewport);
    scene.push_quad(
        Rect::new(
            box_rect.origin.x,
            box_rect.origin.y,
            box_rect.size.width,
            3.0,
        ),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );
    scene.push_text(
        text.to_owned(),
        Rect::new(
            box_rect.origin.x + 20.0,
            box_rect.origin.y + 18.0,
            (box_rect.size.width - 40.0).max(1.0),
            44.0,
        ),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.90, 0.92, 0.96),
            font_size: 14.0,
            line_height: 20.0,
            family: TextFamily::SansSerif,
        },
    );
}

/// Draws the settings panel.
///
/// The panel is drawn over everything else, because a setting that cannot be
/// seen is a setting that cannot be changed.
fn push_settings(scene: &mut FrameScene, viewport: Rect, size: Size, settings: &Settings) {
    let rows = settings.rows();
    let width = (size.width * 0.42).clamp(360.0, 560.0);
    let height = 46.0 + rows.len() as f32 * 30.0 + 32.0;
    let box_rect = Rect::new(
        (size.width - width) / 2.0,
        (size.height - height) / 2.0,
        width,
        height,
    );
    // The window behind the panel is dimmed, so the panel is the thing the
    // eye goes to.
    scene.push_quad(viewport, Color::rgb(0.02, 0.03, 0.05), viewport);
    scene.push_quad(box_rect, Color::rgb(0.13, 0.15, 0.20), viewport);
    scene.push_quad(
        Rect::new(
            box_rect.origin.x,
            box_rect.origin.y,
            box_rect.size.width,
            3.0,
        ),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );
    scene.push_text(
        settings.title().to_owned(),
        Rect::new(
            box_rect.origin.x + 20.0,
            box_rect.origin.y + 14.0,
            (box_rect.size.width - 40.0).max(1.0),
            24.0,
        ),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.90, 0.92, 0.96),
            font_size: 16.0,
            line_height: 22.0,
            family: TextFamily::SansSerif,
        },
    );

    // One line per setting: the name on the left, the value on the right.
    let mut y = box_rect.origin.y + 48.0;
    for row in &rows {
        let color = if row.selected {
            Color::rgb(0.95, 0.97, 1.00)
        } else {
            Color::rgb(0.60, 0.65, 0.74)
        };
        let title = if row.selected {
            format!("› {}", row.title)
        } else {
            format!("  {}", row.title)
        };
        scene.push_text(
            title,
            Rect::new(
                box_rect.origin.x + 20.0,
                y,
                (box_rect.size.width * 0.55).max(1.0),
                26.0,
            ),
            TextStyle {
                clip: viewport,
                color,
                font_size: 14.0,
                line_height: 20.0,
                family: TextFamily::SansSerif,
            },
        );
        scene.push_text(
            row.value.clone(),
            Rect::new(
                box_rect.origin.x + box_rect.size.width * 0.58,
                y,
                (box_rect.size.width * 0.38).max(1.0),
                26.0,
            ),
            TextStyle {
                clip: viewport,
                color,
                font_size: 14.0,
                line_height: 20.0,
                family: TextFamily::SansSerif,
            },
        );
        y += 30.0;
    }

    // The keys the panel takes are written under it.
    scene.push_text(
        settings.hint().to_owned(),
        Rect::new(
            box_rect.origin.x + 20.0,
            y + 2.0,
            (box_rect.size.width - 40.0).max(1.0),
            24.0,
        ),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.52, 0.57, 0.66),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );
}

/// The place the find bar in the corner of the editor body.
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
    conflict: Option<&ConflictView>,
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
            let mut body = String::new();

            // The context the user may pick from. It comes first: what the
            // agent gets to see is the user's decision, and it is made
            // before the turn starts.
            if !panel.context.is_empty() {
                body.push_str("上下文\n");
                for (index, item) in panel.context.iter().enumerate() {
                    body.push_str(&format!(
                        "\n{} {}（{} 字节）",
                        if item.selected { "[x]" } else { "[ ]" },
                        item.label(),
                        item.bytes()
                    ));
                    let _ = index;
                }
                body.push('\n');
            }

            // A change the agent proposes, block by block.
            for proposal in &panel.pending_diffs {
                body.push_str(&format!(
                    "\n变更 {path} · 批准 {approved} / 拒绝 {rejected}\n",
                    path = proposal.path,
                    approved = proposal.approved_count(),
                    rejected = proposal.rejected_count(),
                ));
                for (index, hunk) in proposal.hunks.iter().enumerate() {
                    let mark = match hunk.decision {
                        HunkDecision::Approved => "+",
                        HunkDecision::Rejected => "-",
                        HunkDecision::Pending => " ",
                    };
                    body.push_str(&format!(
                        "\n{mark} 第 {line} 行 {summary}",
                        line = hunk.line,
                        summary = hunk.summary(),
                    ));
                    let _ = index;
                }
            }

            if !panel.transcript.is_empty() {
                body.push('\n');
                body.push_str(&panel.transcript);
            }
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
            if let Some(conflict) = conflict {
                body.push_str(&format!(
                    "\n冲突 {} 处 · 第 {} 处待选（Ctrl+Alt+K 保留我的 / Ctrl+Alt+J 用 Agent 的）",
                    conflict.total,
                    conflict.selected + 1
                ));
                for item in &conflict.items {
                    body.push_str(&format!("\n  我的：{}", item.ours));
                    body.push_str(&format!("\n  Agent：{}", item.theirs));
                }
            }
            if !panel.trace.is_empty() {
                body.push_str("\n轨迹\n");
                for entry in &panel.trace {
                    body.push_str(&format!("\n[{}] {}", entry.kind.label(), entry.detail));
                }
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
        assert_eq!(app.editor().text(), "line one\nline two\n");
        assert_eq!(app.editor().caret().0, 0);
    }

    #[test]
    fn saving_writes_the_buffer_back_to_the_file() {
        let path = scratch("save.txt");
        fs::write(&path, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        app.editor_mut().insert("edited ").unwrap();
        assert!(
            app.status_line().contains('●'),
            "the file must show as changed"
        );

        app.save_tab(active_tab(&app));

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
        app.editor_mut().insert("mine ").unwrap();

        // Something else writes the file after the editor opened it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, "somebody else wrote this\n").unwrap();

        app.save_tab(active_tab(&app));

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "somebody else wrote this\n",
            "the outside change must survive"
        );
        assert!(app.status_line().contains("保存失败"));
    }

    /// The tab that is on screen.
    fn active_tab(app: &NativeApplication) -> TabId {
        app.tabs.active().expect("a tab is open").id
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
        assert_eq!(app.editor().text(), "");
        assert!(app.status_line().contains("untitled"));
    }

    /// An application with a laid out body, for the checks that need one.
    fn app_with_body(text: &str) -> NativeApplication {
        let mut app = NativeApplication::new(None).expect("the application starts");
        let mut measurer = MonospaceMeasurer::new(10.0);
        let pane = app.pane_mut();
        pane.editor.load_text(text);
        pane
            .view
            .layout(pane.editor.buffer(), 1000.0, 200.0, &mut measurer);
        app
    }

    #[test]
    fn the_wheel_turned_towards_the_user_scrolls_down() {
        let mut app = app_with_body("one\ntwo\nthree\nfour\n");
        // The wheel reports a negative delta when it is turned towards the
        // user.
        app.scroll_body_by_wheel(-3.0);
        assert_eq!(app.view().view.first_line, 3);
        app.scroll_body_by_wheel(2.0);
        assert_eq!(app.view().view.first_line, 1);
    }

    #[test]
    fn the_body_never_scrolls_past_the_end_of_the_document() {
        let mut app = app_with_body("one\ntwo\n");
        app.scroll_body_by_wheel(-100.0);
        // The document has three lines, the last one empty.
        assert_eq!(app.view().view.first_line, 2);
    }

    #[test]
    fn the_caret_moves_a_visual_line_at_a_time() {
        let mut app = app_with_body("first\nsecond\n");
        app.editor_mut().select_range(TextOffset(3), TextOffset(3));
        app.move_caret_vertically(1.0, 1);
        // The caret keeps its column: three characters into the next line.
        assert_eq!(app.editor().caret().0, 9);
        app.move_caret_vertically(1.0, -1);
        assert_eq!(app.editor().caret().0, 3);
    }

    #[test]
    fn the_caret_stays_on_screen_when_it_leaves_the_view() {
        let mut app = app_with_body(&"line\n".repeat(20));
        app.editor_mut().select_range(TextOffset(99), TextOffset(99));
        app.keep_caret_visible(1.0);
        assert!(
            app.view().view.first_line > 0,
            "the view must follow the caret"
        );
    }

    #[test]
    fn adding_a_caret_below_gives_every_caret_a_neighbour() {
        let mut app = app_with_body("one\ntwo\nthree\n");
        let pane = app.pane_mut();
        pane.editor.select_range(TextOffset(1), TextOffset(1));
        assert!(pane.view.add_cursor_vertically(&mut pane.editor, 1));
        assert_eq!(pane.editor.cursor_count(), 2);
        // The new caret keeps the column: the second character of the line
        // below.
        assert_eq!(pane.editor.carets(), vec![TextOffset(1), TextOffset(5)]);
    }

    #[test]
    fn adding_a_caret_at_the_edge_of_the_document_does_nothing() {
        let mut app = app_with_body("one\ntwo\n");
        // The last line of the document has no line below it.
        let pane = app.pane_mut();
        pane.editor.select_range(TextOffset(8), TextOffset(8));
        assert!(!pane.view.add_cursor_vertically(&mut pane.editor, 1));
        assert_eq!(pane.editor.cursor_count(), 1);
    }

    #[test]
    fn typing_writes_at_every_caret() {
        let mut app = app_with_body("one\ntwo\n");
        let pane = app.pane_mut();
        pane.editor.select_range(TextOffset(1), TextOffset(1));
        pane.view.add_cursor_vertically(&mut pane.editor, 1);
        pane.editor.insert("X").unwrap();
        assert_eq!(pane.editor.text(), "oXne\ntXwo\n");
    }

    #[test]
    fn holding_shift_extends_the_selection_to_the_line_below() {
        let mut app = app_with_body("one\ntwo\n");
        let pane = app.pane_mut();
        pane.editor.select_range(TextOffset(1), TextOffset(1));
        assert!(pane.view.extend_vertical(&mut pane.editor, 1));
        assert_eq!(
            pane.editor.selection().range(pane.editor.buffer()),
            TextOffset(1)..TextOffset(5)
        );
    }

    /// An application with the find bar open.
    fn app_with_find(text: &str) -> NativeApplication {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.editor_mut().load_text(text);
        app.modifiers = ModifiersState::CONTROL;
        app.handle_find_key(&character("f"));
        app.modifiers = ModifiersState::default();
        app
    }

    /// A key that carries a character.
    fn character(text: &str) -> Key<&'_ str> {
        Key::Character(text)
    }

    /// A key that the window could not name, with the code of the key.
    fn unnamed(code: u16) -> Key<&'static str> {
        Key::Unidentified(NativeKey::Windows(code))
    }

    #[test]
    fn a_key_that_carries_a_character_is_read_from_it() {
        assert_eq!(key_letter(&character("a")), Some('a'));
        assert_eq!(key_letter(&character("1")), Some('1'));
        assert_eq!(key_letter(&named(NamedKey::Enter)), None);
    }

    #[test]
    fn a_key_the_window_could_not_name_is_read_from_its_code() {
        // A machine with a Chinese input method does not name the keys that
        // are pressed with Ctrl and Alt. The shortcuts have to work there too.
        assert_eq!(key_letter(&unnamed(0x41)), Some('A'));
        assert_eq!(key_letter(&unnamed(0x31)), Some('1'));
        assert_eq!(key_letter(&unnamed(0x0d)), None);
    }

    /// A key that has a name, like Enter or Escape.
    fn named(key: NamedKey) -> Key<&'static str> {
        Key::Named(key)
    }

    #[test]
    fn the_find_key_opens_the_find_bar() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL;        assert!(app.handle_find_key(&character("f")));
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
        assert_eq!(app.editor().text(), "one two one");
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
        assert_eq!(app.editor().cursor_count(), 3);
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

    /// A directory of scratch files for the checks that need more than one.
    fn scratch_dir(name: &str) -> PathBuf {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch").join(name);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory
    }

    #[test]
    fn opening_a_second_file_gives_a_second_tab() {
        let directory = scratch_dir("tabs");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();

        let mut app = NativeApplication::new(Some(first.clone())).expect("the application starts");
        assert_eq!(app.tabs.len(), 1);

        assert!(app.open_file(&second));

        assert_eq!(app.tabs.len(), 2, "the second file has its own tab");
        assert_eq!(app.panes.len(), 2, "every tab has its own buffer");
        assert_eq!(app.active, 1, "the file that was opened is in front");
        assert_eq!(app.editor().text(), "two\n");
        assert_eq!(
            app.tabs.tabs()[0].title,
            "first.txt",
            "the tab shows the name of the file"
        );
    }

    #[test]
    fn a_file_that_is_already_open_comes_to_the_front_instead() {
        let directory = scratch_dir("tabs-same");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();

        let mut app = NativeApplication::new(Some(first.clone())).expect("the application starts");
        app.open_file(&second);
        assert_eq!(app.active, 1);

        // The first file is opened again.
        assert!(app.open_file(&first));

        assert_eq!(app.tabs.len(), 2, "the file is not opened twice");
        assert_eq!(app.panes.len(), 2, "and it does not get a second buffer");
        assert_eq!(app.active, 0, "the tab that was there comes to the front");
    }

    #[test]
    fn closing_a_tab_brings_another_one_to_the_front() {
        let directory = scratch_dir("tabs-close");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();

        let mut app = NativeApplication::new(Some(first.clone())).expect("the application starts");
        app.open_file(&second);

        app.close_tab(1);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.panes.len(), 1);
        assert_eq!(app.active, 0);
        assert_eq!(app.editor().text(), "one\n");
    }

    #[test]
    fn closing_the_last_tab_leaves_an_empty_editor() {
        let directory = scratch_dir("tabs-last");
        let only = directory.join("only.txt");
        fs::write(&only, "one\n").unwrap();

        let mut app = NativeApplication::new(Some(only)).expect("the application starts");
        app.close_tab(0);

        assert_eq!(app.tabs.len(), 1, "there is always a tab to type into");
        assert_eq!(app.panes.len(), 1);
        assert_eq!(app.editor().text(), "");
    }

    #[test]
    fn an_edit_marks_the_tab_as_changed() {
        let directory = scratch_dir("dirty");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        app.editor_mut().insert("edited ").unwrap();
        app.sync_dirty_flags();

        assert!(app.tabs.tabs()[0].dirty, "the tab has changes that are not on disk");

        app.save_tab(active_tab(&app));
        app.sync_dirty_flags();

        assert!(!app.tabs.tabs()[0].dirty, "a saved tab has no changes");
    }

    #[test]
    fn only_the_edited_tab_is_marked_as_changed() {
        let directory = scratch_dir("dirty-one");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();

        let mut app = NativeApplication::new(Some(first.clone())).expect("the application starts");
        app.open_file(&second);
        app.editor_mut().insert("edited ").unwrap();
        app.sync_dirty_flags();

        assert!(!app.tabs.tabs()[0].dirty, "the other tab is untouched");
        assert!(app.tabs.tabs()[1].dirty, "the tab in front is changed");
    }

    #[test]
    fn closing_a_tab_with_unsaved_changes_asks_first() {
        let directory = scratch_dir("close-ask");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        app.editor_mut().insert("edited ").unwrap();
        app.sync_dirty_flags();

        app.request_close(0);

        assert_eq!(
            app.prompt,
            Some(Prompt::CloseDirty {
                tab: active_tab(&app)
            }),
            "the window has to ask before the work is thrown away"
        );
        assert_eq!(app.tabs.len(), 1, "the tab is still there");
    }

    #[test]
    fn a_tab_without_changes_is_closed_without_a_question() {
        let directory = scratch_dir("close-quiet");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.request_close(0);

        assert!(app.prompt.is_none(), "nothing to ask about");
        assert_eq!(app.tabs.len(), 1, "an empty editor takes its place");
    }

    #[test]
    fn throwing_the_changes_away_closes_the_tab_and_leaves_the_file_alone() {
        let directory = scratch_dir("close-discard");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        app.editor_mut().insert("edited ").unwrap();
        app.sync_dirty_flags();
        app.request_close(0);

        assert!(app.handle_prompt_key(&character("d")));

        assert!(app.prompt.is_none());
        assert_eq!(app.tabs.len(), 1, "the tab was closed");
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "original\n",
            "the file on disk is untouched"
        );
    }

    #[test]
    fn saving_the_changes_closes_the_tab_and_writes_the_file() {
        let directory = scratch_dir("close-save");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        app.editor_mut().insert("edited ").unwrap();
        app.sync_dirty_flags();
        app.request_close(0);

        assert!(app.handle_prompt_key(&character("s")));

        assert!(app.prompt.is_none());
        assert_eq!(app.tabs.len(), 1, "the tab was closed");
        assert_eq!(fs::read_to_string(&file).unwrap(), "edited original\n");
    }

    #[test]
    fn a_file_that_changed_on_disk_is_asked_about() {
        let directory = scratch_dir("changed-on-disk");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");

        // Somebody else writes the file. The text is longer, so the change
        // cannot be hidden by the clock's resolution.
        fs::write(&file, "somebody else wrote this\n").unwrap();

        assert!(app.poll_disk(), "the change is noticed");
        assert_eq!(
            app.prompt,
            Some(Prompt::ChangedOnDisk {
                tab: active_tab(&app),
                removed: false
            })
        );
    }

    #[test]
    fn a_file_that_is_asked_about_is_not_asked_about_twice() {
        let directory = scratch_dir("changed-once");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        fs::write(&file, "somebody else wrote this\n").unwrap();

        assert!(app.poll_disk());
        assert!(!app.poll_disk(), "the change was already reported");
    }

    #[test]
    fn reloading_throws_the_changes_in_the_buffer_away() {
        let directory = scratch_dir("changed-reload");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        app.editor_mut().insert("mine ").unwrap();
        fs::write(&file, "somebody else wrote this\n").unwrap();
        app.poll_disk();

        assert!(app.handle_prompt_key(&character("r")));

        assert_eq!(
            app.editor().text(),
            "somebody else wrote this\n",
            "the file on disk is what the buffer holds now"
        );
    }

    #[test]
    fn saving_over_a_change_writes_the_buffer_to_the_file() {
        let directory = scratch_dir("changed-save");
        let file = directory.join("file.txt");
        fs::write(&file, "original\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        app.editor_mut().insert("mine ").unwrap();
        fs::write(&file, "somebody else wrote this\n").unwrap();
        app.poll_disk();

        assert!(app.handle_prompt_key(&character("s")));

        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "mine original\n",
            "the user was asked and said so"
        );
    }

    #[test]
    fn the_file_tree_shows_the_folder_of_the_open_file() {
        let directory = scratch_dir("tree");
        let file = directory.join("file.txt");
        fs::write(&file, "one\n").unwrap();

        let app = NativeApplication::new(Some(file)).expect("the application starts");
        let tree = app.tree.as_ref().expect("the tree of files is there");
        assert_eq!(tree.root(), directory.as_path());
    }

    #[test]
    fn the_file_tree_lists_the_files_of_the_folder() {
        let directory = scratch_dir("tree-files");
        fs::write(directory.join("one.txt"), "one\n").unwrap();
        fs::write(directory.join("two.txt"), "two\n").unwrap();
        let file = directory.join("one.txt");

        let app = NativeApplication::new(Some(file)).expect("the application starts");
        let tree = app.tree.as_ref().expect("the tree of files is there");
        let names: Vec<&str> = tree.nodes().iter().map(|node| node.name.as_str()).collect();
        assert!(
            names.contains(&"one.txt") && names.contains(&"two.txt"),
            "the tree shows the files of the folder, got {names:?}"
        );
    }

    /// An application whose body is laid out, so the highlighter has a range
    /// of the text to work on.
    fn app_with_layout(app: &mut NativeApplication) {
        let mut measurer = MonospaceMeasurer::new(10.0);
        let pane = app.pane_mut();
        pane.view
            .layout(pane.editor.buffer(), 1000.0, 200.0, &mut measurer);
    }

    /// Waits for the highlighter to answer.
    ///
    /// The highlighter runs on a thread of its own, so the answer is not there
    /// the moment it is asked for. The wait is bounded: a highlighter that
    /// never answers fails the test instead of hanging it.
    fn wait_for_syntax(app: &mut NativeApplication) -> bool {
        for _ in 0..400 {
            if app.pump_syntax() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_file_within_the_limit_is_highlighted() {
        let directory = scratch_dir("highlighted");
        let file = directory.join("file.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app_with_layout(&mut app);

        assert!(wait_for_syntax(&mut app), "the highlighter answers");
        assert!(!app.panes[app.active].degraded, "the file is small enough");
        assert!(
            !app.panes[app.active].highlights.is_empty(),
            "the text is coloured"
        );
    }

    #[test]
    fn a_file_over_the_limit_is_not_parsed_at_all() {
        let directory = scratch_dir("degraded");
        let file = directory.join("file.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app_with_layout(&mut app);
        app.pane_mut().set_policy(HighlightPolicy {
            max_bytes: 1,
            max_lines: 1,
        });

        assert!(
            app.pump_syntax(),
            "the window has to say that the file is too big"
        );
        assert!(app.panes[app.active].degraded, "the file is not coloured");
        assert!(app.panes[app.active].highlights.is_empty());
        assert!(
            app.status_line().contains("降级模式"),
            "the status line says why, got {}",
            app.status_line()
        );
    }

    #[test]
    fn a_degraded_file_can_still_be_edited_and_saved() {
        let directory = scratch_dir("degraded-editable");
        let file = directory.join("file.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        app_with_layout(&mut app);
        app.pane_mut().set_policy(HighlightPolicy {
            max_bytes: 1,
            max_lines: 1,
        });
        app.pump_syntax();

        app.editor_mut().insert("// edited\n").unwrap();
        app.save_tab(active_tab(&app));

        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "// edited\nfn main() {}\n",
            "a file without colours is still a file"
        );
    }

    #[test]
    fn the_formatting_answer_goes_into_the_buffer() {
        let path = scratch("format.rs");
        fs::write(&path, "fn main( ){}\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        let before = app.editor().version();
        let edits = vec![TextEdit {
            range: codeweaver_lsp::protocol::Range {
                start: codeweaver_lsp::protocol::Position::new(0, 8),
                end: codeweaver_lsp::protocol::Position::new(0, 11),
            },
            new_text: ") {".to_owned(),
        }];

        assert!(app.apply_formatting(edits, before), "the text changed");
        assert_eq!(app.editor().text(), "fn main() {}\n");
        assert_eq!(
            app.editor().version(),
            before + 1,
            "the edits are one change of the document"
        );
        assert!(
            app.status_line().contains('●'),
            "the file has to be marked as changed, got {}",
            app.status_line()
        );
    }

    #[test]
    fn a_formatting_answer_for_another_text_is_not_put_in() {
        let path = scratch("format-stale.rs");
        fs::write(&path, "fn main( ){}\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        let before = app.editor().version();
        let edits = vec![TextEdit {
            range: codeweaver_lsp::protocol::Range {
                start: codeweaver_lsp::protocol::Position::new(0, 8),
                end: codeweaver_lsp::protocol::Position::new(0, 11),
            },
            new_text: ") {".to_owned(),
        }];

        // The answer is about a text that is not the text that is there.
        assert!(!app.apply_formatting(edits, before + 1));
        assert_eq!(app.editor().text(), "fn main( ){}\n", "the text is untouched");
    }

    #[test]
    fn a_second_format_request_is_not_sent_while_one_is_on_its_way() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.formatting = Some(app.editor().version());

        app.request_format();
        assert!(
            app.status.is_none(),
            "the editor must not ask the server twice for one press of the key"
        );
    }

    #[test]
    fn the_format_key_says_when_there_is_no_server() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::ALT | ModifiersState::SHIFT;

        assert!(app.handle_format_key(&character("f")));
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("没有语言服务器"),
            "the editor has to say why it cannot format"
        );
    }

    #[test]
    fn the_format_key_is_only_the_key_with_alt_and_shift() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::ALT;
        assert!(!app.handle_format_key(&character("f")));
        app.modifiers = ModifiersState::SHIFT;
        assert!(!app.handle_format_key(&character("f")));
    }

    #[test]
    fn clicking_a_tab_brings_it_to_the_front() {
        let directory = scratch_dir("tabs-click");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();

        let mut app = NativeApplication::new(Some(first.clone())).expect("the application starts");
        app.open_file(&second);
        assert_eq!(app.active, 1);

        // The strip is laid out and hit the same way as it is drawn.
        let size = Size::new(1200.0, 800.0);
        let mut measurer = MonospaceMeasurer::new(13.0);
        let strip = tab_strip(app.tabs.tabs(), app.active, size, &mut measurer);
        let first_tab = strip.items[0].rect;
        let point = Point {
            x: first_tab.origin.x + 4.0,
            y: first_tab.origin.y + 4.0,
        };

        app.activate_tab_at(point, &strip);

        assert_eq!(app.active, 0, "the tab that was clicked is in front");
    }

    #[test]
    fn the_settings_panel_starts_on_the_values_in_force() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.set_highlight_policy(HighlightPolicy {
            max_bytes: 500_000,
            max_lines: 20_000,
        });

        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_settings_key(&character(",")));

        let settings = app.settings.as_ref().expect("the panel is open");
        assert_eq!(
            settings.policy().max_bytes,
            500_000,
            "the panel shows the limit that is in force"
        );
        assert_eq!(settings.policy().max_lines, 20_000);
    }

    #[test]
    fn the_settings_panel_changes_the_limit_at_once() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_settings_key(&character(",")));

        app.modifiers = ModifiersState::default();
        assert!(app.handle_settings_key(&named(NamedKey::ArrowRight)));

        assert_eq!(
            app.panes[app.active].policy.max_bytes,
            2_000_000,
            "the new limit is in force at once"
        );

        // The panel closes on Enter.
        assert!(app.handle_settings_key(&named(NamedKey::Enter)));
        assert!(app.settings.is_none(), "the panel is closed");
    }

    #[test]
    fn the_settings_panel_takes_every_key_while_it_is_open() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_settings_key(&character(",")));

        // A key that is not one of the panel's is swallowed by the panel.
        assert!(app.handle_settings_key(&character("x")));
        assert_eq!(app.editor().text(), "", "the text is not touched");
        assert!(app.settings.is_some(), "the panel stays open");
    }

    #[test]
    fn raising_the_limit_brings_the_colours_back() {
        let directory = scratch_dir("raised");
        let file = directory.join("file.rs");
        fs::write(&file, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app_with_layout(&mut app);
        app.pane_mut().set_policy(HighlightPolicy {
            max_bytes: 1,
            max_lines: 1,
        });
        app.pump_syntax();
        assert!(app.panes[app.active].degraded, "the file is too big");

        // The limit is raised again.
        app.set_highlight_policy(HighlightPolicy::default());

        assert!(wait_for_syntax(&mut app), "the highlighter answers again");
        assert!(
            !app.panes[app.active].degraded,
            "the file is small enough to be coloured again"
        );
        assert!(
            !app.panes[app.active].highlights.is_empty(),
            "the text is coloured again"
        );
    }
}
