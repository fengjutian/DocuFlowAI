mod activity_bar;
mod agent;
mod breadcrumbs;
mod code_action;
mod command;
mod command_palette;
mod commands;
mod completion;
mod context;
pub mod debug;
mod developer;
mod devlog;
mod diff_review;
pub mod editor_view;
pub mod file_icons;
pub mod find;
mod git;
mod groups;
mod gutter;
mod icons;
mod input;
mod keybindings;
mod layout;
mod lsp;
mod minimap;
mod multicursor;
mod pane;
mod paths;
mod rename;
mod search;
mod settings;
mod shell;
mod sidebar;
mod status_bar;
mod task;
#[cfg(test)]
mod testing;
pub mod workspace_file;
pub mod zoom;

use std::{ops::Range, path::{Path, PathBuf}, sync::Arc, time::{Duration, Instant}};

use anyhow::{Context, Result};
use codeweaver_agent_acp::ToolStatus;
use codeweaver_agent_ui::{ContextItem, HunkDecision};
use codeweaver_dap::AdapterConfig;
use codeweaver_editor_core::Editor;
use codeweaver_lsp::{
    Client as LspClient, LspEvent, ServerConfig,
    protocol::{TextEdit, WorkspaceEdit},
};
use codeweaver_render::{
    Color, FrameScene, Point, Rect, Renderer, RendererError, Size, TextFamily, TextStyle,
};
use codeweaver_syntax::Language;
use codeweaver_text::{Bias, TextOffset};
use codeweaver_ui::TextMeasurer;
use codeweaver_workspace::{
    ChangeDetector, Document, ExternalChange, FileForest, SearchHit, Workspace,
};
use codeweaver_workbench::{close_decision, CloseDecision, Tab, TabBar, TabId};
use code_action::ActionPopup;
use command::CommandBox;
use command_palette::{CommandPalette, PaletteCommand};
use commands::{CommandRegistry, Keybinding, Context as CommandContext, When};
use completion::CompletionPopup;
use debug::{BreakpointMark, Breakpoints, DebugPanel, DebugState, DebugStep, Debugger};
use developer::{DeveloperCommand, DeveloperPanel, Line as DeveloperLine};
use devlog::DevLog;
use editor_view::{
    BodyOverlay, EditorView, FindOverlay, MarkSeverity, RendererMeasurer,
};
use activity_bar::{Activity, Badges, SideBar};
use find::{FindBar, FindField};
use git::{GitAsking, GitBox, GitPanel, GitStatus, GitView};
use groups::Groups;
use input::TextInputState;
use layout::{ColumnLayout, WindowLayout, SEPARATOR};
use pane::EditorPane;
use rename::RenameBox;
use search::{SearchCommand, SearchPanel};
use settings::{KeyRow, Kind as SettingKind, Settings};
use sidebar::{Action as SideBarAction, Row as SideBarRow};
use status_bar::StatusBar;
use task::{Task, TaskOutput, TaskPanel, TaskState};
use zoom::Zoom;
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

/// The height of the output panel of a task.
const TASK_PANEL_HEIGHT: f32 = 200.0;

/// The changes of one file, as ranges of its text.
type FileChanges = Vec<(std::ops::Range<TextOffset>, String)>;

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
    /// The things the server offers to do about the place of the caret, when
    /// the list is open.
    actions: Option<ActionPopup>,
    /// The file the code operations were asked about, and the version of its
    /// text when they were asked.
    ///
    /// The list may be picked long after the question was asked, and the tab
    /// on screen may be another one by then: the operations belong to the
    /// file that was asked about.
    action_target: Option<(String, u64)>,
    /// The task that is running, when there is one.
    task: Option<Task>,
    /// What the task has said.
    task_output: TaskOutput,
    /// What the task was asked to do.
    task_title: String,
    /// Whether the output panel of the task is open.
    task_panel: bool,
    /// The panel of the changes of the repository, when it is open.
    git: Option<GitPanel>,
    /// The box that asks git something, when it is asking.
    git_box: Option<GitBox>,
    /// The branch the repository of the open file is on.
    ///
    /// Git is not asked every frame: the branch is read when the file
    /// changes and when the panel of the changes is opened.
    current_branch: Option<String>,
    /// What git knows about the files of the project.
    ///
    /// It is asked for once and then kept, so that the badge on the activity
    /// bar can say how many files changed without asking git every frame.
    source_control: Option<GitStatus>,
    /// The box that asks for the message of a commit, when it is open.
    commit_box: Option<RenameBox>,
    /// The debug adapter, when one is named.
    debug_adapter: Option<AdapterConfig>,
    /// The debug session, when there is one.
    debug: Option<Debugger>,
    /// The breakpoints of the editor.
    ///
    /// They belong to the files and not to the session: they are there
    /// whether a session is running or not.
    breakpoints: Breakpoints,
    /// The box that asks for the condition of a breakpoint, when it is open.
    breakpoint_condition: Option<CommandBox>,
    /// The box that asks for an expression to be worked out, when it is open.
    debug_expression: Option<CommandBox>,
    /// The frame of the stack that was clicked on last, and when.
    ///
    /// Two clicks in a row on the same frame are a double click: they mean
    /// "take me to that place in the code".
    last_frame_click: Option<(usize, Instant)>,
    /// The line the box that asks for a condition is about.
    breakpoint_condition_line: Option<u32>,
    /// The tabs that are open.
    tabs: TabBar,
    /// The pane of every tab, in the order of the tabs.
    panes: Vec<EditorPane>,
    /// The columns of the window, and the tabs in them.
    ///
    /// A window with one column shows one group of tabs. A window that is
    /// split shows two, and each of them has its own tabs and its own tab in
    /// front. The columns are the window: which column has the keyboard is
    /// asked of them, not remembered apart.
    groups: Groups,
    /// The files of the workspace, as a tree.
    ///
    /// A project of one folder has one tree; a workspace that names several
    /// folders has one tree for each of them.
    tree: Option<FileForest>,
    /// The workspace that is open, when there is one.
    ///
    /// A workspace is a file that names the folders of a project. While one is
    /// open, the file tree is its tree and not the folder of the file that
    /// happens to be open.
    open_workspace: Option<workspace_file::Workspace>,
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
    /// The box that asks for the new name, when it is open.
    rename_box: Option<RenameBox>,
    /// The box that asks for a command, when it is open.
    command_box: Option<CommandBox>,
    /// The commands of the editor.
    ///
    /// They are registered once: everything that asks "which command does
    /// this key run?" or "what can be done here?" asks them.
    commands: CommandRegistry,
    /// The panel that lists the commands, when it is open.
    palette: Option<CommandPalette>,
    /// Whether the editor was started in development mode.
    ///
    /// In development mode the editor writes what it says about itself into
    /// a file, so that a run nobody watched can still be read afterwards.
    dev: bool,
    /// The log of this run, when the editor is in development mode.
    dev_log: Option<DevLog>,
    /// The panel that shows what the editor is doing inside, when it is open.
    developer: Option<DeveloperPanel>,
    /// The status that was written to the log, so that it is written once.
    logged_status: Option<String>,
    /// The commands that were run before.
    ///
    /// They are kept in the order they were run, the newest last.
    command_history: Vec<String>,
    /// The line that is being typed into the terminal, when the terminal is
    /// open.
    ///
    /// It is `None` when there is no terminal.
    terminal_input: Option<CommandBox>,
    /// Whether the keyboard is in the terminal.
    ///
    /// The terminal keeps running when the keyboard leaves it: `Esc` gives
    /// the keyboard back to the document, and the shell goes on.
    terminal_has_focus: bool,
    /// The commands that were typed into the terminal.
    terminal_history: Vec<String>,
    /// The panel that finds text in the files of the project, when it is open.
    search: Option<SearchPanel>,
    /// The text that was in the box the last time.
    ///
    /// It is kept so that the person who searches twice does not have to
    /// type the same word twice.
    search_needle: String,
    /// The places the last search found.
    ///
    /// They are kept after the panel of the search is closed: the section of
    /// the side bar shows them, and the badge on the activity bar counts
    /// them.
    search_hits: Vec<SearchHit>,
    /// The rename request that is on its way.
    ///
    /// The request is about one place in one document, and it is only good
    /// for the text that was there when it was made.
    renaming: Option<RenameRequest>,
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
    /// The section the side bar shows, and whether it is shown at all.
    ///
    /// The section is a state: switching away and back again leaves the
    /// content of the section as it was.
    sidebar: SideBar,
    /// How big the text of the document is drawn.
    ///
    /// The size belongs to the window, not to a file: every file is drawn at
    /// the same size, and the size the person chose is remembered.
    zoom: Zoom,
    /// Where the size is remembered.
    ///
    /// It is the file of the user, next to the file of the keys. A test may
    /// point it somewhere else: a test that changes the size must not write
    /// into the settings of the person who runs the tests.
    zoom_path: Option<PathBuf>,
    /// Whether the keyboard is on the activity bar.
    ///
    /// The keyboard is on one thing at a time: the terminal has it, the find
    /// bar has it, and when the person says so, the activity bar has it.
    activity_focus: bool,
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

/// A rename request that is on its way.
struct RenameRequest {
    /// The file the request is about.
    ///
    /// The renaming is about this file and not about the tab that happens
    /// to be on screen when the answer comes back.
    uri: String,
    /// The version of the text the request was made for.
    version: u64,
    /// The name the user asked for.
    new_name: String,
}

/// What came of a change that was put into the files.
struct EditOutcome {
    /// Whether anything changed.
    changed: bool,
    /// How many files were changed.
    files: usize,
    /// How many of them were not open in the editor and were written to disk.
    written: usize,
    /// Why nothing was changed, when nothing was.
    reason: Option<String>,
}

impl EditOutcome {
    /// Nothing was changed, and this is why.
    fn failed(reason: String) -> Self {
        EditOutcome {
            changed: false,
            files: 0,
            written: 0,
            reason: Some(reason),
        }
    }
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

/// Where the log of a development run is written.
///
/// The log belongs to the folder the editor was started in: that is the
/// folder of the project when the editor is started from it, and the log of
/// a run is then next to the code that run was about.
pub fn dev_log_path() -> PathBuf {
    let folder = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    folder.join(".dev-logs").join("editor.log")
}

impl NativeApplication {
    /// Creates the application, opening `path` when one is given.
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        Self::build(path, None, None)
    }

    /// Creates the application with a language server of the caller's
    /// choosing.
    ///
    /// Without this, the server of the language of the file is used: the
    /// editor knows `rust-analyzer` and `gopls`, but nothing else.
    pub fn with_language_server(path: Option<PathBuf>, server: ServerConfig) -> Result<Self> {
        Self::build(path, Some(server), None)
    }

    /// Creates the application in development mode.
    ///
    /// In development mode the editor writes what it says about itself into
    /// a file, and the developer panel shows that file, the commands of the
    /// editor and the state of the language server.
    ///
    /// The log belongs to this run: it is emptied when the editor starts.
    pub fn in_development_mode(path: Option<PathBuf>, log: PathBuf) -> Result<Self> {
        Self::build(path, None, Some(log))
    }

    /// Creates the application in development mode, with a language server of
    /// the caller's choosing.
    pub fn in_development_mode_with_language_server(
        path: Option<PathBuf>,
        server: ServerConfig,
        log: PathBuf,
    ) -> Result<Self> {
        Self::build(path, Some(server), Some(log))
    }

    /// Creates the application, with the language server to use when it is
    /// given.
    fn build(
        path: Option<PathBuf>,
        server_config: Option<ServerConfig>,
        dev_log: Option<PathBuf>,
    ) -> Result<Self> {
        // The log is opened before anything else happens, so that what
        // happens while the editor starts is in it too.
        let dev_log = dev_log.map(DevLog::open);
        let dev = dev_log.is_some();
        let workspace = Workspace::detached();
        // The keys a person changed live in a file next to the project. The
        // root of the project is the folder of the file that is opened, which
        // is the same folder the tree of files shows.
        let project_root = path
            .as_ref()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf);
        let mut tabs = TabBar::new();
        let mut panes: Vec<EditorPane> = Vec::new();
        let mut server = None;
        let mut detector = ChangeDetector::new();
        let mut tree = None;
        // A workspace file is not a text file: it is opened as a workspace, and
        // that happens once the window is there, so that what went wrong can
        // be said out loud.
        let mut workspace_file_to_open = None;

        // The repository the file belongs to is found before the file is
        // opened: the branch it is on is said in the state line from the
        // start, and not only after the panel of the changes was opened.
        let repository_root = path
            .as_ref()
            .and_then(|path| git::repository_root(path));
        match path {
            Some(path) if workspace_file::is_workspace_file(&path) => {
                let tab = tabs.open_scratch("untitled");
                panes.push(EditorPane::scratch(tab, None));
                workspace_file_to_open = Some(path);
            }
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
                tree = path.parent().map(|root| FileForest::single(root.to_path_buf()));
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

        // The window starts with one column, and that column shows the file
        // that is open.
        let mut groups = Groups::new();
        groups.add(0);

        let mut commands = commands::default_registry();
        // The size of the text the person chose is remembered in a file of the
        // user, next to the file of the keys.
        let zoom_path = zoom::user_path();
        let settings = zoom_path.as_deref().map(zoom::read);
        let zoom = settings.as_ref().map(|loaded| loaded.zoom).unwrap_or_default();
        let settings_errors = settings
            .as_ref()
            .map(|loaded| loaded.errors.clone())
            .unwrap_or_default();
        // The keys in the file of the user and the file of the project are
        // put over the ones the editor was built with, before anything is
        // read from the registry: what the editor starts with is what the
        // person asked for.
        let changes = keybindings::load(project_root.as_deref());
        let mut keybinding_errors = changes.errors.clone();
        keybinding_errors.extend(keybindings::apply(&mut commands, &changes.bindings));
        let keybindings_changed = changes.bindings.len();
        // Two commands that want one key is a mistake that is said out loud:
        // the person has to know which one of them the key really runs.
        let conflict = commands
            .conflicts()
            .first()
            .map(|conflict| {
                format!(
                    "命令 {} 和 {} 都要 {} 这个键",
                    conflict.first,
                    conflict.second,
                    conflict.keybinding.text()
                )
            });
        // What the editor could not make of the file is said as well: a file
        // that is half applied is a file the person cannot trust.
        let status = conflict.or_else(|| {
            keybinding_errors.first().map(|first| {
                if keybinding_errors.len() > 1 {
                    format!(
                        "{first}（键位文件里还有 {} 处不对）",
                        keybinding_errors.len() - 1
                    )
                } else {
                    first.clone()
                }
            })
        });
        // The same goes for the file of the settings: a size that cannot be
        // read is said out loud, and the usual size is used.
        let status = status.or_else(|| settings_errors.first().cloned());

        let mut application = NativeApplication {
            window: None,
            renderer: None,
            input: TextInputState::default(),
            modifiers: ModifiersState::default(),
            agent: None,
            agent_error: None,
            workspace,
            status,
            endpoint: None,
            cursor: None,
            find: None,
            server,
            server_generation: 1,
            completion: None,
            actions: None,
            action_target: None,
            task: None,
            task_output: TaskOutput::default(),
            task_title: String::new(),
            task_panel: false,
            git: None,
            git_box: None,
            current_branch: None,
            source_control: None,
            commit_box: None,
            debug_adapter: None,
            debug: None,
            breakpoints: Breakpoints::default(),
            breakpoint_condition: None,
            debug_expression: None,
            last_frame_click: None,
            breakpoint_condition_line: None,
            tabs,
            panes,
            groups,
            tree,
            open_workspace: None,
            detector,
            prompt: None,
            settings: None,
            formatting: None,
            rename_box: None,
            command_box: None,
            commands,
            palette: None,
            dev,
            dev_log,
            developer: None,
            logged_status: None,
            command_history: Vec::new(),
            terminal_input: None,
            terminal_has_focus: false,
            terminal_history: Vec::new(),
            search: None,
            search_needle: String::new(),
            search_hits: Vec::new(),
            renaming: None,
            pending_diff: None,
            selected_hunk: 0,
            selected_context: 0,
            conflicts: None,
            sidebar: SideBar::default(),
            zoom,
            zoom_path,
            activity_focus: false,
        };

        // The path of the file that is on screen is worked out once here and
        // then kept: the root of the project is found by walking up the disk,
        // and the path does not change by itself.
        application.refresh_crumbs_of(0);
        // The branch the program is on is read at the start as well: the
        // state line says it before anything is done in the window.
        if let Some(root) = repository_root {
            application.remember_the_branch(&root);
        }
        // The text is drawn at the size the person chose.
        application.apply_the_zoom();
        // A workspace that was named on the command line is opened now: the
        // window is there, so what went wrong can be said out loud.
        if let Some(path) = workspace_file_to_open {
            application.open_the_workspace(&path);
        }

        // What happened while the editor started is in the log too: a run
        // that failed to start is the run one wants to read about most.
        if application.dev_log.is_some() {
            let opened = match application.panes[front_tab(&application.groups)].document.as_ref() {
                Some(document) => format!("打开了 {}", document.path().display()),
                None => "没有打开文件".to_owned(),
            };
            application.note(&format!("开发模式启动，{opened}"));
            if application.server.is_some() {
                application.note("语言服务器起来了");
            }
            if let Some(conflict) = application.status.clone() {
                application.note(&conflict);
            }
            if keybindings_changed > 0 {
                application.note(&format!(
                    "键位文件：{keybindings_changed} 条改动已生效"
                ));
            }
            for error in &keybinding_errors {
                application.note(error);
            }
        }
        Ok(application)
    }

    /// Writes one line into the log of this run.
    ///
    /// Without the development mode there is no log, and the line goes
    /// nowhere: the editor does not write files behind the person's back.
    fn note(&mut self, line: &str) {
        if let Some(log) = self.dev_log.as_mut() {
            log.write(line);
        }
    }

    /// Writes the status into the log, when the status has changed.
    ///
    /// The status line is where the editor says what it did, so it is the
    /// one thing the log has to have. It is written once per change: the
    /// status is looked at on every frame, and a log that says the same
    /// thing sixty times a second says nothing.
    fn log_the_status_if_it_changed(&mut self) {
        if self.dev_log.is_none() {
            return;
        }
        let Some(status) = self.status.clone() else {
            return;
        };
        if self.logged_status.as_deref() == Some(status.as_str()) {
            return;
        }
        self.logged_status = Some(status.clone());
        self.note(&status);
    }

    /// The pane that is on screen.
    #[cfg(test)]
    fn pane_mut(&mut self) -> &mut EditorPane {
        &mut self.panes[front_tab(&self.groups)]
    }

    /// Sets how the agent endpoint is started.
    ///
    /// Without this, the endpoint is taken from the environment. Setting it
    /// explicitly lets a caller point the editor at a different endpoint
    /// without touching the process environment.
    pub fn set_endpoint(&mut self, settings: EndpointSettings) {
        self.endpoint = Some(settings);
    }

    /// Sets the debug adapter to use.
    ///
    /// Without this, there is no debugger: the editor says so when a
    /// session is asked for.
    pub fn set_debug_adapter(&mut self, adapter: AdapterConfig) {
        self.debug_adapter = Some(adapter);
    }

    pub fn run(mut self) -> Result<()> {
        let event_loop = EventLoop::new().context("failed to create the native event loop")?;
        event_loop
            .run_app(&mut self)
            .context("native event loop failed")
    }

    /// Whether the editor is in development mode.
    ///
    /// In development mode the editor writes what it says about itself into
    /// a file, so that a run nobody watched can still be read afterwards.
    pub fn is_development_mode(&self) -> bool {
        self.dev
    }

    /// The editor's buffer.
    pub fn editor(&self) -> &Editor {
        &self.panes[front_tab(&self.groups)].editor
    }

    /// The editor's buffer, for the callers that edit it.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.panes[front_tab(&self.groups)].editor
    }

    /// The view of the text on screen.
    pub fn view(&self) -> &EditorView {
        &self.panes[front_tab(&self.groups)].view
    }

    /// The view of the text on screen, for the callers that move it.
    pub fn view_mut(&mut self) -> &mut EditorView {
        &mut self.panes[front_tab(&self.groups)].view
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
        self.panes[front_tab(&self.groups)].document.as_ref()
    }

    /// The file that is open, for the callers that change it.
    pub fn document_mut(&mut self) -> Option<&mut Document> {
        self.panes[front_tab(&self.groups)].document.as_mut()
    }

    /// Opens a file in a tab.
    ///
    /// A file that is already open is not opened twice: the tab that is
    /// already there comes to the front instead.
    fn open_file(&mut self, path: &Path) -> bool {
        // A workspace file is not a text file: it opens a workspace.
        if workspace_file::is_workspace_file(path) {
            return self.open_the_workspace(path);
        }
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
        // The file is opened in the column that has the keyboard: a column
        // has its own tabs, and the tabs of one column are not the tabs of
        // the other.
        self.groups.add(index);
        // The path above the text is the path of the file that was just
        // opened.
        self.refresh_crumbs_of(index);
        // The file that is opened is drawn at the size the window has.
        self.apply_the_zoom();
        // The breakpoints of the file follow its text again: the anchors of
        // the closed file died with its buffer.
        self.track_the_breakpoints_of_the_pane(self.panes.len() - 1);
        self.show_tree_of(path);
        self.activate_tab(index);
        // The branch of the repository the file belongs to is read again:
        // the file may be in another repository than the one before it.
        if let Some(root) = git::repository_root(path) {
            self.remember_the_branch(&root);
        } else {
            self.current_branch = None;
        }
        true
    }

    /// Gives the breakpoints of a file new places.
    ///
    /// The anchors of a file that was closed died with its buffer; when the
    /// file is opened again, its breakpoints get new ones, and they follow
    /// the text from then on.
    fn track_the_breakpoints_of_the_pane(&mut self, pane: usize) {
        let Some(uri) = self.uri_of_a_pane(pane) else {
            return;
        };
        let lines = self.breakpoints.lines_of(&uri);
        self.breakpoints.forget_the_places(&uri);
        for line in lines {
            let anchor = {
                let editor = &mut self.panes[pane].editor;
                let offset = editor
                    .buffer()
                    .line_range(line.saturating_sub(1) as usize)
                    .start;
                editor.buffer_mut().anchor(offset, Bias::Right)
            };
            self.breakpoints.remember(&uri, line, anchor);
        }
    }

    /// Brings a tab to the front.
    fn activate_tab(&mut self, index: usize) {
        if index >= self.panes.len() {
            return;
        }
        let column = self.groups.column_of(index);
        if column == Some(self.groups.active()) && self.groups.front() == Some(index) {
            return;
        }
        // The tab belongs to a column: it comes to the front there, and that
        // column becomes the one that has the keyboard.
        self.groups.focus(index);
        self.tabs.activate_index(index);
        // The language server is told about the document that is now on
        // screen; it knows nothing about the ones that were closed.
        self.tell_server_about_the_new_document();
        // The change waiting for the user is the one about the file that is
        // now in front.
        self.note_new_diff();
    }

    /// Opens a column to the right of the active one.
    ///
    /// The new column shows a file of its own: a column has its own tabs,
    /// and the tabs of one column are not the tabs of the other. The column
    /// that opens becomes the one that has the keyboard.
    fn split_right(&mut self) {
        let tab = self.tabs.open_scratch("untitled");
        let index = self.tabs.len() - 1;
        self.panes.push(EditorPane::scratch(tab, None));
        self.groups.split(index);
        // The path above the text is the path of the file that was just
        // opened.
        self.refresh_crumbs_of(index);
        // The file that is opened is drawn at the size the window has.
        self.apply_the_zoom();
        self.status = Some(format!("窗口分成了 {} 栏", self.groups.len()));
    }

    /// Ends the box of text, if there is one.
    ///
    /// The box is over when the keys that made it are let go: what is left is
    /// a plain selection on every line of the box.
    fn end_the_column_selection(&mut self) {
        self.panes[front_tab(&self.groups)].column = None;
    }

    /// Cancels the selection: one caret is left.
    ///
    /// The escape key does that, and the box of text goes with it.
    fn cancel_the_selection(&mut self) {
        let pane = &mut self.panes[front_tab(&self.groups)];
        pane.editor.collapse_to_primary();
        pane.column = None;
    }

    /// Opens a box of text, or moves the one that is open.
    ///
    /// A box is the same columns of several lines: the corner that moves says
    /// how far the box reaches. The box belongs to the file that is on screen,
    /// so it is where it was when the file is left and come back to.
    fn column_select(&mut self, direction: multicursor::Direction) -> bool {
        let tab = front_tab(&self.groups);
        let pane = &mut self.panes[tab];
        let caret = pane.editor.caret().0;
        // The box opens at the caret, or the box that is open moves one step
        // further.
        let mut box_ = match pane.column {
            Some(box_) => box_,
            None => multicursor::ColumnSelection::of(pane.editor.buffer(), caret),
        };
        if !box_.move_current(direction, pane.editor.buffer()) {
            // The edge of the text is the end of the road.
            return false;
        }
        let ranges = multicursor::ranges(pane.editor.buffer(), &box_);
        pane.editor.set_selection_ranges(&ranges);
        pane.column = Some(box_);
        self.status = Some(format!("列选择：{} 行", ranges.len()));
        true
    }

    /// Adds a caret on the next place the same word appears.
    ///
    /// The first press takes the word under the caret; every press after that
    /// adds the next place the word appears. The word is the one the rename
    /// uses: a run of letters, digits and underscores.
    fn add_the_next_word(&mut self) -> bool {
        let tab = front_tab(&self.groups);
        let pane = &mut self.panes[tab];
        let buffer = pane.editor.buffer();
        let primary = *pane.editor.selection();
        let taken: Vec<Range<usize>> = pane
            .editor
            .selections()
            .iter()
            .map(|selection| {
                let range = selection.range(buffer);
                range.start.0..range.end.0
            })
            .collect();
        // The word that is already selected, or the word under the caret.
        let word = if primary.is_empty(buffer) {
            rename::word_at_caret(buffer, primary.head(buffer).0)
        } else {
            let range = primary.range(buffer);
            Some((range.start.0, range.end.0))
        };
        let Some((start, end)) = word else {
            self.status = Some("光标处没有词".to_owned());
            return false;
        };
        // Everything that is read from the text is read before the text is
        // changed: the borrow of the buffer and the change cannot live
        // together.
        let text = buffer.slice(TextOffset(start)..TextOffset(end));
        let empty = primary.is_empty(buffer);
        let from = primary.range(buffer).end.0;
        if empty {
            // The first press only takes the word: the next press is the one
            // that looks for the next place.
            pane.editor.select_range(TextOffset(start), TextOffset(end));
            self.status = Some(format!("选中了 {text}"));
            return true;
        }
        let Some(found) = multicursor::next_occurrence(buffer, &text, from, &taken) else {
            self.status = Some("没有下一个了".to_owned());
            return false;
        };
        // The new place is selected like the others: editing then reaches all
        // of them at once.
        let mut ranges: Vec<Range<TextOffset>> = taken
            .iter()
            .map(|range| TextOffset(range.start)..TextOffset(range.end))
            .collect();
        ranges.push(TextOffset(found.start)..TextOffset(found.end));
        pane.editor.set_selection_ranges(&ranges);
        self.status = Some(format!("又选中了一处 {text}"));
        true
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
        // The column the tab was in loses it. A column that has no tabs left
        // goes away with it, unless it is the only column there is.
        self.groups.remove(index);

        if self.panes.is_empty() {
            // The last tab was closed: an empty editor takes its place, so
            // there is always something to type into.
            let tab = self.tabs.open_scratch("untitled");
            self.panes.push(EditorPane::scratch(tab, None));
            self.groups = Groups::new();
            self.groups.add(0);
            return;
        }
        // The tab in front of the active column is the one that is on screen.
        if let Some(front) = self.groups.front() {
            self.tabs.activate_index(front);
        }
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
        let pane = &mut self.panes[front_tab(&self.groups)];
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
        let pane = &mut self.panes[front_tab(&self.groups)];
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
    /// file in front of the user is the tree they are working in. A workspace
    /// that is open holds the tree instead: the folders it names do not change
    /// because another file was opened.
    fn show_tree_of(&mut self, path: &Path) {
        if self.open_workspace.is_some() {
            return;
        }
        let Some(root) = path.parent() else {
            return;
        };
        let follows = self
            .tree
            .as_ref()
            .and_then(|tree| tree.roots().first().copied())
            == Some(root);
        if !follows {
            self.tree = Some(FileForest::single(root.to_path_buf()));
        }
    }

    /// Opens a workspace file.
    ///
    /// The file names the folders of the project, and the tree of files
    /// becomes the tree of those folders. What went wrong is said out loud: a
    /// workspace that cannot be read is not a project that silently has no
    /// files in it.
    fn open_the_workspace(&mut self, path: &Path) -> bool {
        match workspace_file::Workspace::open(path) {
            Ok(workspace) => {
                let roots = workspace
                    .roots
                    .iter()
                    .map(|root| (root.name.clone(), root.path.clone()))
                    .collect();
                self.tree = Some(FileForest::many(roots));
                let missing: Vec<String> = workspace
                    .missing()
                    .iter()
                    .map(|root| root.name.clone())
                    .collect();
                let name = workspace.name.clone();
                let count = workspace.roots.len();
                self.open_workspace = Some(workspace);
                let mut said = format!("工作区 {name} 打开了 {count} 个文件夹");
                if !missing.is_empty() {
                    said.push_str(&format!("，其中的 {} 不在了", missing.join("、")));
                }
                self.status = Some(said);
                true
            }
            Err(error) => {
                self.status = Some(format!("打不开工作区：{error}"));
                false
            }
        }
    }

    /// The folder the project is in: the first root of the tree of files, or
    /// the folder of the file that is open.
    fn the_root_of_the_project(&self) -> Option<PathBuf> {
        if let Some(tree) = self.tree.as_ref()
            && let Some(root) = tree.roots().first()
        {
            return Some((*root).to_path_buf());
        }
        self.panes
            .get(front_tab(&self.groups))
            .and_then(|pane| pane.document.as_ref())
            .and_then(|document| document.path().parent())
            .map(Path::to_path_buf)
    }

    /// Saves the workspace.
    ///
    /// A workspace that is open is written back into the file it was read
    /// from. A project that has no workspace file yet gets one of its own,
    /// named after the folder and put next to it.
    fn save_the_workspace(&mut self) {
        let (path, contents) = match self.open_workspace.clone() {
            Some(workspace) => (workspace.path.clone(), workspace.contents()),
            None => {
                let Some(root) = self.the_root_of_the_project() else {
                    self.status = Some("还没有工程可以保存：先打开一个文件夹".to_owned());
                    return;
                };
                let Some(name) = root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                else {
                    self.status = Some(format!("{} 没有名字，存不成工作区", root.display()));
                    return;
                };
                let path = root.join(format!("{name}{}", workspace_file::SUFFIX));
                let contents = workspace_file::Contents {
                    folders: vec![workspace_file::Folder {
                        path: ".".to_owned(),
                        name: None,
                    }],
                    settings: std::collections::BTreeMap::new(),
                };
                (path, contents)
            }
        };
        match workspace_file::write(&path, &contents) {
            Ok(()) => self.status = Some(format!("工作区写到 {}", path.display())),
            Err(error) => self.status = Some(format!("工作区写不进去：{error}")),
        }
    }

    /// Draws the text at the size the window has.
    ///
    /// The size belongs to the window and not to a file: every file is drawn
    /// at the same size. The view of every file is told about it, so that a
    /// file that is opened later is drawn at the same size as the others.
    fn apply_the_zoom(&mut self) {
        let zoom = self.zoom;
        for pane in &mut self.panes {
            pane.view.set_zoom(zoom);
        }
    }

    /// Draws the text bigger or smaller.
    ///
    /// The size is remembered in the file of the settings, so that the person
    /// does not have to make the text big again after every restart.
    fn set_zoom(&mut self, zoom: Zoom) {
        if zoom == self.zoom {
            return;
        }
        self.zoom = zoom;
        self.apply_the_zoom();
        if let Some(path) = self.zoom_path.clone()
            && let Err(error) = zoom::write(&path, zoom)
        {
            self.status = Some(format!("字号没有记住：{error}"));
        }
    }

    /// Works out the pieces of the path of a file.
    ///
    /// The path is the one on the disk, shortened with the root of the
    /// project the file belongs to. The tree of files cannot say it: the tree
    /// is rooted at the folder of the file, so it does not know where that
    /// folder is in the project.
    ///
    /// The pieces belong to the file and not to the window: two columns show
    /// two files, and each of them has its own path above the text.
    fn refresh_crumbs_of(&mut self, index: usize) {
        let path = self
            .panes
            .get(index)
            .and_then(|pane| pane.document.as_ref())
            .map(|document| document.path().to_path_buf());
        let Some(path) = path else {
            if let Some(pane) = self.panes.get_mut(index) {
                pane.crumbs.clear();
            }
            return;
        };
        let root = lsp::workspace_root(&path);
        let crumbs = breadcrumbs::of(&path, Some(root.as_path()));
        if let Some(pane) = self.panes.get_mut(index) {
            pane.crumbs = crumbs;
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
        let policy = self.panes[front_tab(&self.groups)].policy;
        self.settings = Some(Settings::with_keys(
            policy,
            self.zoom,
            self.command_keys(),
        ));
    }

    /// The key each command has now, for the panel of the settings.
    fn command_keys(&self) -> Vec<KeyRow> {
        self.commands
            .all()
            .iter()
            .map(|command| KeyRow {
                command: command.id.clone(),
                title: format!("{} · {}", command.category, command.title),
                key: command.key_text().unwrap_or_default(),
            })
            .collect()
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

        // While the panel is waiting for a key, the next key is the answer:
        // a key that is given to a command is not a key of the panel.
        if self.settings.as_ref().is_some_and(Settings::recording) {
            return self.take_the_new_key(key);
        }

        // The panel closes on Escape, and on Enter when the row is not one of
        // the keys: a key row waits for the key that is to replace its own.
        if matches!(key, Key::Named(NamedKey::Escape)) {
            self.settings = None;
            return true;
        }
        if matches!(key, Key::Named(NamedKey::Enter)) {
            let recording = self
                .settings
                .as_mut()
                .map(Settings::begin_recording)
                .unwrap_or(false);
            if recording {
                self.status = Some("按新的键，Esc 取消".to_owned());
            } else {
                self.settings = None;
            }
            return true;
        }

        // What was changed matters: the size of the text is a setting of its
        // own, and changing the limit of the highlighter must not change it.
        let (changed, changed_kind, policy, zoom) = {
            let Some(settings) = self.settings.as_mut() else {
                return true;
            };
            let mut changed_kind = None;
            let changed = match key {
                Key::Named(NamedKey::ArrowUp) => {
                    settings.move_selection(-1);
                    false
                }
                Key::Named(NamedKey::ArrowDown) => {
                    settings.move_selection(1);
                    false
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    let kind = settings.kind_of_the_picked();
                    if settings.change_value(-1) {
                        changed_kind = kind;
                        true
                    } else {
                        false
                    }
                }
                Key::Named(NamedKey::ArrowRight) => {
                    let kind = settings.kind_of_the_picked();
                    if settings.change_value(1) {
                        changed_kind = kind;
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            (changed, changed_kind, settings.policy(), settings.zoom())
        };
        // A new value is put in force at once: a setting that has to be
        // confirmed before it counts is a setting nobody changes.
        if changed {
            self.set_highlight_policy(policy);
            if changed_kind == Some(SettingKind::FontSize) {
                self.set_zoom(zoom);
                self.status = Some(format!("字号 {}", zoom.label()));
            } else {
                self.status = Some(format!(
                    "高亮上限：{} 字节 / {} 行",
                    policy.max_bytes, policy.max_lines
                ));
            }
        }
        true
    }

    /// Gives the picked command the key that was just pressed.
    ///
    /// A key that cannot be read and a command that is not there are said out
    /// loud: the panel does not close on a change it could not make. A key
    /// that two commands want is said out loud as well, but it is still the
    /// key of the command that was picked — the last one to ask wins, the way
    /// it does in a file.
    fn take_the_new_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        if matches!(key, Key::Named(NamedKey::Escape)) {
            if let Some(settings) = self.settings.as_mut() {
                settings.cancel_recording();
            }
            self.status = Some("没有改键位".to_owned());
            return true;
        }
        // A modifier on its own is not a key: the panel keeps waiting for
        // one that is.
        let Some(text) = keybinding_text(self.modifiers, key) else {
            return true;
        };
        let Some(row) = self
            .settings
            .as_ref()
            .and_then(Settings::selected_key)
            .cloned()
        else {
            return true;
        };
        let binding = match Keybinding::parse(&text) {
            Ok(binding) => binding,
            Err(said) => {
                self.status = Some(format!("{said}；这个键没有用上"));
                return true;
            }
        };
        let written = binding.text();
        if let Err(said) = self.commands.rebind(&row.command, binding) {
            self.status = Some(said);
            if let Some(settings) = self.settings.as_mut() {
                settings.cancel_recording();
            }
            return true;
        }
        let conflict = self
            .commands
            .conflicts()
            .into_iter()
            .find(|conflict| conflict.first == row.command || conflict.second == row.command);
        if let Some(settings) = self.settings.as_mut() {
            settings.set_key(&row.command, &written);
        }
        self.status = Some(match conflict {
            Some(conflict) => format!(
                "{} 和 {} 都要 {}：按下的这个键会先给排在前面的一条",
                conflict.first, conflict.second, written
            ),
            None => format!("{} 的键位改成 {written}", row.title),
        });
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
    ///
    /// The tab belongs to the column it was clicked in: a column has its own
    /// tabs, and the one that is clicked comes to the front of its own
    /// column. That column becomes the one that has the keyboard.
    fn press_tab_bar(&mut self, point: Point, column: usize, layout: &WindowLayout) {
        let strip = {
            let Some(renderer) = self.renderer.as_mut() else {
                return;
            };
            let Some(place) = layout.columns.get(column) else {
                return;
            };
            let mut measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);
            let tabs = tabs_of_a_column(&self.tabs, &self.groups, column);
            let active = front_of_a_column(&self.groups, column);
            shell::TabStrip::layout(&tabs, active, place.tab_bar, &mut measurer)
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

    /// Does what the row of the side bar that was clicked says.
    fn press_side_bar(&mut self, point: Point, panel: Rect) {
        let rows = self.sidebar_rows(panel);
        let Some(row) = sidebar::hit(&rows, point) else {
            return;
        };
        // A directory of the tree is opened and closed.
        match &row.action {
            SideBarAction::None => {}
            SideBarAction::OpenFile(path) => {
                self.open_file(path);
            }
            SideBarAction::OpenFileAt { path, line } => {
                self.open_file(path);
                self.go_to_line(*line);
            }
            SideBarAction::ToggleBreakpoint(line) => self.toggle_breakpoint_at(*line),
            SideBarAction::ToggleDirectory(path) => {
                if let Some(tree) = self.tree.as_mut() {
                    tree.toggle(path);
                }
            }
        }
    }

    /// Shows the section of the activity that was clicked.
    ///
    /// Clicking the activity that is on screen hides the side bar, the way
    /// VS Code does it.
    fn press_activity_bar(&mut self, point: Point, layout: &WindowLayout) {
        let bar = activity_bar::ActivityBar::layout(
            layout.activity_bar,
            self.sidebar.selected(),
            &self.badges(),
        );
        let Some(activity) = bar.hit(point) else {
            return;
        };
        self.sidebar.pick(activity);
    }

    /// Jumps to the place of the path that was clicked.
    ///
    /// A folder of the path is a folder of files: the tree of files goes to
    /// that folder, so that the person can see what is in it. The file
    /// itself is the file that is open already, so it only says so.
    fn press_breadcrumbs(&mut self, point: Point, column: usize, layout: &WindowLayout) {
        let strip = {
            let Some(renderer) = self.renderer.as_mut() else {
                return;
            };
            let Some(place) = layout.columns.get(column) else {
                return;
            };
            let Some(tab) = self.groups.front_of(column) else {
                return;
            };
            let mut measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);
            breadcrumbs::strip(place.breadcrumbs, &self.panes[tab].crumbs, &mut measurer)
        };
        self.go_to_the_place_of_the_path(&strip, point);
    }

    /// Does what the piece of the path under the point says.
    fn go_to_the_place_of_the_path(&mut self, strip: &breadcrumbs::Strip, point: Point) {
        let Some(crumb) = breadcrumbs::hit(strip, point) else {
            return;
        };
        if crumb.file {
            self.status = Some(format!("已经在这个文件里：{}", crumb.path.display()));
            return;
        }
        // The folder of the path: the tree of files goes there, and the side
        // bar shows it. The workspace that was open let go of the tree: the
        // tree is that folder now, not the folders of the workspace.
        self.sidebar.show(Activity::Files);
        self.tree = Some(FileForest::single(crumb.path.clone()));
        self.open_workspace = None;
        self.status = Some(format!("文件树现在看的是 {}", crumb.path.display()));
    }

    /// Jumps to the part of the file that was clicked on the map.
    ///
    /// The map shows the whole file, so a click on it says where in the file
    /// the person wants to be. The caret stays where it is: the map is for
    /// finding a place, not for moving the caret.
    fn press_minimap(&mut self, point: Point, column: usize, layout: &WindowLayout) {
        let Some(place) = layout.columns.get(column) else {
            return;
        };
        let Some(minimap) = place.minimap else {
            return;
        };
        let Some(tab) = self.groups.front_of(column) else {
            return;
        };
        let pane = &mut self.panes[tab];
        let total = pane.editor.buffer().line_count();
        let Some(line) = pane
            .view
            .minimap(pane.editor.buffer())
            .line_at(point.y, minimap)
        else {
            return;
        };
        let visible = ((place.body.size.height / self.zoom.line_height()).floor() as usize).max(1);
        let first = line.saturating_sub(visible / 2).min(total.saturating_sub(1));
        pane.view.view.first_line = first;
    }

    /// Puts the caret on a line of the file that is open.
    fn go_to_line(&mut self, line: usize) {
        let pane = &mut self.panes[front_tab(&self.groups)];
        let buffer = pane.editor.buffer();
        if line == 0 || line > buffer.line_count() {
            return;
        }
        let offset = buffer.line_range(line - 1).start.0;
        pane.editor.set_carets(&[TextOffset(offset)]);
    }

    /// Puts a breakpoint on a line of the file that is open, or takes it away.
    fn toggle_breakpoint_at(&mut self, line: u32) {
        let Some(uri) = self.uri_of_the_open_file() else {
            self.status = Some("没有打开的文件，断点放不了".to_owned());
            return;
        };
        let there = self.breakpoints.toggle(&uri, line);
        self.status = Some(if there {
            format!("第 {line} 行有了断点")
        } else {
            format!("第 {line} 行的断点去掉了")
        });
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
            .get(front_tab(&self.groups))
            .and_then(|pane| pane.document.as_ref())
            .map(|document| document.display_path())
            .zip(
                self.panes
                    .get(front_tab(&self.groups))
                    .map(|pane| pane.editor.text()),
            );
        let selection = self
            .panes
            .get(front_tab(&self.groups))
            .and_then(|pane| context::selection_text(&pane.editor))
            .zip(
                self.panes
                    .get(front_tab(&self.groups))
                    .and_then(|pane| pane.document.as_ref())
                    .map(|document| document.display_path()),
            );
        let tabs: Vec<(String, String)> = self
            .panes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != front_tab(&self.groups))
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
            .get(front_tab(&self.groups))
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
        let Some(pane) = self.panes.get(front_tab(&self.groups)) else {
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

        let Some(pane) = self.panes.get(front_tab(&self.groups)) else {
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
                let pane = &mut self.panes[front_tab(&self.groups)];
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

        let pane = &mut self.panes[front_tab(&self.groups)];
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
        let layout = self.window_layout(self.window_size(scale_factor));
        // The lines that fit on screen are the ones of the column that has
        // the keyboard.
        let Some(column) = layout.column(self.groups.active()) else {
            return 1;
        };
        ((column.body.size.height / self.zoom.line_height()).floor() as usize).max(1)
    }

    /// Rebuilds the layout of the text on screen.
    ///
    /// The layout is only rebuilt when something it depends on changed, so
    /// a mouse move does not pay for measuring the text again.
    fn rebuild_layout(&mut self, scale_factor: f64) {
        let size = self.window_size(scale_factor);
        let layout = self.window_layout(size);
        // The layout of the text is the one of the column that has the
        // keyboard.
        let Some(place) = layout.column(self.groups.active()).copied() else {
            return;
        };
        let body = place.body;
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let mut measurer = RendererMeasurer::new(renderer, TextFamily::Monospace);
        let pane = &mut self.panes[front_tab(&self.groups)];
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
                    if Some(uri.as_str()) != self.panes[front_tab(&self.groups)].document_uri.as_deref() {
                        continue;
                    }
                    let marks = lsp::marks_for(self.panes[front_tab(&self.groups)].editor.buffer(), &diagnostics);
                    self.panes[front_tab(&self.groups)].errors = marks
                        .iter()
                        .filter(|mark| mark.severity == MarkSeverity::Error)
                        .count();
                    self.panes[front_tab(&self.groups)].warnings = marks
                        .iter()
                        .filter(|mark| mark.severity == MarkSeverity::Warning)
                        .count();
                    self.panes[front_tab(&self.groups)].marks = marks;
                    changed = true;
                }
                LspEvent::Disconnected { reason } => {
                    eprintln!("the language server went away: {reason}");
                    self.status = Some(format!("语言服务器断开了：{reason}"));
                    self.server = None;
                    self.panes[front_tab(&self.groups)].marks.clear();
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
            let caret = self.panes[front_tab(&self.groups)].editor.caret().0;
            let prefix = completion::word_before(self.panes[front_tab(&self.groups)].editor.buffer(), caret);
            let items = completion::matching(list.items, &prefix);
            let start = completion::word_start(self.panes[front_tab(&self.groups)].editor.buffer(), caret);
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

        // The rename answers are put into the buffers. The edits of one file
        // are one change of that file.
        let renames = match self.server.as_ref() {
            Some(server) => server.drain_renames(),
            None => Vec::new(),
        };
        for answer in renames {
            match answer {
                Ok(edit) => {
                    // The answer is only about the text that was there when
                    // it was asked for. A text that changed in the meantime
                    // is a different text, and the edits do not fit it.
                    let Some(request) = self.renaming.take() else {
                        continue;
                    };
                    if self.apply_rename(edit, &request) {
                        changed = true;
                    }
                }
                Err(error) => {
                    self.renaming = None;
                    eprintln!("could not rename: {error}");
                    self.status = Some(format!("重命名失败：{error}"));
                    changed = true;
                }
            }
        }

        // The code actions the server offers open the list of the things it
        // can do about the place of the caret. Nothing is changed before the
        // user has picked one.
        let answers = match self.server.as_ref() {
            Some(server) => server.drain_code_actions(),
            None => Vec::new(),
        };
        for answer in answers {
            match answer {
                Ok(actions) => {
                    if actions.is_empty() {
                        self.status = Some("这里没有可以做的操作".to_owned());
                    } else if let Some(popup) = ActionPopup::new(actions) {
                        let count = popup.len();
                        self.actions = Some(popup);
                        self.status = Some(format!(
                            "有 {count} 个操作，上下键挑，回车确认，Esc 取消"
                        ));
                    }
                    changed = true;
                }
                Err(error) => {
                    eprintln!("could not get the code actions: {error}");
                    self.status = Some(format!("代码操作失败：{error}"));
                    changed = true;
                }
            }
        }

        // The answers of the commands that were run for the server.
        let commands = match self.server.as_ref() {
            Some(server) => server.drain_commands(),
            None => Vec::new(),
        };
        for answer in commands {
            match answer {
                Ok(_) => {
                    self.status = Some("服务器把命令做完了".to_owned());
                    changed = true;
                }
                Err(error) => {
                    eprintln!("the command of the server failed: {error}");
                    self.status = Some(format!("命令没跑起来：{error}"));
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
        let active = front_tab(&self.groups);
        self.tell_server_about_the_change_of(active);
    }

    /// Tells the language server what the text of one tab looks like now.
    fn tell_server_about_the_change_of(&mut self, pane: usize) {
        let Some(uri) = self.panes[pane].document_uri.clone() else {
            return;
        };
        let Some(change) = self.panes[pane].editor.last_change() else {
            return;
        };
        let version = self.panes[pane].editor.buffer().version();
        if self.panes[pane].server_version == Some(version) {
            // The server already knows about this text.
            return;
        }
        let changes = lsp::changes_for(self.panes[pane].editor.buffer(), change);
        let text = self.panes[pane].editor.text();
        self.panes[pane].server_version = Some(version);
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
        let pane = &self.panes[front_tab(&self.groups)];
        let total_lines = pane.editor.buffer().line_count();
        let visible = self.visible_lines(scale_factor);
        let caret = pane.editor.caret().0;
        let pane = &mut self.panes[front_tab(&self.groups)];
        pane.view
            .ensure_caret_visible(pane.editor.buffer(), caret, total_lines, visible);
    }

    /// The column a click is in, and which gets the keyboard.
    ///
    /// A click in a column is a click in that column: the keyboard goes
    /// there, because that is what the person means.
    fn column_of_the_click(&mut self, cursor: Point, layout: &WindowLayout) -> Option<usize> {
        let column = layout.column_at(cursor)?;
        self.groups.activate(column);
        Some(column)
    }

    /// Handles a mouse press in the editor body.
    ///
    /// Ctrl+click adds a caret instead of moving the one that is there.
    fn press_body(&mut self, scale_factor: f64) {
        let Some(cursor) = self.cursor else {
            return;
        };
        let size = self.window_size(scale_factor);
        let layout = self.window_layout(size);
        // The bar with the activities is on the left of the window: a click
        // there picks the section of the side bar. The status bar is drawn
        // over the bottom of the bar, so it is not part of it.
        if layout.activity_bar.contains(cursor) && cursor.y < layout.status_bar.origin.y {
            self.press_activity_bar(cursor, &layout);
            return;
        }
        // Which column is the click in? The click gives the keyboard to the
        // column it is in: that is what the person means.
        let column = self.column_of_the_click(cursor, &layout);
        if let Some(column) = column {
            let Some(place) = layout.columns.get(column) else {
                return;
            };
            // The tab strip and the file tree are around the text: a click
            // there is not a click in the document.
            if place.tab_bar.contains(cursor) {
                self.press_tab_bar(cursor, column, &layout);
                return;
            }
            // The path above the text: a click on a piece of it says where the
            // person wants to be.
            if place.breadcrumbs.contains(cursor) {
                self.press_breadcrumbs(cursor, column, &layout);
                return;
            }
            // The map of the document is on the right of the text: a click on
            // it says where in the file to go.
            if let Some(minimap) = place.minimap
                && minimap.contains(cursor)
            {
                self.press_minimap(cursor, column, &layout);
                return;
            }
        }
        if layout.sidebar.size.width > 0.0 && layout.sidebar.contains(cursor) {
            self.press_side_bar(cursor, layout.sidebar_content());
            return;
        }
        // The panel of the debugger is at the bottom of the window: a click
        // on a frame of the stack picks that frame.
        if self.press_the_debug_panel(cursor, size) {
            return;
        }
        // The text of the document is in the column that was clicked.
        let Some(column) = column else {
            return;
        };
        let Some(place) = layout.columns.get(column) else {
            return;
        };
        let body = place.body;
        if !body.contains(cursor) {
            return;
        }
        let add = self.modifiers.control_key();
        let Some(tab) = self.groups.front_of(column) else {
            return;
        };
        let pane = &mut self.panes[tab];
        pane.view.press(
            &mut pane.editor,
            cursor.x - body.origin.x,
            cursor.y - body.origin.y,
            add,
        );
        self.keep_caret_visible(scale_factor);
    }

    /// Handles a click in the panel of the debugger.
    ///
    /// A click on a frame of the stack picks that frame: the variables that
    /// are shown are the ones of the frame that is picked, and an
    /// expression is asked about in it.
    ///
    /// Two clicks in a row on the same frame take the person to the place of
    /// that frame in the code.
    ///
    /// Returns whether the click was on a frame: a click that was not is
    /// none of the panel's business.
    fn press_the_debug_panel(&mut self, cursor: Point, size: Size) -> bool {
        if self.task_panel {
            // The panel of the task is drawn over the panel of the
            // debugger: the click is not on a frame.
            return false;
        }
        let Some(debug) = self.debug.as_ref() else {
            return false;
        };
        if debug.state != DebugState::Stopped {
            return false;
        }
        let frames = debug.frames.len();
        let Some(index) = debug::frame_at(cursor, size, frames) else {
            return false;
        };
        // Two clicks in a row on the same frame are a double click.
        let now = Instant::now();
        let twice = self.last_frame_click.is_some_and(|(last, at)| {
            last == index && now.duration_since(at) < Duration::from_millis(500)
        });
        self.last_frame_click = Some((index, now));
        if twice {
            self.jump_to_the_frame(index);
            return true;
        }
        let Some(debug) = self.debug.as_mut() else {
            return false;
        };
        if debug.select_frame(index) {
            self.status = Some(format!("挑了第 {} 帧", index + 1));
        }
        true
    }

    /// Takes the person to the place of a frame in the code.
    ///
    /// The file of the frame is opened and the caret is put on the line the
    /// frame is on: that is where the program is.
    fn jump_to_the_frame(&mut self, index: usize) {
        let Some(debug) = self.debug.as_ref() else {
            return;
        };
        let Some(frame) = debug.frames.get(index) else {
            return;
        };
        let Some((path, line)) = debug::place_of(frame) else {
            // The adapter did not say where the program is: the editor says
            // so instead of doing nothing.
            self.status = Some("这一帧没有位置，跳不了".to_owned());
            return;
        };
        if !self.open_file(&path) {
            // The file could not be opened: the reason was said already.
            return;
        }
        let pane = &mut self.panes[front_tab(&self.groups)];
        let offset = pane.editor.buffer().line_range(line.saturating_sub(1) as usize).start;
        pane.editor.set_carets(&[offset]);
        // The line of the frame is scrolled into the picture: a place that
        // is not on the screen is not a place.
        let scale_factor = self
            .window
            .as_ref()
            .map(|window| window.scale_factor())
            .unwrap_or(1.0);
        self.keep_caret_visible(scale_factor);
        self.status = Some(format!(
            "跳到 {} 的第 {line} 行",
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string())
        ));
    }

    /// Handles the mouse moving while a drag is in progress.
    fn drag_body(&mut self, scale_factor: f64) {
        if !self.panes[front_tab(&self.groups)].view.is_dragging() {
            return;
        }
        let Some(cursor) = self.cursor else {
            return;
        };
        let body = self
            .window_layout(self.window_size(scale_factor))
            .columns
            .get(self.groups.active())
            .map(|column| column.body)
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        let pane = &mut self.panes[front_tab(&self.groups)];
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
            let pane = &mut self.panes[front_tab(&self.groups)];
            if pane.view.move_vertical(&mut pane.editor, delta) {
                return;
            }
        }
        let total_lines = self.panes[front_tab(&self.groups)].editor.buffer().line_count();
        self.panes[front_tab(&self.groups)].view.scroll(delta, total_lines);
        self.rebuild_layout(scale_factor);
        let pane = &mut self.panes[front_tab(&self.groups)];
        pane.view.move_vertical(&mut pane.editor, delta);
    }

    /// Scrolls the body by a number of lines.
    fn scroll_body(&mut self, lines: isize) {
        let total_lines = self.panes[front_tab(&self.groups)].editor.buffer().line_count();
        self.panes[front_tab(&self.groups)].view.scroll(lines, total_lines);
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
                    if let Err(error) = completion.apply(&mut self.panes[front_tab(&self.groups)].editor) {
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
        let Some(uri) = self.panes[front_tab(&self.groups)].document_uri.clone() else {
            return;
        };
        let caret = self.panes[front_tab(&self.groups)].editor.caret().0;
        let position = lsp::position_of(self.panes[front_tab(&self.groups)].editor.buffer(), caret);
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
        let Some(uri) = self.panes[front_tab(&self.groups)].document_uri.clone() else {
            self.status = Some("这个文件没有语言服务器，格式化不了".to_owned());
            return;
        };
        let editor = &self.panes[front_tab(&self.groups)].editor;
        let buffer = editor.buffer();
        let selection = editor.selection().range(buffer);
        let outcome = self.server.as_ref().map(|server| {
            if selection.is_empty() {
                server.request_formatting(&uri, 4, true)
            } else {
                server.request_range_formatting(
                    &uri,
                    codeweaver_lsp::protocol::Range {
                        start: lsp::position_of(buffer, selection.start.0),
                        end: lsp::position_of(buffer, selection.end.0),
                    },
                    4,
                    true,
                )
            }
        });
        match outcome {
            Some(Ok(())) => {
                // The version is the version of the text the request is
                // about: an answer that comes back for a text that has
                // changed since then does not fit the text that is there
                // now.
                self.formatting = Some(self.panes[front_tab(&self.groups)].editor.version());
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
        if sent_at != self.panes[front_tab(&self.groups)].editor.version() {
            self.status = Some("文件在格式化期间变了，格式化结果没有用上".to_owned());
            return false;
        }
        let Some(changes) =
            lsp::formatting_changes(self.panes[front_tab(&self.groups)].editor.buffer(), &edits)
        else {
            self.status = Some("格式化结果对不上这个文件，没有改".to_owned());
            return false;
        };
        if changes.is_empty() {
            self.status = Some("这个文件已经排好了版".to_owned());
            return false;
        }
        if let Err(error) = self.panes[front_tab(&self.groups)].editor.replace_edits(changes) {
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

    /// Handles the key that asks the language server what it offers to do
    /// about the place of the caret.
    ///
    /// Alt+Enter opens the list of the things the server offers, the way it
    /// does in the other editors. Ctrl+. is not used for it: that key
    /// cancels the turn of the agent.
    fn handle_action_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        if self.actions.is_some() {
            let mut actions = self.actions.take().expect("the list is open");
            match key {
                Key::Named(NamedKey::Escape) => {
                    // The list closes and the editor takes the keyboard back.
                    self.status = Some("取消了代码操作".to_owned());
                    self.action_target = None;
                    return true;
                }
                Key::Named(NamedKey::ArrowDown) => {
                    actions.select_next();
                    self.actions = Some(actions);
                    return true;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    actions.select_previous();
                    self.actions = Some(actions);
                    return true;
                }
                Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) => {
                    self.apply_code_action(&actions);
                    return true;
                }
                _ => {
                    // The key was not one of the list: the list closes and
                    // the key goes on to the editor.
                }
            }
        }

        if self.modifiers.alt_key() && is_enter(key) {
            self.request_code_action();
            return true;
        }
        false
    }

    /// Handles the keys of the activity bar.
    ///
    /// The keyboard is on one thing at a time. When the person says so, the
    /// activity bar has it: the up and the down key walk through the
    /// sections, `Enter` picks one, and `Esc` gives the keyboard back to the
    /// document.
    ///
    /// The keys with a modifier belong to the editor: the shortcuts are the
    /// same wherever the keyboard is, and the activity bar gives the keyboard
    /// up when one of them is pressed.
    fn handle_activity_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        if !self.activity_focus {
            return false;
        }
        if self.modifiers.control_key() || self.modifiers.alt_key() {
            self.activity_focus = false;
            return false;
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                self.activity_focus = false;
                self.status = Some("键盘还给正文了".to_owned());
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                let section = self.sidebar.section.previous();
                self.sidebar.show(section);
                self.status = Some(format!("分区：{}", section.label()));
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                let section = self.sidebar.section.next();
                self.sidebar.show(section);
                self.status = Some(format!("分区：{}", section.label()));
                true
            }
            Key::Named(NamedKey::Enter) => {
                let section = self.sidebar.section;
                self.sidebar.pick(section);
                self.status = Some(if self.sidebar.is_open() {
                    format!("分区：{}", section.label())
                } else {
                    "侧边栏收起来了".to_owned()
                });
                true
            }
            _ => {
                // The activity bar has the keyboard: what is typed does not
                // go into the document.
                true
            }
        }
    }

    /// Gives the keyboard to the activity bar.
    fn focus_the_activity_bar(&mut self) {
        self.activity_focus = true;
        self.sidebar.show(Activity::Files);
        self.status = Some("活动栏：↑↓ 换分区，Enter 选中，Esc 回到正文".to_owned());
    }

    /// Asks the server what it offers to do about the place of the caret.
    ///
    /// The answer comes later: the window collects it and opens the list of
    /// the things the server offers.
    fn request_code_action(&mut self) {
        let Some(uri) = self.panes[front_tab(&self.groups)].document_uri.clone() else {
            self.status = Some("这个文件没有语言服务器，代码操作不了".to_owned());
            return;
        };
        let caret = self.panes[front_tab(&self.groups)].editor.caret().0;
        let range = lsp::line_range(self.panes[front_tab(&self.groups)].editor.buffer(), caret);
        let outcome = self
            .server
            .as_ref()
            .map(|server| server.request_code_action(&uri, range, &[]));
        match outcome {
            Some(Ok(())) => {
                // The file the operations are about, and the version of its
                // text: the list may be picked much later.
                self.action_target = Some((
                    uri,
                    self.panes[front_tab(&self.groups)].editor.version(),
                ));
                self.status = Some("正在问服务器这里能做什么…".to_owned());
            }
            Some(Err(error)) => {
                eprintln!("could not ask the server for code actions: {error}");
                self.status = Some(format!("代码操作请求失败：{error}"));
            }
            None => self.status = Some("没有语言服务器，代码操作不了".to_owned()),
        }
    }

    /// Does the thing the user picked from the list.
    ///
    /// A code action either brings edits of its own or runs a command; the
    /// first is put into the files, the second is given to the server.
    fn apply_code_action(&mut self, actions: &ActionPopup) {
        let Some(action) = actions.selected_action() else {
            return;
        };
        let title = action.title.clone();
        if let Some(edit) = action.edit.as_ref() {
            // The operations belong to the file they were asked about, not to
            // the tab that happens to be on screen.
            let (uri, version) = match self.action_target.clone() {
                Some(target) => target,
                None => (
                    self.panes[front_tab(&self.groups)].document_uri.clone().unwrap_or_default(),
                    self.panes[front_tab(&self.groups)].editor.version(),
                ),
            };
            let outcome = self.apply_workspace_edit(edit, &uri, version, "代码操作");
            match outcome.reason {
                Some(reason) => self.status = Some(reason),
                None => {
                    let mut message = format!("做了：{title}");
                    if outcome.files > 1 {
                        message.push_str(&format!("（改了 {} 个文件", outcome.files));
                        if outcome.written > 0 {
                            message.push_str(&format!(
                                "，其中 {} 个不在编辑器里，已写入磁盘",
                                outcome.written
                            ));
                        }
                        message.push('）');
                    }
                    self.status = Some(message);
                }
            }
            return;
        }
        if let Some(command) = action.command.as_ref() {
            let outcome = self.server.as_ref().map(|server| {
                server.execute_command(&command.command, command.arguments.clone())
            });
            match outcome {
                Some(Ok(())) => self.status = Some(format!("让服务器去做：{title}")),
                Some(Err(error)) => {
                    eprintln!("could not run the command of the server: {error}");
                    self.status = Some(format!("命令没跑起来：{error}"));
                }
                None => self.status = Some("没有语言服务器，这个操作做不了".to_owned()),
            }
            return;
        }
        self.status = Some(format!("这个操作编辑器做不了：{title}"));
    }
    /// The room that is kept at the bottom of the window for the output of a
    /// task.
    fn task_panel_height(&self) -> f32 {
        if self.task_panel {
            TASK_PANEL_HEIGHT
        } else {
            0.0
        }
    }

    /// The room that is kept at the bottom of the window for the panels.
    ///
    /// The output of the task and the panel of the debugger cannot both be
    /// there at once: the taller one gets the room.
    fn bottom_panel_height(&self) -> f32 {
        let task = self.task_panel_height();
        let debug = if self.debug.is_some() {
            debug::DEBUG_PANEL_HEIGHT
        } else {
            0.0
        };
        task.max(debug)
    }

    /// Handles the keys of the task.
    ///
    /// Esc closes the output panel; the task keeps running. Ctrl+Shift+B is
    /// the key of a command, and the command route builds the project.
    fn handle_task_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        if self.task_panel && matches!(key, Key::Named(NamedKey::Escape)) {
            if self.terminal_input.is_some() {
                // The second Esc closes the terminal and stops the shell.
                self.close_terminal();
                self.task_panel = false;
                self.status = Some("终端关了".to_owned());
            } else {
                self.task_panel = false;
            }
            return true;
        }
        false
    }

    /// Starts the build task of the project the open file belongs to.
    ///
    /// The command is the one the manifest of the project says. A project
    /// that has no manifest has nothing to build, and the editor says so
    /// instead of doing nothing.
    fn start_build_task(&mut self) {
        if self.terminal_input.is_some() {
            // The terminal and the tasks share one place: a build would take
            // the place of the terminal.
            self.status = Some("终端开着呢，先把它关了（Esc）".to_owned());
            return;
        }
        if let Some(task) = self.task.as_ref()
            && task.is_running()
        {
            // One task at a time: two builds of the same project would fight
            // over the same files.
            self.status = Some("已经有任务在跑了".to_owned());
            return;
        }
        let Some(path) = self.panes[front_tab(&self.groups)]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())
        else {
            self.status = Some("这个文件不属于任何工程，没有可以跑的命令".to_owned());
            return;
        };
        let root = lsp::workspace_root(&path);
        let Some((program, args)) = task::build_command(&root) else {
            self.status = Some(format!("{} 里没有可以跑的命令", root.display()));
            return;
        };
        let title = format!("{program} {}", args.join(" "));
        match Task::start(&program, &args, &root) {
            Ok(task) => {
                self.task = Some(task);
                self.task_output = TaskOutput::default();
                self.task_title = title.clone();
                self.task_panel = true;
                self.status = Some(format!("正在跑：{title}"));
            }
            Err(error) => {
                eprintln!("could not start the task: {error}");
                self.status = Some(format!("任务起不来：{error}"));
            }
        }
    }

    /// Collects what the task has said.
    ///
    /// The task talks on a thread of its own, so what it has said is
    /// collected once per frame, the way the output of the agent is.
    fn pump_task(&mut self) -> bool {
        let Some(task) = self.task.as_ref() else {
            return false;
        };
        let output = task.output();
        if output == self.task_output {
            return false;
        }
        let state = output.state.clone();
        let lines = output.lines.len();
        self.task_output = output;
        if !matches!(state, TaskState::Running) {
            // The task is done: what it says is said once.
            if self.terminal_input.is_some() {
                // The shell is gone: the terminal goes with it.
                self.close_terminal();
                self.status = Some(format!("终端关了：{}", TaskPanel::state_text(&state)));
            } else {
                self.status = Some(TaskPanel::state_text(&state));
            }
        } else if lines > 0 {
            self.status = Some(format!("正在跑…（{lines} 行）"));
        }
        true
    }



    /// Opens the box that asks for the new name.
    ///
    /// The box starts with the name that is under the caret, so that the
    /// usual case is a change of a few letters.
    fn open_rename_box(&mut self) {
        if self.server.is_none() {
            self.status = Some("没有语言服务器，重命名不了".to_owned());
            return;
        }
        let caret = self.panes[front_tab(&self.groups)].editor.caret().0;
        let Some((start, end)) = rename::word_at_caret(self.panes[front_tab(&self.groups)].editor.buffer(), caret)
        else {
            self.status = Some("光标处没有可以重命名的名字".to_owned());
            return;
        };
        let name = self.panes[front_tab(&self.groups)]
            .editor
            .buffer()
            .slice(TextOffset(start)..TextOffset(end))
            .to_owned();
        self.rename_box = Some(RenameBox::new(name, start));
        self.take_the_keyboard_from_the_terminal();
        self.status = Some("输入新名字，回车确认，Esc 取消".to_owned());
    }

    /// Gives a key to the box that asks for the new name.
    ///
    /// While the box is open it takes every key: a name that is typed into
    /// the document is not a name.
    ///
    /// Returns `true` when the key belonged to the box.
    fn handle_rename_box_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        let Some(mut rename) = self.rename_box.take() else {
            return false;
        };

        let mut handled = true;
        let mut keep = true;
        match key {
            Key::Named(NamedKey::Escape) => {
                self.status = Some("取消了重命名".to_owned());
                keep = false;
            }
            Key::Named(NamedKey::Enter) => {
                keep = false;
                self.request_rename(&rename);
            }
            Key::Named(NamedKey::Backspace) => rename.backspace(),
            Key::Named(NamedKey::Delete) => rename.delete(),
            Key::Named(NamedKey::ArrowLeft) => rename.left(),
            Key::Named(NamedKey::ArrowRight) => rename.right(),
            Key::Named(NamedKey::Home) => rename.home(),
            Key::Named(NamedKey::End) => rename.end(),
            _ => {
                // The keys that change the text are the box's own.
                handled = rename.handle_key(key, text, self.modifiers.control_key());
            }
        }
        if keep {
            self.rename_box = Some(rename);
        }
        handled
    }

    /// Asks the server to rename the thing at the caret.
    ///
    /// The answer comes later: it is put into the buffers by
    /// [`Self::apply_rename`], when the window collects what the server has
    /// to say.
    fn request_rename(&mut self, rename: &RenameBox) {
        let new_name = rename.text().to_owned();
        if new_name.is_empty() {
            self.status = Some("新名字不能是空的".to_owned());
            return;
        }
        if rename.is_unchanged() {
            self.status = Some("名字没有变，没有重命名".to_owned());
            return;
        }
        let Some(uri) = self.panes[front_tab(&self.groups)].document_uri.clone() else {
            self.status = Some("这个文件没有语言服务器，重命名不了".to_owned());
            return;
        };
        let position = lsp::position_of(self.panes[front_tab(&self.groups)].editor.buffer(), rename.start());
        let outcome = self
            .server
            .as_ref()
            .map(|server| server.request_rename(&uri, position, &new_name));
        match outcome {
            Some(Ok(())) => {
                // The version is the version of the text the request is
                // about: an answer that comes back for a text that has
                // changed since then does not fit the text that is there
                // now.
                self.renaming = Some(RenameRequest {
                    uri,
                    version: self.panes[front_tab(&self.groups)].editor.version(),
                    new_name,
                });
                self.status = Some("正在重命名…".to_owned());
            }
            Some(Err(error)) => {
                eprintln!("could not ask the server to rename: {error}");
                self.status = Some(format!("重命名请求失败：{error}"));
            }
            None => self.status = Some("没有语言服务器，重命名不了".to_owned()),
        }
    }

    /// Puts the edits of a rename answer where they belong.
    ///
    /// The edits of one file are one change of that file: they go in as one
    /// transaction, so one undo takes the whole renaming back.
    ///
    /// A file that is not open in the editor is written to disk through the
    /// workspace service, which refuses to overwrite a file that changed
    /// behind the editor's back.
    ///
    /// Returns whether anything changed.
    /// Puts the edits of a rename answer where they belong.
    ///
    /// The edits of one file are one change of that file: they go in as one
    /// transaction, so one undo takes the whole renaming back.
    ///
    /// Returns whether anything changed.
    fn apply_rename(&mut self, edit: WorkspaceEdit, request: &RenameRequest) -> bool {
        let outcome = self.apply_workspace_edit(&edit, &request.uri, request.version, "重命名");
        let Some(reason) = outcome.reason else {
            let mut message = format!("已重命名为 {}", request.new_name);
            if outcome.files > 1 {
                message.push_str(&format!("（改了 {} 个文件", outcome.files));
                if outcome.written > 0 {
                    message.push_str(&format!("，其中 {} 个不在编辑器里，已写入磁盘", outcome.written));
                }
                message.push('）');
            }
            self.status = Some(message);
            return outcome.changed;
        };
        self.status = Some(reason);
        false
    }

    /// Puts the edits of a change where they belong.
    ///
    /// The edits of one file are one change of that file: they go in as one
    /// transaction, so one undo takes the whole change back.
    ///
    /// A file that is not open in the editor is written to disk through the
    /// workspace service, which refuses to overwrite a file that changed
    /// behind the editor's back.
    ///
    /// `uri` is the file the change was asked about, and `version` is the
    /// version of its text when the question was asked: a change that does
    /// not fit the text that is there now is not put in.
    ///
    /// `what` is how the change is called to the user: a renaming and a code
    /// action are the same work with different words.
    fn apply_workspace_edit(
        &mut self,
        edit: &WorkspaceEdit,
        uri: &str,
        version: u64,
        what: &str,
    ) -> EditOutcome {
        // The change is about the file that was asked about. The tab on
        // screen may be another one by now, and the file may be gone.
        let Some(pane) = self.pane_of_a_file(uri) else {
            return EditOutcome::failed(format!("{what}的那个文件已经不在编辑器里了"));
        };
        if version != self.panes[pane].editor.version() {
            return EditOutcome::failed(format!("文件在{what}期间变了，{what}结果没有用上"));
        }
        if !edit.resource_operations.is_empty() {
            return EditOutcome::failed("服务器要改文件的名字，这个编辑器还做不到".to_owned());
        }
        let files = edit.files();
        if files.is_empty() {
            return EditOutcome::failed("没有要改的地方".to_owned());
        }

        // Every file is worked out before anything is changed: a change that
        // lands in the wrong place is worse than no change at all.
        let mut planned: Vec<(String, FileChanges)> = Vec::new();
        for file in &files {
            let edits = edit.edits_of(file);
            if edits.is_empty() {
                continue;
            }
            let Some(changes) = self.changes_of_a_file(file, &edits) else {
                return EditOutcome::failed(format!(
                    "{what}结果和 {} 对不上，没有改",
                    codeweaver_lsp::protocol::uri_to_path(file)
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| file.clone())
                ));
            };
            planned.push((file.clone(), changes));
        }
        if planned.is_empty() {
            return EditOutcome::failed("没有要改的地方".to_owned());
        }

        // The files that are not open in the editor go to disk first: if one
        // of them cannot be written, nothing is changed in the editor either.
        let mut written = 0usize;
        for (file, changes) in &planned {
            if self.pane_of_a_file(file).is_some() {
                continue;
            }
            if let Err(error) = self.write_a_file(file, changes) {
                return EditOutcome::failed(format!("{what}没有做完：{error}"));
            }
            written += 1;
        }

        // The files that are open in the editor change in their buffers.
        let mut changed_files = 0usize;
        for (file, changes) in &planned {
            let Some(index) = self.pane_of_a_file(file) else {
                continue;
            };
            if let Err(error) = self.panes[index].editor.replace_edits(changes.clone()) {
                return EditOutcome::failed(format!("{what}结果写不进去：{error}"));
            }
            changed_files += 1;
        }
        if changed_files > 0 {
            self.sync_dirty_flags();
            // The server has to hear about the new text: its answers are
            // about the text that is on screen and not about the text as the
            // file was opened.
            for index in 0..self.panes.len() {
                self.tell_server_about_the_change_of(index);
            }
        }
        EditOutcome {
            changed: changed_files + written > 0,
            files: changed_files + written,
            written,
            reason: None,
        }
    }

    /// The tab whose file is the one with the given URI.
    fn pane_of_a_file(&self, uri: &str) -> Option<usize> {
        self.panes
            .iter()
            .position(|pane| pane.document_uri.as_deref() == Some(uri))
    }

    /// The changes of one file, as ranges of its text.
    ///
    /// The file is the one that is open in the editor, or the one on disk
    /// when it is not open. A place that is not in the file means the answer
    /// is about a different text, and the file is not changed at all.
    fn changes_of_a_file(
        &self,
        uri: &str,
        edits: &[TextEdit],
    ) -> Option<Vec<(std::ops::Range<TextOffset>, String)>> {
        if let Some(index) = self.pane_of_a_file(uri) {
            return lsp::rename_changes(self.panes[index].editor.buffer(), edits);
        }
        let path = codeweaver_lsp::protocol::uri_to_path(uri)?;
        let document = self.workspace.open(&path).ok()?;
        lsp::rename_changes(&codeweaver_text::Buffer::new(document.saved_text()), edits)
    }

    /// Writes the changes of a file that is not open in the editor to disk.
    fn write_a_file(
        &mut self,
        uri: &str,
        changes: &[(std::ops::Range<TextOffset>, String)],
    ) -> Result<(), String> {
        let Some(path) = codeweaver_lsp::protocol::uri_to_path(uri) else {
            return Err(format!("{uri} 不是编辑器能写的文件"));
        };
        let mut document = self
            .workspace
            .open(&path)
            .map_err(|error| format!("{} 读不了：{error}", path.display()))?;
        let mut buffer = codeweaver_text::Buffer::new(document.saved_text());
        // The edits have to be in the order of the text, and they may not
        // overlap.
        let mut sorted = changes.to_vec();
        sorted.sort_by_key(|(range, _)| (range.start, range.end));
        let text_edits = sorted
            .iter()
            .map(|(range, text)| {
                codeweaver_text::TextEdit::new(range.clone(), text.clone())
            })
            .collect();
        let transaction = codeweaver_text::Transaction::new(buffer.version(), text_edits);
        buffer
            .apply(transaction)
            .map_err(|error| format!("{} 改不了：{error}", path.display()))?;
        self.workspace
            .save(&mut document, &buffer.text())
            .map_err(|error| format!("{} 写不进去：{error}", path.display()))
    }

    /// Opens the find bar when the user asks for it.
    ///
    /// Ctrl+H finds and replaces in the file that is open. Ctrl+F is the
    /// key of a command, and the command route opens the bar.
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
        // Alt+C turns the case sensitivity on and off.
        //
        // The key may have no name on a layout with a Chinese input method,
        // so the letter is taken from the virtual key code. Nothing else is
        // written into the bar while Alt is held.
        if self.modifiers.alt_key()
            && let Some(letter) = key_letter(key)
        {
            if letter.eq_ignore_ascii_case(&'c') {
                find.toggle_case_sensitive(&self.panes[front_tab(&self.groups)].editor);
                let count = find.match_count();
                self.status = Some(format!("{count} 处匹配"));
            }
            if keep {
                self.find = Some(find);
            }
            return true;
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                // The bar closes and the editor takes the keyboard back.
                keep = false;
            }
            Key::Named(NamedKey::Enter) => {
                if self.modifiers.control_key() && self.modifiers.alt_key() {
                    let count = find.replace_all(&mut self.panes[front_tab(&self.groups)].editor);
                    self.status = Some(format!("替换了 {count} 处"));
                } else if self.modifiers.alt_key() {
                    let count = find.select_all_matches(&mut self.panes[front_tab(&self.groups)].editor);
                    self.status = Some(format!("选中了 {count} 处"));
                } else if self.modifiers.shift_key() {
                    find.previous_match();
                } else {
                    find.next_match();
                }
            }
            Key::Named(NamedKey::Tab) => find.toggle_field(),
            Key::Named(NamedKey::Backspace) => find.backspace(&self.panes[front_tab(&self.groups)].editor),
            Key::Named(NamedKey::Space) => find.type_text(&self.panes[front_tab(&self.groups)].editor, " "),
            Key::Character(character) if !self.modifiers.control_key() => {
                let character = character.as_ref();
                if character.chars().any(char::is_control) {
                    // The key did not produce text of its own; the text of
                    // the event is what the user typed.
                    if let Some(typed) = text.filter(|typed| !typed.chars().any(char::is_control)) {
                        find.type_text(&self.panes[front_tab(&self.groups)].editor, typed);
                    } else {
                        handled = false;
                    }
                } else {
                    find.type_text(&self.panes[front_tab(&self.groups)].editor, character);
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
        find.open(&mut self.panes[front_tab(&self.groups)].editor, replacing);
        self.find = Some(find);
    }

    /// Opens the find bar and searches for the given text.
    ///
    /// This is what the editor does when it is started with a search, so the
    /// find bar can be looked at without a keyboard.
    pub fn open_find_with(&mut self, query: &str) {
        let mut find = self.find.take().unwrap_or_default();
        find.open(&mut self.panes[front_tab(&self.groups)].editor, false);
        find.type_text(&self.panes[front_tab(&self.groups)].editor, query);
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
        let mut status = match self.panes[front_tab(&self.groups)].document.as_ref() {
            Some(document) => {
                let dirty = if document.is_dirty(&self.panes[front_tab(&self.groups)].editor.text()) {
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
        if self.panes[front_tab(&self.groups)].errors > 0 || self.panes[front_tab(&self.groups)].warnings > 0 {
            status.push_str(&format!("  ·  ✕ {}  ⚠ {}", self.panes[front_tab(&self.groups)].errors, self.panes[front_tab(&self.groups)].warnings));
        }
        // A file that is too big is not coloured, and the user is told why:
        // a document that quietly loses its colours looks broken.
        if self.panes[front_tab(&self.groups)].degraded {
            let policy = self.panes[front_tab(&self.groups)].policy;
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

    /// The columns of the window, and what each of them shows.
    ///
    /// A column has its own tabs: the tabs of one column are not the tabs of
    /// the other, and the tab in front of it is the file the column shows.
    fn columns_of_the_window(&self) -> Vec<ColumnData> {
        let mut columns = Vec::new();
        for index in 0..self.groups.len() {
            let Some(tab) = self.groups.front_of(index) else {
                continue;
            };
            let tabs: Vec<(Tab, bool)> = self
                .groups
                .tabs_of(index)
                .iter()
                .filter_map(|place| {
                    let tab = self.tabs.tabs().get(*place)?;
                    Some((tab.clone(), tab.dirty))
                })
                .collect();
            let active = self
                .groups
                .tabs_of(index)
                .iter()
                .position(|place| *place == tab)
                .unwrap_or(0);
            // The breakpoints in the margin are the ones of the file that
            // this column shows: a column shows another file, and the
            // breakpoints of that file belong in it.
            let breakpoints = self
                .uri_of_a_pane(tab)
                .map(|uri| self.breakpoint_marks_of(&uri))
                .unwrap_or_default();
            columns.push(ColumnData {
                tab,
                tabs,
                active,
                breakpoints,
            });
        }
        columns
    }

    /// The breakpoints of a file, as they are drawn in its margin.
    fn breakpoint_marks_of(&self, uri: &str) -> Vec<BreakpointMark> {
        let conditions = self.breakpoints.conditions_of(uri);
        self.breakpoints
            .lines_of(uri)
            .into_iter()
            .map(|line| BreakpointMark {
                line,
                conditional: conditions.contains_key(&line),
            })
            .collect()
    }

    /// Where the parts of the window are.
    ///
    /// The drawing and the clicking both go through here: a tab that is
    /// drawn in one place and clicked in another is a tab that cannot be
    /// clicked.
    fn window_layout(&self, size: Size) -> WindowLayout {
        WindowLayout::of(
            size,
            self.bottom_panel_height(),
            self.sidebar.is_open(),
            self.groups.len(),
        )
    }

    /// The numbers that are drawn on the activities.
    ///
    /// The numbers come from the real things: the hits of the search, the
    /// files git knows about, the breakpoints.
    fn badges(&self) -> Badges {
        Badges {
            search: self.search_hits.len(),
            source_control: self
                .source_control
                .as_ref()
                .map(|status| status.entries.len())
                .unwrap_or(0),
            debug: self
                .breakpoints
                .all()
                .iter()
                .map(|(_, lines)| lines.len())
                .sum(),
        }
    }

    /// What the status bar at the bottom of the window says.
    ///
    /// The left says what is open and what is wrong with it, the middle says
    /// what the editor is doing inside, and the right says where the caret
    /// is.
    fn status_bar_text(&self) -> StatusBar {
        let pane = &self.panes[front_tab(&self.groups)];
        let (line, column) = self.caret_position();
        // A window that is split says which of its columns has the keyboard.
        let columns = self.groups.len();
        let place = if columns > 1 {
            format!(" · 栏 {}/{}", self.groups.active() + 1, columns)
        } else {
            String::new()
        };
        // A workspace says which one is open: the tree of files is its tree.
        let workspace = match self.open_workspace.as_ref() {
            Some(workspace) => format!(" · 工作区 {}", workspace.name),
            None => String::new(),
        };
        // The branch the repository is on is said first: where the program
        // is, is something a person has to see without asking.
        let branch = match self.current_branch.as_deref() {
            Some(name) => format!("分支 {name} · "),
            None => String::new(),
        };
        StatusBar {
            left: format!("{branch}{}", self.status_line()),
            middle: format!(
                "Buffer v{} · IME {} · Undo {} · Redo {} · 光标 {}",
                pane.editor.version(),
                if self.input.ime_enabled() { "ON" } else { "READY" },
                pane.editor.can_undo(),
                pane.editor.can_redo(),
                pane.editor.cursor_count(),
            ),
            right: format!(
                "行 {line}，列 {column} · {}{place}{workspace}",
                self.zoom.label()
            ),
        }
    }

    /// Where the caret is: the line and the column, counting from one.
    fn caret_position(&self) -> (usize, usize) {
        let pane = &self.panes[front_tab(&self.groups)];
        let buffer = pane.editor.buffer();
        let caret = pane.editor.caret().0;
        let line = buffer.line_of_offset(caret);
        let start = buffer.line_range(line).start.0;
        let column = buffer.slice(TextOffset(start)..TextOffset(caret)).chars().count();
        (line + 1, column + 1)
    }

    /// The title of the section of the side bar that is on screen.
    ///
    /// The section of the files is the resource manager, unless a workspace is
    /// open: then the title is the name of the workspace, so that the person
    /// sees which project the files belong to.
    fn sidebar_title(&self) -> String {
        match (self.open_workspace.as_ref(), self.sidebar.section) {
            (Some(workspace), Activity::Files) => workspace.name.clone(),
            _ => self.sidebar.section.label().to_owned(),
        }
    }

    /// The rows of the section of the side bar that is on screen.
    ///
    /// Every section shows what belongs to it. A section that has nothing to
    /// show says why: an empty panel is a panel that looks broken.
    fn sidebar_rows(&self, panel: Rect) -> Vec<SideBarRow> {
        match self.sidebar.section {
            Activity::Files => match self.tree.as_ref() {
                Some(tree) => sidebar::tree_rows(tree.nodes(), panel),
                None => sidebar::list_rows(panel, vec![sidebar::Row::text(
                    "没有工程",
                    "打开一个文件夹，里面才有文件",
                )]),
            },
            Activity::Search => {
                if self.search_hits.is_empty() {
                    return sidebar::list_rows(panel, vec![sidebar::Row::text(
                        "还没有搜过",
                        "Ctrl+Shift+F 搜索整个工程",
                    )]);
                }
                let rows = self
                    .search_hits
                    .iter()
                    .map(|hit| sidebar::Row {
                        title: hit
                            .path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("?")
                            .to_owned(),
                        detail: format!("第 {} 行", hit.line),
                        depth: 0,
                        directory: false,
                        expanded: false,
                        action: SideBarAction::OpenFileAt {
                            path: hit.path.clone(),
                            line: hit.line,
                        },
                        rect: Rect::new(0.0, 0.0, 0.0, 0.0),
                    })
                    .collect();
                sidebar::list_rows(panel, rows)
            }
            Activity::SourceControl => match self.source_control.as_ref() {
                Some(status) => {
                    let rows = status
                        .entries
                        .iter()
                        .map(|entry| sidebar::Row {
                            title: entry.path.clone(),
                            detail: entry.label(),
                            depth: 0,
                            directory: false,
                            expanded: false,
                            action: SideBarAction::None,
                            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
                        })
                        .collect();
                    sidebar::list_rows(panel, rows)
                }
                None => sidebar::list_rows(panel, vec![sidebar::Row::text(
                    "没有改动",
                    "Ctrl+Shift+G 问 git 要改动",
                )]),
            },
            Activity::Debug => {
                let mut rows = vec![sidebar::Row::text(
                    self.debug
                        .as_ref()
                        .map(|debug| debug.state.text())
                        .unwrap_or("没有调试"),
                    "",
                )];
                if let Some(uri) = self.uri_of_the_open_file() {
                    for line in self.breakpoints.lines_of(&uri) {
                        rows.push(sidebar::Row {
                            title: format!("第 {line} 行"),
                            detail: if self.breakpoints.condition_of(&uri, line).is_some() {
                                "有条件".to_owned()
                            } else {
                                String::new()
                            },
                            depth: 0,
                            directory: false,
                            expanded: false,
                            action: SideBarAction::ToggleBreakpoint(line),
                            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
                        });
                    }
                }
                sidebar::list_rows(panel, rows)
            }
            Activity::Extensions => sidebar::list_rows(panel, vec![sidebar::Row::text(
                "扩展",
                "扩展生态是 V0.9.0 的事，这一版还没有",
            )]),
        }
    }

    /// Starts an agent turn with the editor's current text as the prompt.
    ///
    /// The text the user chose as context travels with the prompt as
    /// separate blocks, so the agent knows which is the request and which is
    /// the material.
    pub fn start_agent_turn(&mut self) {
        let prompt = self.panes[front_tab(&self.groups)].editor.text();
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
            if let Some(document) = self.panes[front_tab(&self.groups)].document.as_ref()
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

    /// Opens the panel that shows what git knows about the files.
    ///
    /// The file that is open decides which repository is meant: the root of
    /// the repository is the folder that holds the `.git` directory.
    fn open_git_panel(&mut self) {
        self.open_the_git_panel(GitView::Files);
    }

    /// Opens the panel of the changes on the page that is asked for.
    fn open_the_git_panel(&mut self, view: GitView) {
        // Only one thing at a time: the box that asks for a new name and the
        // find bar give up their place to the panel of the changes.
        self.rename_box = None;
        self.find = None;
        let Some(path) = self.panes[front_tab(&self.groups)]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())
        else {
            self.status = Some("这个文件不在任何工程里".to_owned());
            return;
        };
        let Some(root) = git::repository_root(&path) else {
            self.status = Some("这个文件不在 git 仓库里".to_owned());
            return;
        };
        match self.load_git_panel(&root, view) {
            Ok(panel) => {
                let count = panel.entries.len();
                self.take_the_keyboard_from_the_terminal();
                self.remember_the_branch(&root);
                self.git = Some(panel);
                self.status = Some(match view {
                    GitView::Files => format!("改动面板打开了（{count} 个文件）"),
                    GitView::Commits => "提交记录打开了".to_owned(),
                    GitView::Branches => "分支与远端打开了".to_owned(),
                });
            }
            Err(error) => {
                eprintln!("could not ask git about the files: {error}");
                self.status = Some(format!("git 问不了：{error}"));
            }
        }
    }

    /// Reads what git knows and puts it in the panel.
    fn load_git_panel(&mut self, root: &Path, view: GitView) -> Result<GitPanel, String> {
        let status = git::status(root)?;
        // What git knows is kept, so that the badge on the activity bar can
        // say how many files changed without asking git again.
        self.source_control = Some(status.clone());
        let mut panel = GitPanel::new(root.to_path_buf(), status.entries);
        panel.view = view;
        if view == GitView::Branches {
            panel.load_branches()?;
        }
        self.load_git_diff(&mut panel)?;
        Ok(panel)
    }

    /// Reads the diff of the file that is picked, or of the commit that is
    /// picked.
    fn load_git_diff(&mut self, panel: &mut GitPanel) -> Result<(), String> {
        if panel.view() == GitView::Commits {
            let Some(commit) = panel.selected_commit() else {
                panel.diff.clear();
                return Ok(());
            };
            panel.diff = git::show(&panel.root, &commit.id)?;
            return Ok(());
        }
        if panel.view() == GitView::Branches {
            // The branch view says what is known about the branch or the
            // remote that is picked, and no difference.
            panel.diff.clear();
            return Ok(());
        }
        let Some(entry) = panel.selected_entry() else {
            panel.diff.clear();
            return Ok(());
        };
        panel.diff = git::diff(&panel.root, &entry.path, entry.staged)?;
        Ok(())
    }

    /// Remembers which branch the repository is on.
    ///
    /// Git is not asked every frame: the branch is read when the file
    /// changes and when the panel of the changes is opened.
    fn remember_the_branch(&mut self, root: &Path) {
        self.current_branch = git::current_branch(root)
            .ok()
            .filter(|name| !name.is_empty());
    }

    /// The remote that the session uses.
    ///
    /// The branch that is checked out follows a branch on a remote, and that
    /// remote is the one that is used; a branch that follows nothing uses the
    /// first remote of the repository.
    fn the_remote_of(&self, panel: &GitPanel) -> Option<String> {
        let current = panel.branch_items.iter().find_map(|item| match item {
            git::BranchItem::Branch(branch) if branch.current => Some(branch),
            _ => None,
        });
        if let Some(remote) = current.and_then(|branch| branch.remote_of()) {
            return Some(remote.to_owned());
        }
        panel.branch_items.iter().find_map(|item| match item {
            git::BranchItem::Remote(remote) => Some(remote.name.clone()),
            _ => None,
        })
    }

    /// The root of the repository the session is about.
    ///
    /// The panel of the changes knows the repository; without the panel, the
    /// file that is open decides which repository is meant.
    fn the_root_of_the_session(&self) -> Option<PathBuf> {
        self.git
            .as_ref()
            .map(|panel| panel.root.clone())
            .or_else(|| self.the_repository_of_the_open_file())
    }

    /// The root of the repository of the file that is open.
    fn the_repository_of_the_open_file(&self) -> Option<PathBuf> {
        let path = self.panes[front_tab(&self.groups)]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())?;
        git::repository_root(&path)
    }

    /// Whether a panel that takes every key is open.
    ///
    /// A panel opens over the window and the keys go to the one that is on
    /// top: two panels open at once means the one below is stuck and the
    /// person does not know which one he is typing into.
    fn a_modal_is_open(&self) -> bool {
        self.rename_box.is_some()
            || self.command_box.is_some()
            || self.palette.is_some()
            || self.developer.is_some()
            || self.search.is_some()
            || self.breakpoint_condition.is_some()
            || self.debug_expression.is_some()
            || self.git_box.is_some()
    }

    /// Handles the keys of the panel of the changes.
    ///
    /// Ctrl+Shift+G opens the panel. While the panel is open it takes every
    /// key: nothing may happen behind it that the user cannot see.
    fn handle_git_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        // The box that asks git something takes every key while it is open.
        if self.git_box.is_some() {
            return self.handle_git_box_key(key, text);
        }
        // The box that asks for the message of a commit takes every key
        // while it is open.
        if self.commit_box.is_some() {
            return self.handle_commit_box_key(key, text);
        }

        // The panel is not open: Ctrl+Shift+G opens it.
        if self.git.is_none() {
            // A panel that takes every key does not open over another one.
            if self.a_modal_is_open() {
                return false;
            }
            if !(self.modifiers.control_key() && self.modifiers.shift_key()) {
                return false;
            }
            let Some(letter) = key_letter(key) else {
                return false;
            };
            if !letter.eq_ignore_ascii_case(&'g') {
                return false;
            }
            self.open_git_panel();
            return true;
        }

        // The panel is open: it takes every key.
        let Some(mut panel) = self.git.take() else {
            return false;
        };
        // Whether the diff of the file that is picked is read again.
        let mut reread = true;
        // The keys are in a block of their own: a key that does nothing
        // leaves the block instead of the function, so that the panel is
        // put back and does not close under the hand of the person.
        'keys: {
        match key {
            Key::Named(NamedKey::Escape) => {
                if panel.view() != GitView::Files {
                    // The commits and the branches are pages of the panel:
                    // the escape key goes back to the files, and does not
                    // close the panel.
                    panel.view = GitView::Files;
                    panel.selected = 0;
                    self.status = Some("回到改过的文件".to_owned());
                } else {
                    self.status = Some("关掉了改动面板".to_owned());
                    // The panel goes away and is not put back.
                    return true;
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                panel.select_next();
            }
            Key::Named(NamedKey::ArrowUp) => {
                panel.select_previous();
            }
            Key::Named(NamedKey::Enter) => {
                // In the branch view the enter key goes to the branch that
                // is picked.
                if panel.view() == GitView::Branches {
                    self.go_to_the_branch(&mut panel);
                    reread = false;
                }
            }
            Key::Character(character) if !self.modifiers.control_key() => {
                let character = character.as_ref();
                let letter = character
                    .chars()
                    .next()
                    .map(|letter| letter.to_ascii_lowercase());
                // The page tells what the keys mean: the files are only
                // touched on their own page.
                let view = panel.view();
                match letter {
                    Some('s') => {
                        // The file is staged, and the panel says so at once:
                        // staging is something a person has to see.
                        if view != GitView::Files {
                            break 'keys;
                        }
                        let picked = panel.selected_entry().map(|entry| entry.path.clone());
                        let Some(path) = picked else {
                            self.status = Some("没有文件可以暂存".to_owned());
                            break 'keys;
                        };
                        let root = panel.root.clone();
                        match git::stage(&root, &path) {
                            Ok(()) => match self.load_git_panel(&root, GitView::Files) {
                                Ok(fresh) => {
                                    panel = fresh;
                                    reread = false;
                                    self.status = Some(format!("暂存了 {path}"));
                                }
                                Err(error) => {
                                    self.status =
                                        Some(format!("暂存了 {path}，但改动读不了：{error}"));
                                }
                            },
                            Err(error) => {
                                self.status = Some(format!("暂存不了：{error}"));
                            }
                        }
                    }
                    Some('c') => {
                        if view != GitView::Files {
                            break 'keys;
                        }
                        self.commit_box = Some(RenameBox::message("提交说明"));
                        self.status = Some("写提交说明，回车确认，Esc 取消".to_owned());
                    }
                    Some('u') => {
                        // On the branch page the key means something else:
                        // the branch is updated from the remote.
                        if view == GitView::Branches {
                            self.update_the_branch(&mut panel);
                            reread = false;
                            break 'keys;
                        }
                        // The file goes out of the staging area: the file
                        // itself is not touched, only the index is.
                        if view != GitView::Files {
                            break 'keys;
                        }
                        let Some(entry) = panel.selected_entry() else {
                            self.status = Some("没有文件可以取消暂存".to_owned());
                            break 'keys;
                        };
                        let path = entry.path.clone();
                        let staged = entry.staged;
                        if !staged {
                            self.status = Some(format!("{path} 没有暂存"));
                            break 'keys;
                        }
                        let root = panel.root.clone();
                        match git::unstage(&root, &path) {
                            Ok(()) => match self.load_git_panel(&root, GitView::Files) {
                                Ok(fresh) => {
                                    panel = fresh;
                                    reread = false;
                                    self.status = Some(format!("取消暂存 {path}"));
                                }
                                Err(error) => {
                                    self.status =
                                        Some(format!("取消暂存了 {path}，但改动读不了：{error}"));
                                }
                            },
                            Err(error) => {
                                self.status = Some(format!("取消暂存不了：{error}"));
                            }
                        }
                    }
                    Some('l') => {
                        // The commits of the repository: the panel becomes a
                        // page of them.
                        if view != GitView::Files {
                            break 'keys;
                        }
                        let root = panel.root.clone();
                        match git::commits(&root, 30) {
                            Ok(commits) => {
                                let count = commits.len();
                                panel.commits = commits;
                                panel.view = GitView::Commits;
                                panel.selected = 0;
                                self.status = Some(format!("{count} 个提交"));
                            }
                            Err(error) => {
                                self.status = Some(format!("提交记录读不了：{error}"));
                            }
                        }
                    }
                    Some('b') => {
                        // The branches and the remotes: the panel becomes a
                        // page of them, and back again.
                        if view == GitView::Branches {
                            panel.view = GitView::Files;
                            panel.selected = 0;
                            self.status = Some("回到改过的文件".to_owned());
                        } else {
                            match panel.load_branches() {
                                Ok(()) => {
                                    panel.view = GitView::Branches;
                                    panel.selected = 0;
                                    self.status = Some("分支与远端".to_owned());
                                }
                                Err(error) => {
                                    self.status = Some(format!("分支读不了：{error}"));
                                }
                            }
                        }
                    }
                    Some('n') => {
                        // A new branch: the box asks for its name.
                        if view != GitView::Branches {
                            break 'keys;
                        }
                        self.git_box = Some(GitBox::new_branch());
                        self.status =
                            Some("写新分支的名字，回车确认，Esc 取消".to_owned());
                    }
                    Some('f') => {
                        // What the remotes have is taken; nothing here
                        // changes.
                        if view != GitView::Branches {
                            break 'keys;
                        }
                        self.fetch_the_remotes(&mut panel);
                        reread = false;
                    }
                    Some('p') => {
                        // Pushing cannot be undone from here, so it is asked
                        // for first.
                        if view != GitView::Branches {
                            break 'keys;
                        }
                        self.ask_to_push(&panel);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        }
        // The diff of the file that is picked is read again.
        if reread
            && let Err(error) = self.load_git_diff(&mut panel)
        {
            self.status = Some(format!("差别读不了：{error}"));
        }
        self.git = Some(panel);
        true
    }

    /// Goes to the branch that is picked.
    fn go_to_the_branch(&mut self, panel: &mut GitPanel) {
        let Some(branch) = panel.selected_branch().cloned() else {
            self.status = Some("挑的是远端，不是分支".to_owned());
            return;
        };
        if branch.current {
            self.status = Some(format!("已经在 {} 上了", branch.name));
            return;
        }
        let root = panel.root.clone();
        // A branch that is on a remote is not here yet: a local branch that
        // follows it is made, and the program goes to that one.
        let went = if branch.remote {
            git::switch_to_a_remote(&root, &branch.name)
                .map(|name| format!("{name}（跟着 {}）", branch.name))
        } else {
            git::switch_to(&root, &branch.name).map(|()| branch.name.clone())
        };
        match went {
            Ok(where_to) => {
                self.remember_the_branch(&root);
                if let Err(error) = panel.load_branches() {
                    self.status = Some(format!("切到 {where_to} 了，但分支读不了：{error}"));
                    return;
                }
                self.status = Some(format!("切到 {where_to} 了"));
            }
            Err(error) => self.status = Some(format!("切不到 {}：{error}", branch.name)),
        }
    }

    /// Makes a branch and goes to it.
    fn make_the_branch(&mut self, name: &str) {
        let Some(root) = self.the_root_of_the_session() else {
            self.status = Some("不知道在哪儿建分支".to_owned());
            return;
        };
        match git::create_branch(&root, name) {
            Ok(()) => {
                self.remember_the_branch(&root);
                if let Some(panel) = self.git.as_mut()
                    && let Err(error) = panel.load_branches()
                {
                    self.status = Some(format!("建了 {name}，但分支读不了：{error}"));
                    return;
                }
                self.status = Some(format!("建了 {name}，在上面了"));
            }
            Err(error) => self.status = Some(format!("建不了 {name}：{error}")),
        }
    }

    /// Takes what the remotes have, without changing anything here.
    fn fetch_the_remotes(&mut self, panel: &mut GitPanel) {
        let root = panel.root.clone();
        let remote = self.the_remote_of(panel);
        match git::fetch(&root, remote.as_deref()) {
            Ok(said) => {
                if let Err(error) = panel.load_branches() {
                    self.status = Some(format!("取回完了，但分支读不了：{error}"));
                    return;
                }
                let said = said.trim();
                self.status = Some(if said.is_empty() {
                    "取回完了".to_owned()
                } else {
                    format!("取回完了：{said}")
                });
            }
            Err(error) => self.status = Some(format!("取回不了：{error}")),
        }
    }

    /// Brings what the remote has into the branch that is checked out.
    fn update_the_branch(&mut self, panel: &mut GitPanel) {
        let root = panel.root.clone();
        let remote = self.the_remote_of(panel);
        let branch = panel.current_branch.clone();
        let branch = (!branch.is_empty()).then_some(branch);
        match git::pull(&root, remote.as_deref(), branch.as_deref()) {
            Ok(said) => {
                if let Err(error) = panel.load_branches() {
                    self.status = Some(format!("更新完了，但分支读不了：{error}"));
                    return;
                }
                let said = said.trim();
                self.status = Some(if said.is_empty() {
                    "更新完了".to_owned()
                } else {
                    format!("更新完了：{said}")
                });
            }
            Err(error) => self.status = Some(format!("更新不了：{error}")),
        }
    }

    /// Asks whether the branch may be pushed.
    ///
    /// A push cannot be taken back from here, so it is asked for first: the
    /// box says where the branch goes, and the push only happens when the
    /// answer is given.
    fn ask_to_push(&mut self, panel: &GitPanel) {
        let Some(remote) = self.the_remote_of(panel) else {
            self.status = Some("这个仓库没有远端，推不出去".to_owned());
            return;
        };
        let branch = panel.current_branch.clone();
        if branch.is_empty() {
            self.status = Some("还没有分支，推不出去".to_owned());
            return;
        }
        self.git_box = Some(GitBox::confirm_push(&remote, &branch));
        self.status = Some(format!(
            "推到 {remote} 的 {branch}？回车确认，Esc 取消"
        ));
    }

    /// Pushes the branch to the remote.
    fn push_the_branch(&mut self, remote: &str, branch: &str) {
        let Some(root) = self.the_root_of_the_session() else {
            self.status = Some("不知道往哪儿推".to_owned());
            return;
        };
        match git::push(&root, Some(remote), branch) {
            Ok(said) => {
                let said = said.trim();
                self.status = Some(if said.is_empty() {
                    format!("推到 {remote} 的 {branch} 了")
                } else {
                    format!("推到 {remote} 的 {branch} 了：{said}")
                });
            }
            Err(error) => self.status = Some(format!("推不出去：{error}")),
        }
    }

    /// Gives a key to the box that asks git something.
    ///
    /// The box is a thing of its own: a key that means nothing to it is
    /// swallowed, so that the editor behind it never sees it.
    fn handle_git_box_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        let Some(mut asking) = self.git_box.take() else {
            return false;
        };
        // Whether the box stays open.
        let mut keep = true;
        match key {
            Key::Named(NamedKey::Escape) => {
                self.status = Some("算了".to_owned());
                keep = false;
            }
            Key::Named(NamedKey::Enter) => {
                keep = false;
                match asking.asking.clone() {
                    GitAsking::NewBranch => {
                        let name = asking.input.text().trim().to_owned();
                        if name.is_empty() {
                            self.status = Some("分支要有名字".to_owned());
                        } else {
                            self.make_the_branch(&name);
                        }
                    }
                    GitAsking::Push { remote, branch } => {
                        self.push_the_branch(&remote, &branch);
                    }
                }
            }
            Key::Named(NamedKey::Backspace) => asking.input.backspace(),
            Key::Named(NamedKey::Delete) => asking.input.delete(),
            Key::Named(NamedKey::ArrowLeft) => asking.input.left(),
            Key::Named(NamedKey::ArrowRight) => asking.input.right(),
            Key::Named(NamedKey::Home) => asking.input.home(),
            Key::Named(NamedKey::End) => asking.input.end(),
            Key::Character(character) if !self.modifiers.control_key() => {
                let character = character.as_ref();
                if let Some(typed) = text.filter(|typed| !typed.chars().any(char::is_control)) {
                    asking.input.type_text(typed);
                } else {
                    asking.input.type_text(character);
                }
            }
            _ => {}
        }
        if keep {
            self.git_box = Some(asking);
        }
        true
    }

    /// Asks whether the branch may be pushed, from the command.
    fn ask_to_push_from_the_command(&mut self) {
        let Some(root) = self.the_repository_of_the_open_file() else {
            self.status = Some("这个文件不在 git 仓库里".to_owned());
            return;
        };
        let Some(branch) = self.current_branch.clone().filter(|name| !name.is_empty())
        else {
            self.status = Some("还没有分支，推不出去".to_owned());
            return;
        };
        let remotes = git::remotes(&root).unwrap_or_default();
        let Some(remote) = remotes.first().map(|remote| remote.name.clone()) else {
            self.status = Some("这个仓库没有远端，推不出去".to_owned());
            return;
        };
        self.git_box = Some(GitBox::confirm_push(&remote, &branch));
        self.status = Some(format!("推到 {remote} 的 {branch}？回车确认，Esc 取消"));
    }

    /// Takes what the remotes have, from the command.
    fn fetch_from_the_command(&mut self) {
        let Some(root) = self.the_repository_of_the_open_file() else {
            self.status = Some("这个文件不在 git 仓库里".to_owned());
            return;
        };
        match git::fetch(&root, None) {
            Ok(said) => {
                let said = said.trim();
                self.status = Some(if said.is_empty() {
                    "取回完了".to_owned()
                } else {
                    format!("取回完了：{said}")
                });
            }
            Err(error) => self.status = Some(format!("取回不了：{error}")),
        }
    }

    /// Brings what the remote has in, from the command.
    fn update_from_the_command(&mut self) {
        let Some(root) = self.the_repository_of_the_open_file() else {
            self.status = Some("这个文件不在 git 仓库里".to_owned());
            return;
        };
        let remote = git::remotes(&root)
            .ok()
            .and_then(|remotes| remotes.first().map(|remote| remote.name.clone()));
        let branch = self.current_branch.clone().filter(|name| !name.is_empty());
        match git::pull(&root, remote.as_deref(), branch.as_deref()) {
            Ok(said) => {
                let said = said.trim();
                self.status = Some(if said.is_empty() {
                    "更新完了".to_owned()
                } else {
                    format!("更新完了：{said}")
                });
            }
            Err(error) => self.status = Some(format!("更新不了：{error}")),
        }
    }

    /// Gives a key to the box that asks for the message of a commit.
    ///
    /// The box is a thing of its own: a key that means nothing to it is
    /// swallowed, so that the editor behind it never sees it.
    fn handle_commit_box_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        let Some(mut commit) = self.commit_box.take() else {
            return false;
        };
        // Whether the box stays open.
        let mut keep = true;
        match key {
            Key::Named(NamedKey::Escape) => {
                self.status = Some("取消了提交".to_owned());
                keep = false;
            }
            Key::Named(NamedKey::Enter) => {
                keep = false;
                self.commit_staged(commit.text());
            }
            Key::Named(NamedKey::Backspace) => commit.backspace(),
            Key::Named(NamedKey::Delete) => commit.delete(),
            Key::Named(NamedKey::ArrowLeft) => commit.left(),
            Key::Named(NamedKey::ArrowRight) => commit.right(),
            Key::Named(NamedKey::Home) => commit.home(),
            Key::Named(NamedKey::End) => commit.end(),
            // The window system names the space bar instead of making it a
            // character: without this arm no message could have a word in it.
            Key::Named(NamedKey::Space) => commit.type_text(" "),
            Key::Character(character) if !self.modifiers.control_key() => {
                let character = character.as_ref();
                if character.chars().any(char::is_control) {
                    // The key did not produce text of its own; the text of the
                    // event is what the user typed.
                    if let Some(typed) = text.filter(|typed| !typed.chars().any(char::is_control)) {
                        commit.type_text(typed);
                    }
                } else {
                    commit.type_text(character);
                }
            }
            _ => {}
        }
        if keep {
            self.commit_box = Some(commit);
        }
        true
    }

    /// Commits what is in the staging area.
    fn commit_staged(&mut self, message: &str) {
        let message = message.trim();
        if message.is_empty() {
            self.status = Some("提交说明不能是空的".to_owned());
            return;
        }
        let Some(panel) = self.git.as_ref() else {
            self.status = Some("改动面板没有打开，不知道往哪儿提交".to_owned());
            return;
        };
        let root = panel.root.clone();
        match git::commit(&root, message) {
            Ok(said) => match self.load_git_panel(&root, GitView::Files) {
                Ok(fresh) => {
                    self.git = Some(fresh);
                    self.status = Some(format!("提交了：{said}"));
                }
                Err(error) => {
                    self.git = None;
                    self.status = Some(format!("提交了，但改动读不了：{error}"));
                }
            },
            Err(error) => self.status = Some(format!("提交没成：{error}")),
        }
    }

    /// Gives the keyboard back to the editor.
    ///
    /// The terminal has the keyboard while it is the one being typed into.
    /// A panel or a box that opens takes the keyboard: what is typed after
    /// that goes into the panel, not into the terminal.
    fn take_the_keyboard_from_the_terminal(&mut self) {
        self.terminal_has_focus = false;
    }

    /// Handles the keys of the box that asks for an expression.
    ///
    /// Ctrl+Shift+E opens the box. While the box is open it takes every
    /// key: an expression that is typed into the document is not an
    /// expression.
    fn handle_debug_expression_key<T: AsRef<str>>(
        &mut self,
        key: &Key<T>,
        text: Option<&str>,
    ) -> bool {
        let Some(mut box_) = self.debug_expression.take() else {
            // The box is not open: Ctrl+Shift+E opens it.
            // A panel that takes every key does not open over another one.
            if self.a_modal_is_open() {
                return false;
            }
            if !(self.modifiers.control_key() && self.modifiers.shift_key()) {
                return false;
            }
            let Some(letter) = key_letter(key) else {
                return false;
            };
            if !letter.eq_ignore_ascii_case(&'e') {
                return false;
            }
            self.ask_for_an_expression();
            return true;
        };

        // Whether the box stays open.
        let mut keep = true;
        match key {
            Key::Named(NamedKey::Escape) => {
                self.status = Some("没求值".to_owned());
                keep = false;
            }
            Key::Named(NamedKey::Enter) => {
                keep = false;
                self.evaluate_the_expression(box_.text());
            }
            _ => {
                box_.handle_key(key, text, self.modifiers.control_key());
            }
        }
        if keep {
            self.debug_expression = Some(box_);
        }
        true
    }

    /// Asks for an expression to be worked out.
    ///
    /// The expression is in the language of the program: the adapter is the
    /// one that knows it. The program has to be stopped, because an
    /// expression is asked about in a frame, and there is no frame while the
    /// program runs.
    fn ask_for_an_expression(&mut self) {
        let Some(debug) = self.debug.as_ref() else {
            self.status = Some("没有调试会话，没法求值".to_owned());
            return;
        };
        if debug.state != DebugState::Stopped {
            self.status = Some("程序在跑，停下来才能求值".to_owned());
            return;
        }
        if debug.frame_id.is_none() {
            self.status = Some("还没有拿到栈帧，等一等再求值".to_owned());
            return;
        }
        self.take_the_keyboard_from_the_terminal();
        self.debug_expression = Some(CommandBox::titled("求值"));
        self.status = Some("写表达式，回车求值，Esc 取消".to_owned());
    }

    /// Asks the adapter what the expression is worth.
    ///
    /// The answer comes later, on the thread of the adapter: the panel says
    /// it when it arrives.
    fn evaluate_the_expression(&mut self, expression: &str) {
        let expression = expression.trim();
        if expression.is_empty() {
            self.status = Some("表达式不能是空的".to_owned());
            return;
        }
        let Some(debug) = self.debug.as_mut() else {
            self.status = Some("没有调试会话，没法求值".to_owned());
            return;
        };
        let Some(frame_id) = debug.frame_id else {
            self.status = Some("还没有栈帧，没法求值".to_owned());
            return;
        };
        // The expression is kept: the answer does not carry it, and the
        // answer has to be said with what was asked.
        debug.evaluating.push(expression.to_owned());
        if let Err(error) = debug.client.evaluate(expression, frame_id) {
            debug.evaluating.pop();
            debug.push_output(&format!("{expression} 求不了：{error}"));
            return;
        }
        self.status = Some(format!("求值：{expression}"));
    }

    /// Handles the keys of the debugger.
    ///
    /// F9 puts a breakpoint on the line of the caret, F5 starts the
    /// session, F10 and F11 walk through the program, and F8 ends it.
    /// Ctrl+F9 is not here: the box that asks for the condition of a
    /// breakpoint takes it, and it takes it before this.
    fn handle_debug_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        match key {
            Key::Named(NamedKey::F10) => {
                self.step_debug(DebugStep::Over);
                true
            }
            Key::Named(NamedKey::F11) if self.modifiers.shift_key() => {
                self.step_debug(DebugStep::Out);
                true
            }
            Key::Named(NamedKey::F11) => {
                self.step_debug(DebugStep::In);
                true
            }
            _ => false,
        }
    }

    /// The address of the file that is on screen.
    fn uri_of_the_open_file(&self) -> Option<String> {
        self.uri_of_a_pane(front_tab(&self.groups))
    }

    /// The address of the file of a tab.
    ///
    /// The address comes from the path of the file and not from the language
    /// server: a file that has no server still has an address, and a
    /// breakpoint can be put on it.
    fn uri_of_a_pane(&self, pane: usize) -> Option<String> {
        let path = self.panes[pane]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())?;
        Some(codeweaver_lsp::protocol::file_uri(&path))
    }

    /// Puts a breakpoint on the line of the caret, or takes it away.
    fn toggle_breakpoint(&mut self) {
        let Some(uri) = self.uri_of_the_open_file() else {
            self.status = Some("这个文件没有地址，断点放不了".to_owned());
            return;
        };
        let caret = self.panes[front_tab(&self.groups)].editor.caret().0;
        let line = self.panes[front_tab(&self.groups)].editor.buffer().line_of_offset(caret) + 1;
        if self.breakpoints.toggle(&uri, line as u32) {
            // The place of the breakpoint is remembered, so that it follows
            // the text when the file changes above it.
            self.remember_the_place_of_the_breakpoint(&uri, line as u32);
            self.status = Some(format!(
                "第 {line} 行有了断点（{} 个）",
                self.breakpoints.len()
            ));
        } else {
            self.forget_the_place_of_the_breakpoint(&uri, line as u32);
            let count = self.breakpoints.len();
            self.status = Some(if count == 0 {
                "断点都撤了".to_owned()
            } else {
                format!("第 {line} 行的断点撤了（{count} 个）")
            });
        }
        // The adapter has to hear about it: a breakpoint it does not know
        // about is a breakpoint that does not stop the program.
        self.tell_the_adapter_about_the_breakpoints(&uri);
    }

    /// Asks for the condition of the breakpoint on the line of the caret.
    ///
    /// A condition says what has to be true for the program to stop at that
    /// line: `count > 3` stops the program only when the count is bigger
    /// than three. An empty condition is no condition: the breakpoint stops
    /// the program every time it comes to that line.
    fn ask_for_the_condition_of_the_breakpoint(&mut self) {
        let Some(uri) = self.uri_of_the_open_file() else {
            self.status = Some("这个文件没有地址，断点放不了".to_owned());
            return;
        };
        let Some(line) = self.the_line_of_the_caret() else {
            self.status = Some("光标不在哪一行，断点放不了".to_owned());
            return;
        };
        if !self.breakpoints.lines_of(&uri).contains(&line) {
            self.status = Some(format!("第 {line} 行还没有断点（F9 先放一个）"));
            return;
        }
        let mut box_ = CommandBox::titled("断点条件");
        if let Some(condition) = self.breakpoints.condition_of(&uri, line) {
            // The condition that is there is put in the box: a condition is
            // changed by writing it again, not by writing it from nothing.
            box_.set_text(condition);
        }
        self.take_the_keyboard_from_the_terminal();
        self.breakpoint_condition = Some(box_);
        self.breakpoint_condition_line = Some(line);
        self.status = Some("写条件，回车记住；空的表示每次都停；Esc 取消".to_owned());
    }

    /// The line the caret is on.
    fn the_line_of_the_caret(&self) -> Option<u32> {
        let caret = self.panes[front_tab(&self.groups)].editor.caret().0;
        let line = self.panes[front_tab(&self.groups)].editor.buffer().line_of_offset(caret) + 1;
        Some(line as u32)
    }

    /// Handles the keys of the box that asks for the condition.
    ///
    /// Ctrl+F9 opens the box. While the box is open it takes every key: a
    /// condition that is typed into the document is not a condition.
    fn handle_breakpoint_condition_key<T: AsRef<str>>(
        &mut self,
        key: &Key<T>,
        text: Option<&str>,
    ) -> bool {
        let Some(mut box_) = self.breakpoint_condition.take() else {
            // The box is not open: Ctrl+F9 opens it.
            // A panel that takes every key does not open over another one.
            if self.a_modal_is_open() {
                return false;
            }
            if !matches!(key, Key::Named(NamedKey::F9)) {
                return false;
            }
            // Ctrl+F9 and not Ctrl+Shift+F9: the shift is another key.
            if !self.modifiers.control_key() || self.modifiers.shift_key() {
                return false;
            }
            self.ask_for_the_condition_of_the_breakpoint();
            return true;
        };

        // Whether the box stays open.
        let mut keep = true;
        match key {
            Key::Named(NamedKey::Escape) => {
                self.status = Some("没改条件".to_owned());
                keep = false;
            }
            Key::Named(NamedKey::Enter) => {
                keep = false;
                self.set_the_condition_of_the_breakpoint(box_.text());
            }
            _ => {
                box_.handle_key(key, text, self.modifiers.control_key());
            }
        }
        if keep {
            self.breakpoint_condition = Some(box_);
        } else {
            self.breakpoint_condition_line = None;
        }
        true
    }

    /// Remembers the condition that was written.
    ///
    /// An empty condition takes the condition away: the breakpoint stops the
    /// program every time it comes to that line again.
    fn set_the_condition_of_the_breakpoint(&mut self, condition: &str) {
        let Some(uri) = self.uri_of_the_open_file() else {
            self.status = Some("这个文件没有地址，断点放不了".to_owned());
            return;
        };
        let Some(line) = self.breakpoint_condition_line.take() else {
            self.status = Some("不知道是哪个断点，条件没记住".to_owned());
            return;
        };
        let condition = condition.trim();
        if condition.is_empty() {
            self.breakpoints.set_condition(&uri, line, None);
            self.status = Some(format!("第 {line} 行的条件去掉了，每次都停"));
        } else {
            self.breakpoints.set_condition(&uri, line, Some(condition));
            self.status = Some(format!("第 {line} 行的条件：{condition}"));
        }
        // The adapter has to hear about it: a condition it does not know
        // about is a condition that is not worked out.
        self.tell_the_adapter_about_the_breakpoints(&uri);
    }

    /// Remembers where the breakpoint of a line is.    ///
    /// The place is an anchor in the buffer of the file: it follows the text
    /// when the file changes.
    fn remember_the_place_of_the_breakpoint(&mut self, uri: &str, line: u32) {
        let anchor = {
            let editor = &mut self.panes[front_tab(&self.groups)].editor;
            let offset = editor
                .buffer()
                .line_range(line.saturating_sub(1) as usize)
                .start;
            editor.buffer_mut().anchor(offset, Bias::Right)
        };
        self.breakpoints.remember(uri, line, anchor);
    }

    /// Forgets where the breakpoint of a line was.
    fn forget_the_place_of_the_breakpoint(&mut self, uri: &str, line: u32) {
        if let Some(anchor) = self.breakpoints.forget(uri, line) {
            self.panes[front_tab(&self.groups)]
                .editor
                .buffer_mut()
                .release_anchor(&anchor);
        }
    }

    /// Tells the adapter where the breakpoints of a file are.
    ///
    /// The adapter only stops the program where it was told to: a breakpoint
    /// it does not know about is a breakpoint that does not stop anything.
    /// The conditions go with them: a condition the adapter does not know
    /// about is a condition that is not worked out.
    fn tell_the_adapter_about_the_breakpoints(&mut self, uri: &str) {
        let Some(debug) = self.debug.as_mut() else {
            return;
        };
        let breakpoints = self.breakpoints.with_conditions_of(uri);
        debug.tell_about_the_breakpoints(uri, &breakpoints);
    }

    /// Moves the breakpoints of the open files to the lines the text is on.
    ///
    /// A breakpoint that was above a line that was added is on the line the
    /// text is on now: a breakpoint that kept its number would be on the
    /// wrong line.
    fn follow_the_breakpoints(&mut self) {
        for pane in 0..self.panes.len() {
            self.follow_the_breakpoints_of(pane);
        }
    }

    /// Moves the breakpoints of one file to the lines the text is on now.
    ///
    /// Returns whether a breakpoint moved.
    fn follow_the_breakpoints_of(&mut self, pane: usize) -> bool {
        let Some(uri) = self.uri_of_a_pane(pane) else {
            return false;
        };
        let moved = {
            let Self { panes, breakpoints, .. } = self;
            let buffer = panes[pane].editor.buffer();
            breakpoints.follow(&uri, buffer)
        };
        if !moved {
            return false;
        }
        // The adapter has to hear about the new lines.
        self.tell_the_adapter_about_the_breakpoints(&uri);
        true
    }

    /// Starts a debug session.
    fn start_debug_session(&mut self) {
        if self.debug.is_some() {
            self.status = Some("调试会话已经在跑了".to_owned());
            return;
        }
        let Some(config) = self.debug_adapter.clone() else {
            self.status = Some("没有调试适配器（用 --dap <program> 点名）".to_owned());
            return;
        };
        let Some(path) = self.panes[front_tab(&self.groups)]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())
        else {
            self.status = Some("这个文件不属于任何工程，没有可以调试的程序".to_owned());
            return;
        };
        let warning = if self.breakpoints.is_empty() {
            Some("没有断点，程序不会停（按 F9 放一个）".to_owned())
        } else {
            None
        };
        match Debugger::start(&config, &path) {
            Ok(debugger) => {
                self.debug = Some(debugger);
                self.status = Some(
                    warning.unwrap_or_else(|| "调试会话起了，等适配器说准备好了".to_owned()),
                );
            }
            Err(error) => {
                self.status = Some(format!("调试会话起不来：{error}"));
            }
        }
    }

    /// Ends the debug session.
    fn stop_debug_session(&mut self) {
        let Some(mut debug) = self.debug.take() else {
            self.status = Some("没有调试会话".to_owned());
            return;
        };
        let _ = debug.client.terminate();
        debug.client.shutdown();
        self.status = Some("调试结束了".to_owned());
    }

    /// Walks the program one step.
    fn step_debug(&mut self, step: DebugStep) {
        let message = match self.debug.as_ref() {
            None => Some("没有调试会话".to_owned()),
            Some(debug) if debug.state != DebugState::Stopped => {
                Some("程序没有停住，不能单步".to_owned())
            }
            Some(debug) => {
                let thread_id = debug.thread_id;
                let result = match step {
                    DebugStep::Over => debug.client.next(thread_id),
                    DebugStep::In => debug.client.step_in(thread_id),
                    DebugStep::Out => debug.client.step_out(thread_id),
                };
                match result {
                    Ok(()) => Some("走一步".to_owned()),
                    Err(error) => Some(format!("走不了：{error}")),
                }
            }
        };
        if let Some(message) = message {
            self.status = Some(message);
        }
    }

    /// Collects what the adapter has said.
    ///
    /// The adapter talks on a thread of its own, so what it says is
    /// collected once per frame, the way the output of the task is.
    ///
    /// The session itself is in the debug component: this only hands it the
    /// breakpoints of the editor.
    fn pump_debug(&mut self) -> bool {
        if self.debug.is_none() {
            return false;
        }
        // The breakpoints are read before the session is borrowed: the
        // session hands them to the adapter when it says it is ready.
        let breakpoints = self.breakpoints.all_with_conditions();
        let Some(debug) = self.debug.as_mut() else {
            return false;
        };
        debug.pump(&breakpoints)
    }

    /// Opens the panel that lists the commands.
    ///
    /// The panel is the one the person writes a few letters into to find a
    /// command. It is opened by Ctrl+Shift+P and by the command of its own
    /// in the panel, the way VS Code opens its own.
    fn open_palette(&mut self) {
        // Only one panel at a time: the box that asks for a new name and the
        // box that asks for a command give up their place.
        self.rename_box = None;
        self.command_box = None;
        self.take_the_keyboard_from_the_terminal();
        let context = self.command_context();
        let palette = CommandPalette::open(&self.commands, &context);
        self.status = Some(if palette.is_empty() {
            "这个编辑器还没有命令".to_owned()
        } else {
            "写几个字找命令，上下键挑，回车就跑，Esc 取消".to_owned()
        });
        self.palette = Some(palette);
    }

    /// Handles the keys of the panel that lists the commands.
    ///
    /// Ctrl+Shift+P opens it, the way VS Code opens its own. While the panel
    /// is open it takes every key: a command that is typed into the document
    /// is not a command.
    fn handle_palette_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        let Some(mut palette) = self.palette.take() else {
            // The panel is not open: Ctrl+Shift+P opens it.
            // A panel that takes every key does not open over another one.
            if self.a_modal_is_open() {
                return false;
            }
            if !(self.modifiers.control_key() && self.modifiers.shift_key()) {
                return false;
            }
            let Some(letter) = key_letter(key) else {
                return false;
            };
            if !letter.eq_ignore_ascii_case(&'p') {
                return false;
            }
            self.open_palette();
            return true;
        };

        let command = palette.handle_key(key, text, self.modifiers.control_key());
        match command {
            PaletteCommand::None => {}
            PaletteCommand::Close => {
                self.status = Some("关掉了命令面板".to_owned());
                return true;
            }
            PaletteCommand::Run(id) => {
                // The panel goes away when the command runs: the command may
                // open a panel of its own.
                self.run_named_command(&id);
                return true;
            }
        }
        self.palette = Some(palette);
        true
    }

    /// Opens the panel that shows what the editor is doing inside.
    ///
    /// The panel is a modal: it covers the window, the way the settings
    /// panel does, and it takes the keys while it is open.
    fn open_developer_panel(&mut self) {
        // Only one panel at a time: the panel that lists the commands and
        // the settings panel give up their place.
        self.palette = None;
        self.settings = None;
        self.developer = Some(DeveloperPanel::new());
        self.status = Some("开发者面板：↑↓ 翻页，Esc 关掉".to_owned());
    }

    /// Handles the keys of the developer panel.
    fn handle_developer_key<T: AsRef<str>>(&mut self, key: &Key<T>, scale_factor: f64) -> bool {
        let Some(mut panel) = self.developer.take() else {
            return false;
        };
        let lines = self.developer_lines();
        let visible = developer::visible_rows(self.window_size(scale_factor));
        match panel.handle_key(key, lines.len(), visible) {
            DeveloperCommand::None => {}
            DeveloperCommand::Close => {
                self.status = Some("关掉了开发者面板".to_owned());
                return true;
            }
        }
        self.developer = Some(panel);
        true
    }

    /// The lines the developer panel shows.
    ///
    /// The panel says what the editor is made of: the commands and their
    /// keys, whether two of them want the same key, what the language server
    /// and the agent are doing, and what the log says.
    fn developer_lines(&self) -> Vec<DeveloperLine> {
        let mut lines = Vec::new();

        lines.push(DeveloperLine::section("开发模式"));
        if self.dev {
            lines.push(DeveloperLine::good("开着".to_owned()));
        } else {
            lines.push(DeveloperLine::warning(
                "没有以开发模式启动（加 --dev 再来）".to_owned(),
            ));
        }
        match self.dev_log.as_ref() {
            Some(log) => {
                lines.push(DeveloperLine::text(format!(
                    "日志：{}",
                    log.path().display()
                )));
                if log.is_writing() {
                    lines.push(DeveloperLine::good("日志在写".to_owned()));
                } else {
                    lines.push(DeveloperLine::warning(
                        log.problem().unwrap_or("日志没有在写").to_owned(),
                    ));
                }
            }
            None => lines.push(DeveloperLine::text("没有日志".to_owned())),
        }

        lines.push(DeveloperLine::section("语言服务器"));
        lines.push(match self.server.as_ref() {
            Some(_) => DeveloperLine::good("连着".to_owned()),
            None => DeveloperLine::text("没有连着".to_owned()),
        });

        lines.push(DeveloperLine::section("Agent"));
        lines.push(match self.agent.as_ref() {
            Some(_) => DeveloperLine::good("在".to_owned()),
            None => match self.agent_error.as_ref() {
                Some(error) => DeveloperLine::warning(format!("不可用：{error}")),
                None => DeveloperLine::text("没有连".to_owned()),
            },
        });

        lines.push(DeveloperLine::section(format!(
            "命令（{} 条）",
            self.commands.all().len()
        )));
        for command in self.commands.all() {
            let key = command
                .keybinding
                .as_ref()
                .map(|keybinding| keybinding.text())
                .unwrap_or_default();
            let when = match command.when {
                When::Always => String::new(),
                When::HasFile => "要文件".to_owned(),
                When::HasNameAtCaret => "要名字".to_owned(),
                When::HasRepository => "要仓库".to_owned(),
            };
            lines.push(DeveloperLine::text(format!(
                "{:<28} {:<16} {:<12} {}",
                command.id, command.title, key, when
            )));
        }

        lines.push(DeveloperLine::section("键位冲突"));
        let conflicts = self.commands.conflicts();
        if conflicts.is_empty() {
            lines.push(DeveloperLine::good("没有两条命令要同一个键".to_owned()));
        } else {
            for conflict in conflicts {
                lines.push(DeveloperLine::warning(format!(
                    "{} 和 {} 都要 {}",
                    conflict.first, conflict.second, conflict.keybinding.text()
                )));
            }
        }

        lines.push(DeveloperLine::section("日志"));
        match self.dev_log.as_ref() {
            Some(log) => {
                let tail = log.last(30);
                if tail.is_empty() {
                    lines.push(DeveloperLine::text("还没有话说".to_owned()));
                }
                for line in tail {
                    lines.push(DeveloperLine::text(line.to_owned()));
                }
            }
            None => lines.push(DeveloperLine::text("没有日志".to_owned())),
        }

        lines
    }

    /// What the commands may ask about.
    ///
    /// The editor tells the registry what is true right now; the registry
    /// says which commands that lets it run.
    fn command_context(&self) -> CommandContext {        let pane = &self.panes[front_tab(&self.groups)];
        let has_name_at_caret = rename::word_at_caret(pane.editor.buffer(), pane.editor.caret().0)
            .is_some();
        let has_repository = pane
            .document
            .as_ref()
            .and_then(|document| git::repository_root(document.path()))
            .is_some();
        CommandContext {
            has_file: pane.document.is_some(),
            has_name_at_caret,
            has_repository,
        }
    }

    /// The command whose key was pressed, if there is one.
    ///
    /// The key route does not know the commands one by one: it asks the
    /// registry which command wants this key.
    fn command_of_the_key<T: AsRef<str>>(&self, key: &Key<T>) -> Option<String> {
        let pressed = commands::PressedKey {
            letter: key_letter(key),
            name: key_name(key),
            ctrl: self.modifiers.control_key(),
            alt: self.modifiers.alt_key(),
            shift: self.modifiers.shift_key(),
            cmd: false,
        };
        self.commands
            .by_key(&pressed)
            .map(|command| command.id.clone())
    }

    /// Runs the command whose key was pressed.
    fn run_the_command_of_the_key<T: AsRef<str>>(&mut self, key: &Key<T>) -> bool {
        // A panel that is open takes every key: the key does not belong to
        // the editor then.
        if self.a_modal_is_open() {
            return false;
        }
        let Some(id) = self.command_of_the_key(key) else {
            return false;
        };
        self.run_named_command(&id)
    }

    /// Runs the command with that id.
    ///
    /// The panel of the commands and the keys both come here: a command is
    /// run the same way either way.
    fn run_named_command(&mut self, id: &str) -> bool {
        let context = self.command_context();
        if !self.commands.is_enabled(id, &context) {
            self.status = Some(match self.commands.get(id).map(|command| command.when) {
                Some(When::HasFile) => "没有打开的文件，这条命令用不了".to_owned(),
                Some(When::HasNameAtCaret) => "光标处没有名字，这条命令用不了".to_owned(),
                Some(When::HasRepository) => "这个文件不在仓库里，这条命令用不了".to_owned(),
                _ => format!("命令 {id} 现在用不了"),
            });
            return false;
        }
        match id {
            "workbench.commandPalette" => {
                // The command of the panel opens the panel again.
                self.open_palette();
            }
            "workbench.developerPanel" => self.open_developer_panel(),
            "view.activityBar" => self.focus_the_activity_bar(),
            "view.toggleSidebar" => {
                if self.sidebar.is_open() {
                    self.sidebar.hide();
                    self.status = Some("侧边栏收起来了".to_owned());
                } else {
                    self.sidebar.show(self.sidebar.section);
                    self.status = Some("侧边栏打开了".to_owned());
                }
            }
            "workbench.settings" => self.open_settings(),
            "view.zoomIn" => {
                self.set_zoom(self.zoom.bigger());
                self.status = Some(format!("字号 {}", self.zoom.label()));
            }
            "view.zoomOut" => {
                self.set_zoom(self.zoom.smaller());
                self.status = Some(format!("字号 {}", self.zoom.label()));
            }
            "view.zoomReset" => {
                self.set_zoom(self.zoom.reset());
                self.status = Some(format!("字号 {}", self.zoom.label()));
            }
            "view.splitRight" => self.split_right(),
            "workbench.save" => {
                if let Some(tab) = self.tabs.active().map(|tab| tab.id) {
                    self.save_tab(tab);
                    self.sync_dirty_flags();
                }
            }
            "workbench.closeTab" => self.request_close(front_tab(&self.groups)),
            "workspace.save" => self.save_the_workspace(),
            "editor.format" => self.request_format(),
            "editor.rename" => self.open_rename_box(),
            "editor.codeAction" => self.request_code_action(),
            "editor.find" => self.open_find(false),
            "editor.columnSelectDown" => {
                self.column_select(multicursor::Direction::Down);
            }
            "editor.columnSelectUp" => {
                self.column_select(multicursor::Direction::Up);
            }
            "editor.columnSelectRight" => {
                self.column_select(multicursor::Direction::Right);
            }
            "editor.columnSelectLeft" => {
                self.column_select(multicursor::Direction::Left);
            }
            "editor.addWordCursor" => {
                self.add_the_next_word();
            }
            "search.findInFiles" => self.open_search_panel(false),
            "search.replaceInFiles" => self.open_search_panel(true),
            "terminal.toggle" => self.open_terminal(),
            "terminal.runCommand" => {
                self.command_box = Some(CommandBox::new());
                self.status = Some("写命令，回车就跑，Esc 取消".to_owned());
            }
            "task.build" => self.start_build_task(),
            "git.showChanges" => self.open_git_panel(),
            "git.showBranches" => self.open_the_git_panel(GitView::Branches),
            "git.fetch" => self.fetch_from_the_command(),
            "git.pull" => self.update_from_the_command(),
            "git.push" => self.ask_to_push_from_the_command(),
            "debug.toggleBreakpoint" => self.toggle_breakpoint(),
            "debug.start" => self.start_debug_session(),
            "debug.stop" => self.stop_debug_session(),
            "debug.evaluate" => self.ask_for_an_expression(),
            "agent.startTurn" => self.start_agent_turn(),
            other => {
                self.status = Some(format!("命令 {other} 还不知道怎么跑"));
                return false;
            }
        }
        true
    }

    /// Handles the keys of the box that asks for a command.
    ///
    /// The box is opened by the command route (Ctrl+Shift+U). While it is
    /// open it takes every key: a command that is typed into the document is
    /// not a command.
    fn handle_command_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        let Some(mut box_) = self.command_box.take() else {
            // The box is not open: the command route opens it.
            return false;
        };

        // Whether the box stays open.
        let mut keep = true;
        match key {
            Key::Named(NamedKey::Escape) => {
                self.status = Some("取消了命令".to_owned());
                keep = false;
            }
            Key::Named(NamedKey::Enter) => {
                keep = false;
                self.run_command(box_.text());
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(previous) = box_.previous(&self.command_history) {
                    box_.set_text(&previous);
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(next) = box_.next(&self.command_history) {
                    box_.set_text(&next);
                }
            }
            _ => {
                box_.handle_key(key, text, self.modifiers.control_key());
            }
        }
        if keep {
            self.command_box = Some(box_);
        }
        true
    }

    /// Runs the command that was written.
    ///
    /// The command runs in the folder of the project the open file belongs
    /// to, and its output goes into the panel of the task.
    fn run_command(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            self.status = Some("命令不能是空的".to_owned());
            return;
        }
        let (program, args) = match task::split_command(line) {
            Ok(parts) => parts,
            Err(error) => {
                self.status = Some(format!("命令写错了：{error}"));
                return;
            }
        };
        // The command joins the history, whether it worked or not: what was
        // typed is what is worth repeating.
        self.command_history.push(line.to_owned());
        if self.command_history.len() > 50 {
            self.command_history.remove(0);
        }
        let Some(path) = self.panes[front_tab(&self.groups)]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())
        else {
            self.status = Some("这个文件不属于任何工程，不知道在哪儿跑这个命令".to_owned());
            return;
        };
        if let Some(task) = self.task.as_ref()
            && task.is_running()
        {
            self.status = Some("已经有任务在跑了".to_owned());
            return;
        }

        let root = lsp::workspace_root(&path);
        let title = format!("{program} {}", args.join(" "));
        match Task::start(&program, &args, &root) {
            Ok(task) => {
                self.task = Some(task);
                self.task_output = TaskOutput::default();
                self.task_title = title.clone();
                self.task_panel = true;
                self.status = Some(format!("正在跑：{title}"));
            }
            Err(error) => {
                eprintln!("could not start the task: {error}");
                self.status = Some(format!("命令起不来：{error}"));
            }
        }
    }

    /// Opens the terminal, or gives it the keyboard back.
    ///
    /// The terminal is a shell whose input is a pipe: it lives in the folder
    /// of the project and what is typed goes into it. The keyboard belongs
    /// to the terminal until `Esc` gives it back to the document.
    fn open_terminal(&mut self) {
        if self.terminal_input.is_some() {
            // The terminal is already there: it takes the keyboard back.
            self.terminal_has_focus = true;
            self.status = Some("终端拿到了键盘：写命令回车就跑".to_owned());
            return;
        }
        // A panel that takes every key does not open over another one.
        if self.a_modal_is_open() {
            self.status = Some("有个面板开着，先把它关掉再开终端".to_owned());
            return;
        }
        let Some(path) = self.panes[front_tab(&self.groups)]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())
        else {
            self.status = Some("这个文件不属于任何工程，不知道在哪儿开终端".to_owned());
            return;
        };
        if let Some(task) = self.task.as_ref()
            && task.is_running()
        {
            // The terminal and the tasks share one place: two of them at the
            // same time would fight over the output panel.
            self.status = Some("已经有任务在跑了（终端和任务共用一个地方）".to_owned());
            return;
        }
        let root = lsp::workspace_root(&path);
        let (program, args) = task::shell_command();
        match Task::start_with_input(program, args.as_slice(), &root) {
            Ok(task) => {
                self.task = Some(task);
                self.task_output = TaskOutput::default();
                self.task_title = format!("终端 · {program}");
                self.task_panel = true;
                self.terminal_input = Some(CommandBox::titled("终端"));
                self.terminal_has_focus = true;
                self.status =
                    Some("终端开着：写命令回车就跑，Esc 把键盘还给编辑器".to_owned());
            }
            Err(error) => {
                eprintln!("could not start the terminal: {error}");
                self.status = Some(format!("终端起不来：{error}"));
            }
        }
    }

    /// Closes the terminal.
    ///
    /// The shell is stopped: a shell that nobody is watching is a shell that
    /// nobody can kill.
    fn close_terminal(&mut self) {
        if let Some(task) = self.task.as_ref() {
            task.stop();
        }
        self.terminal_input = None;
        self.terminal_has_focus = false;
    }

    /// Handles the keys of the terminal.
    ///
    /// Ctrl+Shift+T opens the terminal, and takes the keyboard back when it
    /// is open. While the terminal has the keyboard, what is typed goes into
    /// it and not into the document. The keys with a modifier still belong
    /// to the editor: the shortcuts are the same wherever the focus is, and
    /// the terminal gives the keyboard up when one of them is pressed.
    fn handle_terminal_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        // Ctrl+Shift+T belongs to the terminal whether it is open or not.
        if self.modifiers.control_key() && self.modifiers.shift_key() {
            let is_the_terminal_key = key_letter(key)
                .is_some_and(|letter| letter.eq_ignore_ascii_case(&'t'));
            if is_the_terminal_key {
                self.open_terminal();
                return true;
            }
        }

        // The terminal is not open, or the keyboard is not in it: the key
        // belongs to the editor.
        let Some(mut box_) = self.terminal_input.take() else {
            return false;
        };
        if !self.terminal_has_focus {
            self.terminal_input = Some(box_);
            return false;
        }

        // The keys with a modifier belong to the editor.
        //
        // The terminal gives the keyboard up as well: the shortcut that is
        // pressed opens something of its own (the find bar, for one), and
        // what is typed after that has to go there and not here.
        if self.modifiers.control_key() || self.modifiers.alt_key() {
            self.terminal_has_focus = false;
            self.terminal_input = Some(box_);
            return false;
        }

        match key {
            Key::Named(NamedKey::Escape) => {
                // The keyboard goes back to the document; the shell keeps
                // running.
                self.terminal_has_focus = false;
                self.terminal_input = Some(box_);
                self.status =
                    Some("键盘还给编辑器了，终端还在跑（Ctrl+Shift+T 回去）".to_owned());
                true
            }
            Key::Named(NamedKey::Enter) => {
                self.send_to_the_terminal(box_.text());
                box_.set_text("");
                self.terminal_input = Some(box_);
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(previous) = box_.previous(&self.terminal_history) {
                    box_.set_text(&previous);
                }
                self.terminal_input = Some(box_);
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(next) = box_.next(&self.terminal_history) {
                    box_.set_text(&next);
                }
                self.terminal_input = Some(box_);
                true
            }
            Key::Named(NamedKey::Backspace)
            | Key::Named(NamedKey::Delete)
            | Key::Named(NamedKey::ArrowLeft)
            | Key::Named(NamedKey::ArrowRight)
            | Key::Named(NamedKey::Home)
            | Key::Named(NamedKey::End)
            // The window system names the space bar instead of making it a
            // character: without this arm the space would fall through to
            // the document behind the terminal, and no command could have an
            // argument (`echo hello`, `cargo build`, `git status`).
            | Key::Named(NamedKey::Space)
            | Key::Character(_) => {
                box_.handle_key(key, text, false);
                self.terminal_input = Some(box_);
                true
            }
            _ => {
                // The key is not the terminal's: the editor may have it.
                self.terminal_input = Some(box_);
                false
            }
        }
    }

    /// Sends the line that was typed to the shell.
    ///
    /// The shell reads its input from a pipe, so nothing echoes it: the
    /// editor says the line itself, so that the panel shows what was typed.
    fn send_to_the_terminal(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let Some(task) = self.task.as_ref() else {
            self.status = Some("没有终端可以写".to_owned());
            return;
        };
        task.echo(line);
        if let Err(error) = task.write_input(&format!("{line}\n")) {
            self.status = Some(format!("写不进去：{error}"));
            return;
        }
        // The command joins the history, so that it can be run again.
        self.terminal_history.push(line.to_owned());
        if self.terminal_history.len() > 50 {
            self.terminal_history.remove(0);
        }
    }

    /// Handles the keys of the panel that finds text in the files.
    ///
    /// Ctrl+Shift+F opens the panel. While the panel is open it takes every
    /// key: a word that is typed into the document behind it is not a word
    /// the person is looking for.
    fn handle_search_key<T: AsRef<str>>(&mut self, key: &Key<T>, text: Option<&str>) -> bool {
        let Some(mut panel) = self.search.take() else {
            // The panel is not open: Ctrl+Shift+F opens it, and Ctrl+Shift+H
            // opens it to replace as well as to find.
            // A panel that takes every key does not open over another one.
            if self.a_modal_is_open() {
                return false;
            }
            if !(self.modifiers.control_key() && self.modifiers.shift_key()) {
                return false;
            }
            let Some(letter) = key_letter(key) else {
                return false;
            };
            if letter.eq_ignore_ascii_case(&'f') {
                self.open_search_panel(false);
                return true;
            }
            if letter.eq_ignore_ascii_case(&'h') {
                self.open_search_panel(true);
                return true;
            }
            return false;
        };

        match panel.handle_key(
            key,
            text,
            self.modifiers.control_key(),
            self.modifiers.alt_key(),
        ) {
            SearchCommand::None => {}
            SearchCommand::Close => {
                self.status = Some("关掉了查找面板".to_owned());
                // The panel goes away and is not put back.
                return true;
            }
            SearchCommand::Open(hit) => {
                let root = panel.root_of(&hit.path).map(Path::to_path_buf);
                self.open_search_hit(&hit, root.as_deref());
                return true;
            }
            SearchCommand::ReplaceAll { needle, replacement } => {
                let (files, places, skipped) =
                    self.replace_in_the_files(&panel, &needle, &replacement);
                // The files that are open are dirty now, and the server has to
                // hear about the new text.
                self.sync_dirty_flags();
                for index in 0..self.panes.len() {
                    self.tell_server_about_the_change_of(index);
                }
                let mut said = format!("改了 {files} 个文件里的 {places} 处");
                if skipped > 0 {
                    said.push_str(&format!("（{skipped} 个文件没有改）"));
                }
                self.status = Some(said);
                // The places that were found are the old ones: the search runs
                // again, so that the list is the list of what is there now.
                panel.start_search();
                self.search = Some(panel);
                return true;
            }
        }
        // The text that was in the box is kept: the person who searches
        // twice does not have to type the same word twice.
        self.search_needle = panel.needle().to_owned();
        self.search = Some(panel);
        true
    }

    /// Opens the panel that finds text in the files of the project.
    ///
    /// The files that are searched are the ones of the project the open file
    /// belongs to: the folder that holds the manifest of the project. A
    /// workspace that is open names more than one folder, and the search looks
    /// in all of them.
    ///
    /// With `replacing` the panel has a box for the text that takes the
    /// place of what is found: it replaces as well as it finds.
    fn open_search_panel(&mut self, replacing: bool) {
        let roots = self.the_roots_to_search();
        if roots.is_empty() {
            self.status = Some("没有打开的文件，不知道在哪儿找".to_owned());
            return;
        }
        let needle = self.search_needle.clone();
        // A workspace that names more than one folder is searched in all of
        // them, and the person is told so.
        let where_ = if roots.len() > 1 {
            format!("在 {} 个文件夹里找：", roots.len())
        } else {
            String::new()
        };
        self.take_the_keyboard_from_the_terminal();
        self.search = Some(if replacing {
            SearchPanel::with_replace(roots, &needle)
        } else {
            SearchPanel::new(roots, &needle)
        });
        self.status = Some(if replacing {
            format!("{where_}写要找的文字，回车开始找；Tab 换到替换那一栏")
        } else {
            format!("{where_}写要找的文字，回车开始找")
        });
    }

    /// The folders the search looks in.
    ///
    /// A workspace that is open names the folders of the project, and the
    /// search looks in every one of them. A project without a workspace has
    /// one folder: the one the open file belongs to.
    fn the_roots_to_search(&self) -> Vec<PathBuf> {
        if let Some(workspace) = self.open_workspace.as_ref() {
            let roots: Vec<PathBuf> = workspace
                .roots
                .iter()
                .map(|root| root.path.clone())
                .collect();
            if !roots.is_empty() {
                return roots;
            }
        }
        let Some(path) = self.panes[front_tab(&self.groups)]
            .document
            .as_ref()
            .map(|document| document.path().to_path_buf())
        else {
            return Vec::new();
        };
        vec![lsp::workspace_root(&path)]
    }

    /// Replaces everything that was found with something else.
    ///
    /// The files that are open in the editor change in their buffers, as one
    /// transaction each: the undo takes them back. The files that are not
    /// open go to disk, and the undo does not take them back — the same
    /// trade-off the renaming of the language server makes.
    ///
    /// The places are found again in the text as it is now: a file that
    /// changed since the search is left alone, rather than changed in the
    /// wrong place.
    ///
    /// Returns how many files were changed, how many places were replaced,
    /// and how many files were left alone.
    fn replace_in_the_files(
        &mut self,
        panel: &SearchPanel,
        needle: &str,
        replacement: &str,
    ) -> (usize, usize, usize) {
        // The panel is given, not looked for: the one that asks for the
        // replacement holds the panel in its own hands while it does so.
        let case_sensitive = panel.is_case_sensitive();
        let whole_word = panel.is_whole_word();
        // The files that were found, in the order they were found.
        let mut files: Vec<PathBuf> = Vec::new();
        for hit in panel.hits() {
            if !files.contains(&hit.path) {
                files.push(hit.path.clone());
            }
        }
        let mut changed_files = 0;
        let mut changed_places = 0;
        let mut skipped = 0;
        for path in files {
            let uri = codeweaver_lsp::protocol::file_uri(&path);
            match self.replace_in_a_file(&uri, needle, replacement, case_sensitive, whole_word) {
                Ok(places) if places > 0 => {
                    changed_files += 1;
                    changed_places += places;
                }
                Ok(_) => skipped += 1,
                Err(error) => {
                    eprintln!("could not replace the text in {}: {error}", path.display());
                    skipped += 1;
                }
            }
        }
        (changed_files, changed_places, skipped)
    }

    /// Replaces the text in one file.
    ///
    /// The file that is open in the editor changes in its buffer, as one
    /// transaction: the undo takes it back. The file that is not open goes to
    /// disk.
    fn replace_in_a_file(
        &mut self,
        uri: &str,
        needle: &str,
        replacement: &str,
        case_sensitive: bool,
        whole_word: bool,
    ) -> Result<usize, String> {
        // The places are found in the text as it is now.
        let (text, index): (String, Option<usize>) = match self.pane_of_a_file(uri) {
            Some(index) => (self.panes[index].editor.buffer().text(), Some(index)),
            None => {
                let path = codeweaver_lsp::protocol::uri_to_path(uri)
                    .ok_or_else(|| "不是编辑器能改的文件".to_owned())?;
                let document = self
                    .workspace
                    .open(&path)
                    .map_err(|error| format!("读不了：{error}"))?;
                (document.saved_text().to_owned(), None)
            }
        };
        let Some(places) =
            codeweaver_workspace::search::places_of(&text, needle, case_sensitive, whole_word)
        else {
            return Err("这个文件里有折叠后长度不一样的大小写字母，没敢改".to_owned());
        };
        if places.is_empty() {
            // The file does not hold the text any more: it changed since the
            // search.
            return Ok(0);
        }
        let changes: Vec<(std::ops::Range<TextOffset>, String)> = places
            .iter()
            .map(|(start, end)| {
                (
                    TextOffset(*start)..TextOffset(*end),
                    replacement.to_owned(),
                )
            })
            .collect();
        match index {
            Some(index) => {
                self.panes[index]
                    .editor
                    .replace_edits(changes)
                    .map_err(|error| format!("写不进去：{error}"))?;
            }
            None => self.write_a_file(uri, &changes)?,
        }
        Ok(places.len())
    }

    /// Opens the file of a place the search found, and puts the caret there.
    ///
    /// The caret is on the line that was found: the place is the point of
    /// the whole search. The root is the folder the place is in, when there is
    /// one: the path that starts at the drive of the machine says less.
    fn open_search_hit(&mut self, hit: &SearchHit, root: Option<&Path>) {
        if !self.open_file(&hit.path) {
            return;
        }
        let pane = &mut self.panes[front_tab(&self.groups)];
        let line = hit.line.saturating_sub(1);
        let offset = pane.editor.buffer().line_range(line).start;
        pane.editor.set_carets(&[offset]);
        // The line that was found is scrolled into the picture: a place that
        // is not on the screen is not opened.
        let scale_factor = self
            .window
            .as_ref()
            .map(|window| window.scale_factor())
            .unwrap_or(1.0);
        self.keep_caret_visible(scale_factor);
        // The place is said in words that are inside the project: the path
        // that starts at the drive of the machine says less.
        let place = match root {
            Some(root) => hit
                .path
                .strip_prefix(root)
                .unwrap_or(&hit.path)
                .display()
                .to_string(),
            None => hit.path.display().to_string(),
        };
        self.status = Some(format!("打开了 {place} 的第 {} 行", hit.line));
    }

    /// Collects the answer of the search, if it has come.
    ///
    /// The search runs on a thread of its own, so the window has to wake up
    /// to hear what it found.
    fn pump_search(&mut self) -> bool {
        let Some(panel) = self.search.as_mut() else {
            return false;
        };
        let arrived = panel.pump();
        if arrived {
            // The places that were found are kept: the panel of the search
            // can be closed, and the side bar still shows what was found.
            self.search_hits = panel.hits().to_vec();
        }
        arrived
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
        if self.pump_task()
            && let Some(window) = self.window.as_ref()
        {
            window.request_redraw();
        }
        // The search runs on a thread of its own, so the window has to wake
        // up to hear what it found.
        if self.pump_search()
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
                            find.type_text(&self.panes[front_tab(&self.groups)].editor, &text);
                        } else if let Some(rename) = self.rename_box.as_mut() {
                            rename.type_text(&text);
                        } else if let Some(commit) = self.commit_box.as_mut() {
                            commit.type_text(&text);
                        } else if let Some(command) = self.command_box.as_mut() {
                            command.type_text(&text);
                        } else if let Some(palette) = self.palette.as_mut() {
                            palette.type_text(&text);
                        } else if let Some(condition) = self.breakpoint_condition.as_mut() {
                            condition.type_text(&text);
                        } else if let Some(expression) = self.debug_expression.as_mut() {
                            expression.type_text(&text);
                        } else if let Some(search) = self.search.as_mut() {
                            search.type_text(&text);
                        } else if self.terminal_has_focus
                            && let Some(terminal) = self.terminal_input.as_mut()
                        {
                            terminal.type_text(&text);
                        } else if let Err(error) = self.panes[front_tab(&self.groups)].editor.insert(&text) {
                            eprintln!("failed to commit IME text: {error}");
                        }
                        self.input.finish_commit();
                    }
                }
                window.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                    self.modifiers = modifiers.state();
                    // A box of text is over when the keys that made it are let
                    // go: what is left is a plain selection on every line.
                    if !(self.modifiers.control_key()
                        && self.modifiers.shift_key()
                        && self.modifiers.alt_key())
                    {
                        self.end_the_column_selection();
                    }
                }
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
                        ElementState::Released => self.panes[front_tab(&self.groups)].view.release(),
                    }
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.modifiers.control_key() {
                    // The wheel with the control key held changes the size of
                    // the text instead of scrolling it.
                    let notches = match delta {
                        MouseScrollDelta::LineDelta(lines, _) => lines,
                        MouseScrollDelta::PixelDelta(pixels) => pixels.y as f32 / 40.0,
                    };
                    if notches != 0.0 {
                        let zoom = if notches > 0.0 {
                            self.zoom.bigger()
                        } else {
                            self.zoom.smaller()
                        };
                        self.set_zoom(zoom);
                        self.status = Some(format!("字号 {}", self.zoom.label()));
                    }
                    window.request_redraw();
                    return;
                }
                let lines = match delta {
                    MouseScrollDelta::LineDelta(lines, _) => lines as isize,
                    // The wheel that is turned sideways scrolls sideways; the
                    // text is scrolled up and down, so the vertical part of
                    // the movement is the one that counts.
                    MouseScrollDelta::PixelDelta(pixels) => {
                        (pixels.y / self.zoom.line_height() as f64) as isize
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
                // The panel that lists the commands takes every key while it
                // is open.
                if self.handle_palette_key(&event.logical_key.as_ref(), event.text.as_deref()) {
                    window.request_redraw();
                    return;
                }
                // The developer panel takes every key while it is open.
                if self.handle_developer_key(&event.logical_key.as_ref(), window.scale_factor()) {
                    window.request_redraw();
                    return;
                }
                // The settings panel takes every key while it is open, and
                // Ctrl+, opens it.
                if self.handle_settings_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // Ctrl+Shift+G opens the panel of the changes. While the
                // panel is open it takes every key.
                if self.handle_git_key(&event.logical_key.as_ref(), event.text.as_deref()) {
                    window.request_redraw();
                    return;
                }
                // Ctrl+Shift+F opens the panel that finds text in the files.
                // While the panel is open it takes every key.
                if self.handle_search_key(&event.logical_key.as_ref(), event.text.as_deref()) {
                    window.request_redraw();
                    return;
                }
                // Ctrl+F9 asks for the condition of the breakpoint on the
                // line of the caret. While the box is open it takes every
                // key.
                if self.handle_breakpoint_condition_key(&event.logical_key.as_ref(), event.text.as_deref())
                {
                    window.request_redraw();
                    return;
                }
                // Ctrl+Shift+E asks for an expression to be worked out. While
                // the box is open it takes every key.
                if self.handle_debug_expression_key(&event.logical_key.as_ref(), event.text.as_deref())
                {
                    window.request_redraw();
                    return;
                }
                // F9 puts a breakpoint, F5 starts the debugging, F10 and F11
                // walk through the program.
                if self.handle_debug_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // The box that asks for a command takes every key while it is
                // open. It is opened by the command route (Ctrl+Shift+U).
                if self.handle_command_key(&event.logical_key.as_ref(), event.text.as_deref()) {
                    window.request_redraw();
                    return;
                }
                // Ctrl+Shift+T opens the terminal. While the terminal has the
                // focus, what is typed goes into it.
                if self.handle_terminal_key(&event.logical_key.as_ref(), event.text.as_deref()) {
                    window.request_redraw();
                    return;
                }
                // Alt+Enter asks the server what it offers to do about the
                // place of the caret.
                if self.handle_action_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // The activity bar takes the keyboard when the person says so.
                if self.handle_activity_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // The key may be the key of a command of the editor. The
                // route does not know the commands one by one: it asks the
                // registry.
                if self.run_the_command_of_the_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // Ctrl+Shift+B builds the project; Esc closes the output of
                // the task.
                if self.handle_task_key(&event.logical_key.as_ref()) {
                    window.request_redraw();
                    return;
                }
                // While the box that asks for the new name is open, the keys
                // go to it.
                if self.handle_rename_box_key(&event.logical_key.as_ref(), event.text.as_deref()) {
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
                    Key::Named(NamedKey::Backspace) => self.panes[front_tab(&self.groups)].editor.backspace(),
                    Key::Named(NamedKey::Delete) => self.panes[front_tab(&self.groups)].editor.delete_forward(),
                    Key::Named(NamedKey::ArrowLeft) if self.modifiers.shift_key() => {
                        self.panes[front_tab(&self.groups)].editor.extend_left();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowRight) if self.modifiers.shift_key() => {
                        self.panes[front_tab(&self.groups)].editor.extend_right();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowUp) if self.modifiers.shift_key() => {
                        let pane = &mut self.panes[front_tab(&self.groups)];
                        pane.view.extend_vertical(&mut pane.editor, -1);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowDown) if self.modifiers.shift_key() => {
                        let pane = &mut self.panes[front_tab(&self.groups)];
                        pane.view.extend_vertical(&mut pane.editor, 1);
                        Ok(())
                    }
                    Key::Named(NamedKey::Home) if self.modifiers.shift_key() => {
                        let pane = &mut self.panes[front_tab(&self.groups)];
                        pane.view.extend_to_line_edge(&mut pane.editor, false);
                        Ok(())
                    }
                    Key::Named(NamedKey::End) if self.modifiers.shift_key() => {
                        let pane = &mut self.panes[front_tab(&self.groups)];
                        pane.view.extend_to_line_edge(&mut pane.editor, true);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        self.panes[front_tab(&self.groups)].editor.move_left();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        self.panes[front_tab(&self.groups)].editor.move_right();
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowUp) if self.modifiers.alt_key() => {
                        let pane = &mut self.panes[front_tab(&self.groups)];
                        pane.view.add_cursor_vertically(&mut pane.editor, -1);
                        Ok(())
                    }
                    Key::Named(NamedKey::ArrowDown) if self.modifiers.alt_key() => {
                        let pane = &mut self.panes[front_tab(&self.groups)];
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
                        // The escape key cancels the selection and the box of
                        // text with it: one caret is left.
                        self.cancel_the_selection();
                        Ok(())
                    }
                    Key::Named(NamedKey::Home) => {
                        let pane = &mut self.panes[front_tab(&self.groups)];
                        pane.view.move_to_line_edge(&mut pane.editor, false);
                        Ok(())
                    }
                    Key::Named(NamedKey::End) => {
                        let pane = &mut self.panes[front_tab(&self.groups)];
                        pane.view.move_to_line_edge(&mut pane.editor, true);
                        Ok(())
                    }
                    Key::Named(NamedKey::Enter) => self.panes[front_tab(&self.groups)].editor.insert("\n"),
                    Key::Character(character)
                        if self.modifiers.control_key() && character.eq_ignore_ascii_case("z") =>
                    {
                        self.panes[front_tab(&self.groups)].editor.undo()
                    }
                    Key::Character(character)
                        if self.modifiers.control_key() && character.eq_ignore_ascii_case("y") =>
                    {
                        self.panes[front_tab(&self.groups)].editor.redo()
                    }
                    _ if self.input.preedit().is_empty()
                        && !self.modifiers.control_key()
                        && let Some(text) = event.text
                        && !text.chars().any(char::is_control) =>
                    {
                        self.panes[front_tab(&self.groups)].editor.insert(&text)
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
                // The breakpoints follow the text: a breakpoint that was above
                // a line that was added is on the line the text is on now.
                self.follow_the_breakpoints();
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
                // The task talks on a thread of its own as well.
                if self.pump_task() {
                    window.request_redraw();
                }
                // The debug adapter talks on a thread of its own as well.
                if self.pump_debug() {
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
                // What the editor says about itself goes into the log of the
                // run, once per change.
                self.log_the_status_if_it_changed();
                // The status bar is read before the renderer is borrowed.
                let status = self.status_bar_text();
                // The lines of the developer panel are read before the
                // renderer is borrowed as well.
                let developer_lines = self.developer.as_ref().map(|_| self.developer_lines());
                // The output of the task is read before the renderer is
                // borrowed as well.
                let task_panel = self.task_panel.then(|| TaskPanel {
                    lines: &self.task_output.lines,
                    state: &self.task_output.state,
                    title: &self.task_title,
                    input: self.terminal_input.as_ref().map(|box_| box_.field()),
                    focused: self.terminal_has_focus,
                });
                // The panel of the debugger is read before the renderer is
                // borrowed as well.
                let debug_panel = self.debug.as_ref().map(|debug| DebugPanel {
                    state: debug.state,
                    output: &debug.output,
                    variables: &debug.variables,
                    frame: debug.frame.as_deref(),
                    frames: &debug.frames,
                    selected_frame: debug.selected_frame,
                    program: &debug.program,
                });
                // The breakpoints in the margin belong to the file that the
                // column shows, so they are worked out with the columns.
                let scale_factor = window.scale_factor();
                // Where the parts of the window are, and what the side bar
                // shows, are worked out before the renderer is borrowed.
                let size = self.window_size(scale_factor);
                let layout = self.window_layout(size);
                let sidebar_rows = self.sidebar_rows(layout.sidebar_content());
                // The title of the side bar is worked out before the renderer
                // is borrowed, like the rows are.
                let sidebar_title = self.sidebar_title();
                let badges = self.badges();
                // The columns of the window: each of them shows a file of its
                // own. They are worked out before the renderer is borrowed.
                let columns = self.columns_of_the_window();
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
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
                // The title of the side bar is worked out before the scene is
                // built: the scene borrows the panes of the window.
                let scene = build_scene(
                    renderer,
                    layout,
                    &mut self.panes,
                    &columns,
                    &self.input,
                    Chrome {
                        agent: self.agent.as_ref(),
                        agent_error: self.agent_error.as_deref(),
                        status,
                        sidebar: self.sidebar,
                        sidebar_title,
                        sidebar_rows: &sidebar_rows,
                        badges,
                        find: self.find.as_ref(),
                        rename: self.rename_box.as_ref(),
                        completion: self.completion.as_ref(),
                        actions: self.actions.as_ref(),
                        task_panel,
                        active_column: self.groups.active(),
                        prompt: prompt_text.as_deref(),
                        settings: self.settings.as_ref(),
                        git: self.git.as_ref(),
                        commit: self.commit_box.as_ref(),
                        git_box: self.git_box.as_ref(),
                        command: self.command_box.as_ref().map(|box_| box_.field()),
                        condition: self.breakpoint_condition.as_ref().map(|box_| box_.field()),
                        expression: self.debug_expression.as_ref().map(|box_| box_.field()),
                        search: self.search.as_ref(),
                        palette: self.palette.as_ref(),
                        developer: self.developer.as_ref(),
                        developer_lines: developer_lines.as_deref().unwrap_or(&[]),
                        debug: debug_panel,
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

/// The name of a key that has one, written the way the commands write it.
///
/// `F9` is `f9`, `Enter` is `enter`: the name is the same in the command
/// table and in the key that was pressed, so the two can be compared.
fn key_name<T: AsRef<str>>(key: &Key<T>) -> Option<String> {
    let Key::Named(named) = key else {
        return None;
    };
    let name = match named {
        NamedKey::Enter => "enter",
        NamedKey::Escape => "escape",
        NamedKey::Tab => "tab",
        NamedKey::Space => "space",
        NamedKey::Backspace => "backspace",
        NamedKey::Delete => "delete",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::ArrowUp => "arrowup",
        NamedKey::ArrowDown => "arrowdown",
        NamedKey::ArrowLeft => "arrowleft",
        NamedKey::ArrowRight => "arrowright",
        NamedKey::F1 => "f1",
        NamedKey::F2 => "f2",
        NamedKey::F3 => "f3",
        NamedKey::F4 => "f4",
        NamedKey::F5 => "f5",
        NamedKey::F6 => "f6",
        NamedKey::F7 => "f7",
        NamedKey::F8 => "f8",
        NamedKey::F9 => "f9",
        NamedKey::F10 => "f10",
        NamedKey::F11 => "f11",
        NamedKey::F12 => "f12",
        _ => return None,
    };
    Some(name.to_owned())
}

/// The key that was pressed, written the way the commands write it.
///
/// `Ctrl+Shift+P` is `ctrl+shift+p`. A modifier on its own is not a key: it
/// says nothing back, and the panel that waits for a key keeps waiting.
fn keybinding_text<T: AsRef<str>>(modifiers: ModifiersState, key: &Key<T>) -> Option<String> {
    let base = match key_letter(key) {
        Some(letter) => letter.to_lowercase().to_string(),
        None => key_name(key)?,
    };
    let mut text = String::new();
    if modifiers.control_key() {
        text.push_str("ctrl+");
    }
    if modifiers.alt_key() {
        text.push_str("alt+");
    }
    if modifiers.shift_key() {
        text.push_str("shift+");
    }
    text.push_str(&base);
    Some(text)
}

/// The parts of the frame that are not the text body.
struct Chrome<'a> {
    agent: Option<&'a AgentSession>,
    agent_error: Option<&'a str>,
    /// The status bar at the bottom of the window.
    status: StatusBar,
    /// The section of the side bar that is on screen.
    sidebar: SideBar,
    /// The title of that section.
    ///
    /// It is the name of the section, unless a workspace is open: then it is
    /// the name of the workspace.
    sidebar_title: String,
    /// The rows of that section.
    sidebar_rows: &'a [SideBarRow],
    /// The numbers that are drawn on the activities.
    badges: Badges,
    /// The find bar, when it is open.
    find: Option<&'a FindBar>,
    /// The box that asks for the new name, when it is open.
    rename: Option<&'a RenameBox>,
    /// The list of completions, when it is open.
    completion: Option<&'a CompletionPopup>,
    /// The things the server offers to do, when the list is open.
    actions: Option<&'a ActionPopup>,
    /// The output of the task, when the panel is open.
    task_panel: Option<TaskPanel<'a>>,
    /// The column that has the keyboard.
    ///
    /// The panels that float over the window are drawn over it, and the
    /// keys go to the file it shows.
    active_column: usize,
    /// The question the window is asking, if it is asking one.
    prompt: Option<&'a str>,
    /// The settings panel, when it is open.
    settings: Option<&'a Settings>,
    /// The panel of the changes of the repository, when it is open.
    git: Option<&'a GitPanel>,
    /// The box that asks for the message of a commit, when it is open.
    commit: Option<&'a RenameBox>,
    /// The box that asks git something, when it is open.
    ///
    /// It asks for the name of a branch to make, or whether the branch may
    /// be pushed. It is a box of its own, and it is drawn over the panel of
    /// the changes: a name that is typed into the void is a name that is
    /// nowhere.
    git_box: Option<&'a GitBox>,
    /// The box that asks for a command, when it is open.
    command: Option<&'a RenameBox>,
    /// The box that asks for the condition of a breakpoint, when it is open.
    condition: Option<&'a RenameBox>,
    /// The box that asks for an expression, when it is open.
    expression: Option<&'a RenameBox>,
    /// The panel that finds text in the files, when it is open.
    search: Option<&'a SearchPanel>,
    /// The panel that lists the commands, when it is open.
    palette: Option<&'a CommandPalette>,
    /// The panel that shows what the editor is doing inside, when it is open.
    developer: Option<&'a DeveloperPanel>,
    /// The lines of that panel, as they are drawn.
    developer_lines: &'a [DeveloperLine],
    /// The panel of the debugger, when there is a session.
    debug: Option<DebugPanel<'a>>,
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

/// Whether the key is the enter key, whatever the keyboard layout says.
///
/// A machine with a Chinese input method does not name the keys that are
/// pressed with a modifier: the virtual key code is the same on every
/// layout, so it is the second opinion.
fn is_enter<T: AsRef<str>>(key: &Key<T>) -> bool {
    match key {
        Key::Named(NamedKey::Enter) => true,
        // VK_RETURN, the key the enter is on.
        Key::Unidentified(NativeKey::Windows(code)) => *code == 0x0D,
        _ => false,
    }
}

/// Draws the chrome of one column: its strip of tabs and the row that says
/// where the file is.
///
/// The strip of tabs and the path of the file are part of the window, not of
/// the text: they are drawn around it. The strip is laid out here so that the
/// place of a tab is worked out the same way twice — once to draw it, once to
/// click it.
fn push_the_chrome_of_a_column(
    scene: &mut FrameScene,
    viewport: Rect,
    layout: &ColumnLayout,
    tabs: &[(Tab, bool)],
    active: usize,
    crumbs: &[breadcrumbs::Crumb],
    ui_measurer: &mut RendererMeasurer,
) {
    let strip = shell::TabStrip::layout(tabs, active, layout.tab_bar, ui_measurer);
    push_tab_bar(scene, viewport, layout.tab_bar, &strip.items);

    // The path of the file that is on screen is above the text: it says where
    // the person is, and every piece of it is a place that can be clicked.
    let pieces = breadcrumbs::strip(layout.breadcrumbs, crumbs, ui_measurer);
    breadcrumbs::draw(scene, viewport, layout.breadcrumbs, &pieces);
}

/// Draws the text of one column: the ground, the line numbers, the map and
/// the text itself.
#[allow(clippy::too_many_arguments)]
fn push_the_text_of_a_column(
    scene: &mut FrameScene,
    viewport: Rect,
    layout: &ColumnLayout,
    pane: &mut EditorPane,
    overlay: FindOverlay,
    breakpoints: &[BreakpointMark],
    input: &TextInputState,
    measurer: &mut RendererMeasurer,
) {
    let body = layout.body;
    // The pieces of the pane are borrowed apart: the text is drawn with the
    // view of the file, and the path with the marks of the file.
    let EditorPane {
        editor,
        view,
        highlights,
        marks,
        ..
    } = pane;

    // Only the renderer knows how the text is shaped, so the layout is
    // measured through it.
    view.layout(
        editor.buffer(),
        body.size.width,
        body.size.height,
        measurer,
    );

    // The highlights were made on the highlighter's own thread; they are the
    // ones that belong to the text that is on screen.
    scene.push_quad(body, Color::rgb(0.075, 0.086, 0.11), viewport);
    scene.push_quad(
        Rect::new(body.origin.x, body.origin.y, 3.0, body.size.height),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );

    // The line numbers are on the left of the text.
    let current_line = editor.buffer().line_of_offset(editor.caret().0);
    let numbers = gutter::lines(
        editor.buffer(),
        view.visual_lines(),
        layout.gutter,
        current_line,
        view.zoom().line_height(),
    );
    gutter::draw(
        scene,
        viewport,
        layout.gutter,
        &numbers,
        view.zoom().gutter_font_size(),
        view.zoom().line_height(),
        measurer,
    );

    // The map of the whole document is on the right of the text. A column
    // that is too narrow for it has none: the text is worth more.
    let first_line = view.view.first_line;
    let visible_lines = ((body.size.height / view.zoom().line_height()).floor() as usize).max(1);
    if let Some(minimap) = layout.minimap {
        let map = view.minimap(editor.buffer());
        minimap::draw(scene, viewport, minimap, map, first_line, visible_lines);
    }

    view.draw(
        scene,
        editor,
        input.preedit(),
        body,
        BodyOverlay {
            find: overlay,
            highlights: highlights.as_slice(),
            marks: marks.as_slice(),
        },
        measurer,
    );

    // The breakpoints of the file that is on screen are drawn in the margin
    // of the text: a breakpoint that cannot be seen is a breakpoint nobody
    // knows about. A breakpoint with a condition is drawn in another colour:
    // it stops the program only sometimes.
    for mark in breakpoints {
        let index = mark.line as usize;
        if index == 0 || index > editor.buffer().line_count() {
            continue;
        }
        let offset = editor.buffer().line_range(index - 1).start.0;
        if let Some((_, y)) = view.point_of(offset) {
            let colour = if mark.conditional {
                Color::rgb(0.95, 0.68, 0.25)
            } else {
                Color::rgb(0.90, 0.35, 0.35)
            };
            scene.push_quad(
                Rect::new(layout.gutter.origin.x + 4.0, body.origin.y + y + 6.0, 8.0, 8.0),
                colour,
                layout.gutter,
            );
        }
    }
}

/// What one column of the window shows, without the text itself.
struct ColumnData {
    /// The tab in front of the column, by its place in the tab bar.
    tab: usize,
    /// The tabs of the column, with the flag that says which of them have
    /// changes that are not on disk yet.
    tabs: Vec<(Tab, bool)>,
    /// Which of them is in front.
    active: usize,
    /// The breakpoints of the file that the column shows, as they are drawn
    /// in its margin.
    breakpoints: Vec<BreakpointMark>,
}

fn build_scene(
    renderer: &mut Renderer,
    layout: WindowLayout,
    panes: &mut [EditorPane],
    columns: &[ColumnData],
    input: &TextInputState,
    chrome: Chrome<'_>,
) -> FrameScene {
    let size = layout.size;
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let mut scene = FrameScene::default();

    // The column that has the keyboard. The panels that float over the
    // window are drawn over it.
    let active_column = chrome.active_column;
    let body = layout
        .columns
        .get(active_column)
        .map(|column| column.body)
        .unwrap_or(viewport);

    // A panel that is drawn over the window hides what is under it.
    //
    // The renderer draws every rectangle first and every text after it, so a
    // text that lies under the panel would be drawn *on* it. The window under
    // the panel is therefore left out of the picture, not covered by it.
    if let Some(settings) = chrome.settings {
        push_settings(&mut scene, viewport, size, settings);
        return scene;
    }

    // The panel that shows what the editor is doing inside covers the
    // window, the way the settings do.
    if let Some(panel) = chrome.developer {
        developer::draw(&mut scene, viewport, size, panel, chrome.developer_lines);
        return scene;
    }

    // The panel that lists the commands covers the window, the way the
    // settings do.
    if let Some(palette) = chrome.palette {
        let mut ui_measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);
        command_palette::draw(&mut scene, viewport, palette, &mut ui_measurer);
        return scene;
    }

    // The panel of the changes covers the window, the way the settings do.
    if let Some(panel) = chrome.git {
        let mut ui_measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);
        if chrome.commit.is_some() || chrome.git_box.is_some() {
            // Only the ground of the panel is drawn while a box is open: the
            // text of the panel would lie on top of the box.
            git::draw_background(&mut scene, viewport);
        } else {
            git::draw_panel(&mut scene, viewport, size, panel, &mut ui_measurer);
        }
        // The box that asks for the message of the commit floats over it.
        push_rename_box(&mut scene, size, body, chrome.commit, &mut ui_measurer);
        // The box that asks git something floats over it as well: the name of
        // a branch and the question about a push are written there.
        push_rename_box(
            &mut scene,
            size,
            body,
            chrome.git_box.map(|box_| &box_.input),
            &mut ui_measurer,
        );
        return scene;
    }

    // The panel that finds text in the files covers the window, the way the
    // panel of the changes does.
    if let Some(panel) = chrome.search {
        let mut ui_measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);
        search::draw_panel(&mut scene, viewport, size, panel, &mut ui_measurer);
        return scene;
    }

    // The matches the find bar found, and the one it is on. They belong to
    // the column that has the keyboard: the find bar floats over it.
    let (matches, current_match) = match chrome.find {
        Some(find) => (find.matches(), Some(find.current_index())),
        None => (NO_MATCHES, None),
    };
    let find_bar = chrome
        .find
        .map(|find| find_bar_rect(body, find.replacing()));

    // The chrome of the columns: the strips of tabs and the rows that say where
    // the files are.
    {
        // The strip of tabs is drawn in the sans-serif face, so it gets a
        // measurer of its own.
        let mut ui_measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);
        for (index, column) in columns.iter().enumerate() {
            let Some(place) = layout.columns.get(index) else {
                continue;
            };
            let Some(pane) = panes.get(column.tab) else {
                continue;
            };
            push_the_chrome_of_a_column(
                &mut scene,
                viewport,
                place,
                &column.tabs,
                column.active,
                &pane.crumbs,
                &mut ui_measurer,
            );
        }
    }

    // The text of the columns. Each of them has its own text, its own line
    // numbers and its own map of the document: a window with two columns
    // shows two files, not one file twice.
    {
        // Only the renderer knows how the text is shaped, so the layout is
        // measured through it.
        let mut measurer = RendererMeasurer::new(renderer, TextFamily::Monospace);
        for (index, column) in columns.iter().enumerate() {
            let Some(place) = layout.columns.get(index) else {
                continue;
            };
            let Some(pane) = panes.get_mut(column.tab) else {
                continue;
            };
            let overlay = if index == active_column {
                FindOverlay {
                    // The find bar floats over the body, so the text under it
                    // is left out.
                    bar: find_bar,
                    matches,
                    current: current_match,
                }
            } else {
                FindOverlay {
                    bar: None,
                    matches: NO_MATCHES,
                    current: None,
                }
            };
            push_the_text_of_a_column(
                &mut scene,
                viewport,
                place,
                pane,
                overlay,
                &column.breakpoints,
                input,
                &mut measurer,
            );
            // The line between two columns: the eye needs to know where one
            // column ends and the other begins.
            if index > 0 {
                scene.push_quad(
                    Rect::new(place.column.origin.x - SEPARATOR, 0.0, SEPARATOR, size.height),
                    Color::rgb(0.16, 0.18, 0.22),
                    viewport,
                );
            }
        }
    }

    push_agent_panel(
        &mut scene,
        size,
        &layout,
        chrome.agent,
        chrome.agent_error,
        chrome.conflicts.as_ref(),
    );

    // The find bar is drawn in the sans-serif face, so it gets a measurer of
    // its own.
    let mut ui_measurer = RendererMeasurer::new(renderer, TextFamily::SansSerif);

    // The bar with the activities is on the left of the window, and the
    // section that was picked is on its right.
    let bar = activity_bar::ActivityBar::layout(
        layout.activity_bar,
        chrome.sidebar.selected(),
        &chrome.badges,
    );
    activity_bar::draw(&mut scene, viewport, layout.activity_bar, &bar.items);
    if chrome.sidebar.is_open() {
        sidebar::draw(
            &mut scene,
            viewport,
            layout.sidebar,
            &chrome.sidebar_title,
            chrome.sidebar_rows,
            &mut ui_measurer,
        );
    }

    // The status bar is at the bottom of the window.
    status_bar::draw(
        &mut scene,
        viewport,
        layout.status_bar,
        &chrome.status,
        &mut ui_measurer,
    );

    // The question the window is asking is drawn last, over everything.
    if let Some(prompt) = chrome.prompt {
        push_prompt(&mut scene, viewport, size, prompt);
    }

    push_find_bar(&mut scene, size, body, chrome.find, &mut ui_measurer);

    // The box that asks for the new name floats over the body. The box that
    // asks for a command, the box that asks for the condition of a
    // breakpoint, the box that asks for an expression and the box that asks
    // git something are the same box, and only one of them is open.
    push_rename_box(
        &mut scene,
        size,
        body,
        chrome
            .rename
            .or(chrome.command)
            .or(chrome.condition)
            .or(chrome.expression)
            .or(chrome.git_box.map(|box_| &box_.input)),
        &mut ui_measurer,
    );

    // The list of completions floats under the caret. It belongs to the column
    // that has the keyboard: the caret is in that column.
    let active_tab = columns.get(active_column).map(|column| column.tab);
    if let Some(popup) = chrome.completion
        && let Some(tab) = active_tab
        && let Some(pane) = panes.get_mut(tab)
        && let Some(point) = pane.view.point_of(pane.editor.caret().0)
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

    // The list of the things the server offers floats under the caret as
    // well.
    if let Some(popup) = chrome.actions
        && let Some(tab) = active_tab
        && let Some(pane) = panes.get_mut(tab)
        && let Some(point) = pane.view.point_of(pane.editor.caret().0)
    {
        code_action::draw(
            &mut scene,
            Rect::new(0.0, 0.0, size.width, size.height),
            body,
            popup,
            point,
            &mut ui_measurer,
        );
    }
    // The output of the task is at the bottom of the window.
    if let Some(panel) = chrome.task_panel {
        push_task_panel(&mut scene, size, &panel, &mut ui_measurer);
    }
    // The panel of the debugger is at the bottom of the window as well.
    if let Some(panel) = chrome.debug {
        debug::draw_panel(&mut scene, viewport, size, &panel);
    }

    scene
}

/// The tabs of a column, with the flag that says which of them have changes.
///
/// They are the tabs of **one** column: a column has its own tabs, and the
/// ones of the other column are not on its strip.
fn tabs_of_a_column(tabs: &TabBar, groups: &Groups, column: usize) -> Vec<(Tab, bool)> {
    groups
        .tabs_of(column)
        .iter()
        .filter_map(|place| {
            let tab = tabs.tabs().get(*place)?;
            Some((tab.clone(), tab.dirty))
        })
        .collect()
}

/// Which of the tabs of a column is in front.
fn front_of_a_column(groups: &Groups, column: usize) -> usize {
    let Some(front) = groups.front_of(column) else {
        return 0;
    };
    groups
        .tabs_of(column)
        .iter()
        .position(|place| *place == front)
        .unwrap_or(0)
}

/// The tab in front of the active column, by its place in the tab bar.
///
/// It is a free function and not a method of the window on purpose: the
/// panes of the window and the groups of the window are two different
/// fields, and borrowing one of them must not borrow the other.
fn front_tab(groups: &Groups) -> usize {
    groups.front().unwrap_or(0)
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
    let rows = settings.visible_rows();
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
        "Enter 下一个 · Shift+Enter 上一个 · Tab 切换 · Alt+Enter 全选匹配 · Alt+C 大小写",
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

/// The place of the box that asks for the new name.
///
/// The box sits in the top left corner of the body: it is in the way of
/// nothing, and it is where the eye goes first when it is told to rename
/// something.
fn rename_box_rect(body: Rect) -> Rect {
    let width = 380.0f32.min(body.size.width.max(1.0));
    Rect::new(body.origin.x + 8.0, body.origin.y + 8.0, width, 40.0)
}

/// Draws the box that asks for the new name.
fn push_rename_box(
    scene: &mut FrameScene,
    size: Size,
    body: Rect,
    rename: Option<&RenameBox>,
    measurer: &mut dyn TextMeasurer,
) {
    let Some(rename) = rename else {
        return;
    };
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let box_ = rename_box_rect(body);

    scene.push_quad(box_, Color::rgb(0.10, 0.12, 0.16), viewport);
    scene.push_quad(
        Rect::new(box_.origin.x, box_.origin.y, box_.size.width, 1.0),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );

    // The name that is being renamed, in a quieter colour.
    let label = rename.title();
    let label_width = measurer.measure(&label, 13.0).width;
    scene.push_text(
        label,
        Rect::new(
            box_.origin.x + 12.0,
            box_.origin.y + 11.0,
            label_width + 4.0,
            20.0,
        ),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.55, 0.60, 0.68),
            font_size: 13.0,
            line_height: 20.0,
            family: TextFamily::SansSerif,
        },
    );

    // The field the new name is typed into.
    let field = Rect::new(
        box_.origin.x + 16.0 + label_width,
        box_.origin.y + 8.0,
        (box_.size.width - label_width - 32.0).max(1.0),
        24.0,
    );
    scene.push_quad(field, Color::rgb(0.06, 0.07, 0.09), viewport);
    scene.push_quad(
        Rect::new(field.origin.x, field.origin.y, field.size.width, 1.0),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );

    let text = rename.text();
    let before = text.get(..rename.caret().min(text.len())).unwrap_or("");
    let width_before = measurer.measure(before, 14.0).width;
    scene.push_text(
        text.to_owned(),
        Rect::new(
            field.origin.x + 6.0,
            field.origin.y + 2.0,
            (field.size.width - 12.0).max(1.0),
            (field.size.height - 4.0).max(1.0),
        ),
        TextStyle {
            clip: field,
            color: Color::rgb(0.86, 0.89, 0.94),
            font_size: 14.0,
            line_height: 20.0,
            family: TextFamily::SansSerif,
        },
    );
    // The caret stands where the user put it, not at the end of the name.
    scene.push_quad(
        Rect::new(
            field.origin.x + 6.0 + width_before + 1.0,
            field.origin.y + 4.0,
            1.5,
            (field.size.height - 8.0).max(1.0),
        ),
        Color::rgb(0.30, 0.58, 0.96),
        field,
    );
}

/// The place of the output panel of the task.
///
/// The panel is at the bottom of the window, under the text of the document
/// and above the status bar.
fn task_panel_rect(size: Size) -> Rect {
    Rect::new(
        0.0,
        size.height - layout::STATUS_BAR_HEIGHT - TASK_PANEL_HEIGHT,
        size.width,
        TASK_PANEL_HEIGHT,
    )
}

/// Draws the output panel of the task.
///
/// The panel says what the task was asked to do and what it is doing now,
/// and under that the lines the task has said, the newest at the bottom.
/// When the task takes input, the line that is being typed is under them.
fn push_task_panel(
    scene: &mut FrameScene,
    size: Size,
    panel: &TaskPanel<'_>,
    measurer: &mut dyn TextMeasurer,
) {
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let rect = task_panel_rect(size);
    scene.push_quad(rect, Color::rgb(0.055, 0.063, 0.082), viewport);
    scene.push_quad(
        Rect::new(rect.origin.x, rect.origin.y, rect.size.width, 1.0),
        Color::rgb(0.16, 0.18, 0.22),
        viewport,
    );

    let failed = TaskPanel::is_failure(panel.state);
    let title = format!(
        "{} · {}",
        panel.title,
        TaskPanel::state_text(panel.state)
    );
    scene.push_text(
        title,
        Rect::new(
            rect.origin.x + 12.0,
            rect.origin.y + 8.0,
            (rect.size.width - 24.0).max(1.0),
            20.0,
        ),
        TextStyle {
            clip: viewport,
            color: if failed {
                Color::rgb(0.93, 0.58, 0.55)
            } else {
                Color::rgb(0.72, 0.78, 0.88)
            },
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );

    // The lines the task has said, the newest at the bottom.
    let height = 18.0;
    // The room the lines have is smaller when there is a line being typed
    // under them.
    let taken = if panel.input.is_some() { 28.0 } else { 0.0 };
    let room = ((rect.size.height - 44.0 - taken) / height).floor().max(1.0) as usize;
    let shown = panel.lines.len().min(room);
    let first = panel.lines.len() - shown;
    for (index, line) in panel.lines.iter().skip(first).enumerate() {
        let y = rect.origin.y + 32.0 + index as f32 * height;
        scene.push_text(
            line.clone(),
            Rect::new(
                rect.origin.x + 12.0,
                y,
                (rect.size.width - 24.0).max(1.0),
                height,
            ),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.78, 0.81, 0.87),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    }

    // The line that is being typed into the task.
    if let Some(input) = panel.input {
        let y = rect.origin.y + rect.size.height - 26.0;
        let prompt = "> ";
        let text = input.text();
        scene.push_text(
            format!("{prompt}{text}"),
            Rect::new(
                rect.origin.x + 12.0,
                y,
                (rect.size.width - 24.0).max(1.0),
                20.0,
            ),
            TextStyle {
                clip: viewport,
                color: if panel.focused {
                    Color::rgb(0.86, 0.89, 0.94)
                } else {
                    // The keyboard is not here: the line is dimmer.
                    Color::rgb(0.48, 0.52, 0.60)
                },
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
        // The caret stands where the person put it, not at the end.
        if panel.focused {
            let prompt_width = measurer.measure(prompt, 13.0).width;
            let before = measurer.measure(&text[..input.caret()], 13.0).width;
            scene.push_quad(
                Rect::new(
                    rect.origin.x + 12.0 + prompt_width + before,
                    y + 1.0,
                    1.5,
                    16.0,
                ),
                Color::rgb(0.30, 0.58, 0.96),
                rect,
            );
        }
    }
}

/// Draws the agent panel on the right of the window.
fn push_agent_panel(
    scene: &mut FrameScene,
    size: Size,
    layout: &WindowLayout,
    agent: Option<&AgentSession>,
    agent_error: Option<&str>,
    conflict: Option<&ConflictView>,
) {
    let viewport = Rect::new(0.0, 0.0, size.width, size.height);
    let panel = layout.agent_panel;
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

    use crate::testing::{scratch, scratch_dir, scratch_project, scratch_project_without_build};
    use codeweaver_lsp::protocol::CodeAction;
    use codeweaver_text::TextOffset;
    use codeweaver_ui::MonospaceMeasurer;

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
        // Ctrl+F is the key of a command: the command route opens the bar.
        app.run_the_command_of_the_key(&character("f"));
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
    fn clicking_an_activity_picks_the_section_of_the_side_bar() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        let layout = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 1);

        // The second activity is the search.
        let point = Point { x: 24.0, y: 60.0 };
        app.press_activity_bar(point, &layout);
        assert_eq!(app.sidebar.section, Activity::Search);
        assert!(app.sidebar.is_open(), "the side bar is shown");
    }

    #[test]
    fn picking_the_same_activity_again_hides_the_side_bar() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        let layout = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 1);

        // The search is not the section that is on screen: picking it shows
        // it.
        let point = Point { x: 24.0, y: 60.0 };
        app.press_activity_bar(point, &layout);
        assert!(app.sidebar.is_open());
        // Picking it again hides the side bar.
        app.press_activity_bar(point, &layout);
        assert!(!app.sidebar.is_open(), "the side bar is hidden");
    }

    #[test]
    fn the_keyboard_walks_through_the_activities() {
        let mut app = NativeApplication::new(None).expect("the application starts");

        // Ctrl+Shift+E gives the keyboard to the activity bar.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("e")));
        assert!(app.activity_focus, "the activity bar has the keyboard");

        app.modifiers = ModifiersState::default();
        assert!(app.handle_activity_key(&named(NamedKey::ArrowDown)));
        assert_eq!(app.sidebar.section, Activity::Search);
        assert!(app.handle_activity_key(&named(NamedKey::ArrowUp)));
        assert_eq!(app.sidebar.section, Activity::Files);

        // The keyboard goes back to the document.
        assert!(app.handle_activity_key(&named(NamedKey::Escape)));
        assert!(!app.activity_focus);
    }

    #[test]
    fn what_is_typed_does_not_go_into_the_document_while_the_activity_bar_has_the_keyboard() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("e")));
        app.modifiers = ModifiersState::default();

        // The activity bar eats the key: the document does not see it.
        assert!(app.handle_activity_key(&character("x")));
        assert_eq!(app.panes[front_tab(&app.groups)].editor.text(), "");
    }

    #[test]
    fn the_side_bar_can_be_hidden_and_shown_again() {
        let mut app = NativeApplication::new(None).expect("the application starts");

        app.modifiers = ModifiersState::CONTROL;
        assert!(app.run_the_command_of_the_key(&character("b")));
        assert!(!app.sidebar.is_open(), "the side bar is hidden");
        assert!(app.run_the_command_of_the_key(&character("b")));
        assert!(app.sidebar.is_open(), "the side bar is shown");
    }

    #[test]
    fn the_status_bar_says_where_the_caret_is() {
        let directory = scratch_dir("status-bar");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\n").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");

        let status = app.status_bar_text();
        assert!(status.right.contains("行 1，列 1"), "got {}", status.right);
        assert!(status.middle.contains("Buffer v"), "got {}", status.middle);
        assert!(status.left.contains("main.rs"), "got {}", status.left);

        // The caret moves down: the status bar says so.
        app.panes[0].editor.set_carets(&[TextOffset(4)]);
        let status = app.status_bar_text();
        assert!(status.right.contains("行 2，列 1"), "got {}", status.right);
    }

    #[test]
    fn the_path_of_the_file_that_is_open_is_said_above_the_text() {
        let project = scratch_project("breadcrumbs");
        let folder = project.join("src");
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("main.rs");
        fs::write(&file, "one\ntwo\n").unwrap();
        let app = NativeApplication::new(Some(file)).expect("the application starts");

        let labels: Vec<&str> = app.panes[0].crumbs.iter().map(|crumb| crumb.label.as_str()).collect();
        assert_eq!(labels.last(), Some(&"main.rs"), "got {labels:?}");
        assert!(labels.contains(&"src"), "got {labels:?}");
        assert!(
            labels.contains(&"breadcrumbs"),
            "the project is in the path, got {labels:?}"
        );
    }

    #[test]
    fn clicking_a_folder_of_the_path_makes_the_file_tree_look_at_it() {
        let project = scratch_project("breadcrumbs-click");
        let folder = project.join("src");
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("main.rs");
        fs::write(&file, "one\ntwo\n").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        let layout = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 1);

        let mut measurer = MonospaceMeasurer::new(8.0);
        let strip = breadcrumbs::strip(layout.column(0).expect("there is a column").breadcrumbs, &app.panes[0].crumbs, &mut measurer);
        let index = strip
            .crumbs
            .iter()
            .position(|crumb| crumb.label == "src")
            .expect("the folder is in the path");
        let place = strip.crumbs[index].rect;
        app.go_to_the_place_of_the_path(
            &strip,
            Point {
                x: place.origin.x + 1.0,
                y: place.origin.y + 4.0,
            },
        );

        let root = app
            .tree
            .as_ref()
            .expect("the tree of files is there")
            .roots()
            .first()
            .expect("the tree looks at one folder")
            .to_path_buf();
        assert_eq!(root, folder, "the tree looks at the folder of the path");
        assert!(app.sidebar.is_open(), "the side bar shows the files");
    }

    #[test]
    fn clicking_the_file_of_the_path_says_that_it_is_open() {
        let project = scratch_project("breadcrumbs-file");
        let file = project.join("main.rs");
        fs::write(&file, "one\ntwo\n").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        let layout = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 1);

        let mut measurer = MonospaceMeasurer::new(8.0);
        let strip = breadcrumbs::strip(layout.column(0).expect("there is a column").breadcrumbs, &app.panes[0].crumbs, &mut measurer);
        let last = strip.crumbs.last().expect("the file is in the path");
        let place = last.rect;
        app.go_to_the_place_of_the_path(
            &strip,
            Point {
                x: place.origin.x + 1.0,
                y: place.origin.y + 4.0,
            },
        );

        assert!(
            app.status
                .as_deref()
                .is_some_and(|status| status.contains("已经在这个文件里")),
            "got {:?}",
            app.status
        );
    }

    #[test]
    fn the_path_of_the_file_in_front_is_the_one_that_is_shown() {        let project = scratch_project("breadcrumbs-tabs");
        let first = project.join("first.rs");
        let second = project.join("second.rs");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();
        let mut app = NativeApplication::new(Some(first)).expect("the application starts");
        app.open_file(&second);

        // The path belongs to the file, so the file that is in front has its
        // own path above the text.
        let front = front_tab(&app.groups);
        let labels: Vec<&str> = app.panes[front]
            .crumbs
            .iter()
            .map(|crumb| crumb.label.as_str())
            .collect();
        assert_eq!(labels.last(), Some(&"second.rs"), "got {labels:?}");
    }

    #[test]
    fn clicking_the_map_jumps_to_that_part_of_the_file() {
        let directory = scratch_dir("minimap");
        let file = directory.join("main.rs");
        let text = (0..200)
            .map(|index| format!("line {index}\n"))
            .collect::<String>();
        fs::write(&file, text).unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        let layout = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 1);

        // The middle of the map is the middle of the file. The map of a
        // short file does not fill the whole strip, so the middle of the map
        // is not the middle of the strip.
        let pane = &mut app.panes[0];
        let geometry = pane
            .view
            .minimap(pane.editor.buffer())
            .geometry(layout.column(0).expect("there is a column").minimap.expect("a wide column has a map"));
        let point = Point {
            x: layout.column(0).expect("there is a column").minimap.expect("a wide column has a map").origin.x + 10.0,
            y: layout.column(0).expect("there is a column").minimap.expect("a wide column has a map").origin.y + geometry.height / 2.0,
        };
        app.press_minimap(point, 0, &layout);
        let first = app.panes[0].view.view.first_line;
        assert!(
            first > 50 && first < 150,
            "the view is in the middle of the file, got {first}"
        );
    }

    #[test]
    fn the_map_is_built_again_when_the_text_changed() {
        let directory = scratch_dir("minimap-version");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");

        let pane = &mut app.panes[0];
        assert_eq!(pane.view.minimap(pane.editor.buffer()).len(), 3);
        // The text changed: the map is built again, and it knows the new
        // line.
        pane.editor.insert("\nfour").expect("the text goes in");
        assert_eq!(pane.view.minimap(pane.editor.buffer()).len(), 4);
    }

    #[test]
    fn the_line_numbers_are_the_numbers_of_the_lines_of_the_document() {
        let directory = scratch_dir("gutter");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let app = NativeApplication::new(Some(file)).expect("the application starts");

        let window = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 1);
        let buffer = app.panes[0].editor.buffer();
        let mut measurer = MonospaceMeasurer::new(8.0);
        let text = codeweaver_ui::TextLayout::build(
            buffer,
            0,
            10,
            400.0,
            16.0,
            app.zoom.line_height(),
            &mut measurer,
        );
        let numbers = gutter::lines(buffer, &text.lines, window.column(0).expect("there is a column").gutter, 0, app.zoom.line_height());
        assert_eq!(
            numbers.iter().map(|line| line.number).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn the_command_makes_the_text_bigger_and_the_size_is_remembered() {
        let directory = scratch_dir("zoom-command");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        // The size is remembered in the file of the check, not in the file of
        // the person who runs the checks.
        app.zoom_path = Some(directory.join("settings.json"));

        assert_eq!(app.zoom.percent, 100.0);
        assert!(app.run_named_command("view.zoomIn"));
        assert_eq!(app.zoom.percent, 110.0);
        let size = app.panes[0].view.zoom().font_size();
        assert!((size - 17.6).abs() < 0.01, "the text is bigger, got {size}");
        let written = fs::read_to_string(directory.join("settings.json"))
            .expect("the size is remembered in the file");
        assert!(written.contains("110"), "got {written}");

        assert!(app.run_named_command("view.zoomOut"));
        assert_eq!(app.zoom.percent, 100.0);
        assert!(app.run_named_command("view.zoomReset"));
        assert_eq!(app.zoom.percent, 100.0);
    }

    #[test]
    fn the_size_of_the_text_is_changed_in_the_panel_and_the_limits_are_left_alone() {
        let directory = scratch_dir("zoom-panel");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.zoom_path = Some(directory.join("settings.json"));

        app.open_settings();
        // The rows are: the limit of the bytes, the limit of the lines, the
        // size of the text, and then the keys.
        app.handle_settings_key(&named(NamedKey::ArrowDown));
        app.handle_settings_key(&named(NamedKey::ArrowDown));
        assert_eq!(
            app.settings
                .as_ref()
                .expect("the panel is open")
                .kind_of_the_picked(),
            Some(SettingKind::FontSize),
            "the size of the text is the third row"
        );
        app.handle_settings_key(&named(NamedKey::ArrowRight));
        assert_eq!(app.zoom.percent, 110.0, "the text is bigger");
        let size = app.panes[0].view.zoom().font_size();
        assert!((size - 17.6).abs() < 0.01, "the text is drawn bigger, got {size}");

        // The limit of the bytes is changed: the size of the text stays where
        // the person put it.
        app.handle_settings_key(&named(NamedKey::ArrowUp));
        app.handle_settings_key(&named(NamedKey::ArrowUp));
        assert_eq!(
            app.settings
                .as_ref()
                .expect("the panel is open")
                .kind_of_the_picked(),
            Some(SettingKind::Bytes),
            "the limit of the bytes is the first row"
        );
        app.handle_settings_key(&named(NamedKey::ArrowRight));
        assert_eq!(app.zoom.percent, 110.0, "the size of the text did not change");
    }

    #[test]
    fn a_file_that_is_opened_later_is_drawn_at_the_size_of_the_window() {
        let directory = scratch_dir("zoom-later");
        let first = directory.join("first.rs");
        let second = directory.join("second.rs");
        fs::write(&first, "one\ntwo").unwrap();
        fs::write(&second, "three\nfour").unwrap();
        let mut app = NativeApplication::new(Some(first)).expect("the application starts");
        app.zoom_path = Some(directory.join("settings.json"));
        app.run_named_command("view.zoomIn");

        assert!(app.open_file(&second));
        let last = app.panes.len() - 1;
        assert_eq!(
            app.panes[last].view.zoom().percent,
            110.0,
            "the file that is opened later is drawn at the size of the window"
        );
    }

    #[test]
    fn the_numbers_of_the_lines_stay_on_their_lines_when_the_text_is_bigger() {
        let directory = scratch_dir("zoom-gutter");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.zoom_path = Some(directory.join("settings.json"));
        app.run_named_command("view.zoomIn");

        let window = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 1);
        let buffer = app.panes[0].editor.buffer();
        let mut measurer = MonospaceMeasurer::new(8.0);
        let line_height = app.zoom.line_height();
        let text = codeweaver_ui::TextLayout::build(
            buffer,
            0,
            10,
            400.0,
            app.zoom.font_size(),
            line_height,
            &mut measurer,
        );
        let numbers = gutter::lines(buffer, &text.lines, window.column(0).expect("there is a column").gutter, 0, line_height);
        // The second number is one row of the text lower than the first one:
        // the numbers follow the size of the text.
        let distance = numbers[1].rect.origin.y - numbers[0].rect.origin.y;
        assert!(
            (distance - line_height).abs() < 0.01,
            "the numbers keep the rows of the text, got {distance} and {line_height}"
        );
    }

    #[test]
    fn the_find_key_opens_the_find_bar() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        // Ctrl+F is the key of a command: the command route opens the bar.
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.run_the_command_of_the_key(&character("f")));
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

    /// An application with a file that is open, and the panel of the search
    /// open on it.
    fn app_with_search() -> NativeApplication {
        let directory = scratch_project("search-app");
        let file = directory.join("notes.txt");
        fs::write(&file, "the needle is here\n").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_search_key(&character("f"), None);
        app.modifiers = ModifiersState::default();
        app
    }

    #[test]
    fn the_search_key_opens_the_panel() {
        let app = app_with_search();
        assert!(app.search.is_some(), "the panel of the search is open");
    }

    #[test]
    fn the_panel_of_the_search_takes_the_keys_while_it_is_open() {
        let mut app = app_with_search();
        assert!(app.handle_search_key(&character("n"), Some("n")));
        let panel = app.search.as_ref().expect("the panel is open");
        assert_eq!(panel.needle(), "n");
        assert_eq!(
            app.editor().text(),
            "the needle is here\n",
            "the word did not go into the document"
        );
    }

    #[test]
    fn the_escape_key_closes_the_panel_of_the_search() {
        let mut app = app_with_search();
        assert!(app.handle_search_key(&named(NamedKey::Escape), None));
        assert!(app.search.is_none());
    }

    #[test]
    fn a_panel_does_not_open_over_another_one() {
        let mut app = app_with_search();
        // The box of the command does not open over the panel of the search.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(!app.handle_command_key(&character("p"), None));
        assert!(app.command_box.is_none());
        // And the panel of the changes does not open over it either.
        assert!(!app.handle_git_key(&character("g"), None));
        assert!(app.git.is_none());
    }

    #[test]
    fn the_search_opens_the_place_that_is_picked() {
        let directory = scratch_project("search-opens");
        let file = directory.join("notes.txt");
        fs::write(&file, "the first line\nthe needle is here\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_search_key(&character("f"), None);
        app.modifiers = ModifiersState::default();
        for letter in ["n", "e", "e", "d", "l", "e"] {
            app.handle_search_key(&character(letter), Some(letter));
        }
        app.handle_search_key(&named(NamedKey::Enter), None);

        // The answer comes in on the thread of the search.
        for _ in 0..200 {
            if app.pump_search() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let panel = app.search.as_ref().expect("the panel is open");
        assert_eq!(panel.hits().len(), 1, "the word was found");

        // The focus is on the places now, so the enter key opens one of them.
        assert!(app.handle_search_key(&named(NamedKey::Enter), None));
        assert!(app.search.is_none(), "the panel is closed");
        let expected = app.editor().buffer().line_range(1).start.0;
        assert_eq!(
            app.editor().caret().0,
            expected,
            "the caret is on the line that was found"
        );
    }

    #[test]
    fn a_search_in_a_workspace_looks_in_every_root() {
        let (directory, file) = a_workspace_file("search-in-a-workspace");
        fs::write(directory.join("first").join("one.txt"), "the needle is here\n").unwrap();
        fs::write(directory.join("second").join("two.txt"), "the needle is there\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.search_needle = "needle".to_owned();
        app.open_search_panel(false);
        let panel = app.search.as_mut().expect("the panel of the search is open");
        panel.start_search();

        // The answer comes in on the thread of the search.
        for _ in 0..200 {
            if app.pump_search() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let panel = app.search.as_ref().expect("the panel is open");
        let names: Vec<&str> = panel
            .hits()
            .iter()
            .filter_map(|hit| hit.path.file_name())
            .filter_map(|name| name.to_str())
            .collect();
        assert_eq!(names.len(), 2, "both roots are searched, got {names:?}");
        assert!(names.contains(&"one.txt"), "the first root is searched");
        assert!(names.contains(&"two.txt"), "the second root is searched");
    }

    #[test]
    fn a_search_without_a_workspace_looks_in_the_project_of_the_open_file() {
        let project = scratch_project("search-without-a-workspace");
        let file = project.join("notes.txt");
        fs::write(&file, "the needle is here\n").unwrap();
        let other = project.join("more");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("two.txt"), "the needle is there\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.search_needle = "needle".to_owned();
        app.open_search_panel(false);
        let panel = app.search.as_mut().expect("the panel of the search is open");
        panel.start_search();
        for _ in 0..200 {
            if app.pump_search() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let panel = app.search.as_ref().expect("the panel is open");
        assert_eq!(
            panel.hits().len(),
            2,
            "the whole project is searched, not one folder"
        );
    }

    #[test]
    fn a_breakpoint_follows_the_line_that_was_added_above_it() {
        let directory = scratch_dir("breakpoint-follows");
        let file = directory.join("notes.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        let uri = app.uri_of_the_open_file().expect("the file has an address");
        // The caret is on the third line.
        app.pane_mut().editor.set_carets(&[TextOffset(8)]);
        app.toggle_breakpoint();
        assert_eq!(app.breakpoints.lines_of(&uri), vec![3]);

        // A line is put above the breakpoint.
        app.pane_mut().editor.set_carets(&[TextOffset(0)]);
        app.pane_mut()
            .editor
            .insert("zero\n")
            .expect("the text fits");
        app.follow_the_breakpoints();

        assert_eq!(
            app.breakpoints.lines_of(&uri),
            vec![4],
            "the breakpoint is on the line the text is on now"
        );
    }

    #[test]
    fn the_condition_key_asks_for_the_condition_of_the_breakpoint() {
        let mut app = app_with_file("breakpoint-condition");
        let uri = app.uri_of_the_open_file().expect("the file has an address");
        app.toggle_breakpoint();
        assert_eq!(app.breakpoints.lines_of(&uri), vec![1]);

        app.modifiers = ModifiersState::CONTROL;
        assert!(app.handle_breakpoint_condition_key(&named(NamedKey::F9), None));
        assert!(
            app.breakpoint_condition.is_some(),
            "the box that asks for the condition is open"
        );
        assert_eq!(app.breakpoint_condition_line, Some(1));
    }

    #[test]
    fn a_condition_that_is_written_is_remembered() {
        let mut app = app_with_file("breakpoint-condition-written");
        let uri = app.uri_of_the_open_file().expect("the file has an address");
        app.toggle_breakpoint();

        app.modifiers = ModifiersState::CONTROL;
        app.handle_breakpoint_condition_key(&named(NamedKey::F9), None);
        app.modifiers = ModifiersState::default();
        for letter in ["c", ">", "3"] {
            app.handle_breakpoint_condition_key(&character(letter), Some(letter));
        }
        app.handle_breakpoint_condition_key(&named(NamedKey::Enter), None);

        assert_eq!(app.breakpoints.condition_of(&uri, 1), Some("c>3"));
        assert!(
            app.breakpoint_condition.is_none(),
            "the box is closed"
        );
    }

    #[test]
    fn an_empty_condition_takes_the_condition_away() {
        let mut app = app_with_file("breakpoint-condition-empty");
        let uri = app.uri_of_the_open_file().expect("the file has an address");
        app.toggle_breakpoint();
        app.breakpoints.set_condition(&uri, 1, Some("c>3"));

        // The box opens with the condition that is there.
        app.modifiers = ModifiersState::CONTROL;
        app.handle_breakpoint_condition_key(&named(NamedKey::F9), None);
        app.modifiers = ModifiersState::default();
        let box_ = app.breakpoint_condition.as_ref().expect("the box is there");
        assert_eq!(box_.text(), "c>3");

        // The condition is taken away by erasing it and confirming.
        for _ in 0..3 {
            app.handle_breakpoint_condition_key(&named(NamedKey::Backspace), None);
        }
        app.handle_breakpoint_condition_key(&named(NamedKey::Enter), None);
        assert_eq!(app.breakpoints.condition_of(&uri, 1), None);
        assert_eq!(
            app.breakpoints.lines_of(&uri),
            vec![1],
            "the breakpoint is still there"
        );
    }

    #[test]
    fn a_line_without_a_breakpoint_has_no_condition_to_ask_for() {
        let mut app = app_with_file("breakpoint-condition-none");
        app.modifiers = ModifiersState::CONTROL;
        app.handle_breakpoint_condition_key(&named(NamedKey::F9), None);

        assert!(
            app.breakpoint_condition.is_none(),
            "there is no box to open"
        );
        assert!(
            app.status
                .as_deref()
                .is_some_and(|status| status.contains("还没有断点")),
            "the editor has to say why: {:?}",
            app.status
        );
    }

    #[test]
    fn without_a_debug_session_there_is_nothing_to_evaluate() {
        let mut app = app_with_file("debug-expression-none");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_debug_expression_key(&character("e"), None));
        assert!(
            app.debug_expression.is_none(),
            "there is no box to open"
        );
        assert!(
            app.status
                .as_deref()
                .is_some_and(|status| status.contains("没有调试会话")),
            "the editor has to say why: {:?}",
            app.status
        );
    }

    #[test]
    fn an_empty_expression_is_not_asked_about() {
        let mut app = app_with_file("debug-expression-empty");
        app.evaluate_the_expression("   ");
        assert!(
            app.status
                .as_deref()
                .is_some_and(|status| status.contains("不能是空的")),
            "the editor has to say why: {:?}",
            app.status
        );
    }

    #[test]
    fn the_expression_box_takes_the_keys_that_are_typed() {
        let mut app = app_with_file("debug-expression-typed");
        app.debug_expression = Some(CommandBox::titled("求值"));

        for letter in ["c", "o", "u", "n", "t"] {
            assert!(app.handle_debug_expression_key(&character(letter), Some(letter)));
        }
        let box_ = app.debug_expression.as_ref().expect("the box is there");
        assert_eq!(box_.text(), "count");

        // The box closes when the expression is asked about.
        app.handle_debug_expression_key(&named(NamedKey::Enter), None);
        assert!(app.debug_expression.is_none(), "the box is closed");
        assert!(
            app.status
                .as_deref()
                .is_some_and(|status| status.contains("没有调试会话")),
            "the editor has to say why it cannot ask: {:?}",
            app.status
        );
    }

    #[test]
    fn the_text_is_replaced_in_the_files_that_were_found() {
        let directory = scratch_project("search-replaces");
        let file = directory.join("notes.txt");
        fs::write(&file, "the needle is here\nand the needle is there\n").unwrap();
        let other = directory.join("other.txt");
        fs::write(&other, "a needle here\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        // The panel is opened to find and to replace.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_search_key(&character("h"), None);
        app.modifiers = ModifiersState::default();
        for letter in ["n", "e", "e", "d", "l", "e"] {
            app.handle_search_key(&character(letter), Some(letter));
        }
        app.handle_search_key(&named(NamedKey::Enter), None);

        // The search runs on a thread of its own: the answer comes later.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app
            .search
            .as_ref()
            .is_some_and(|panel| panel.is_running())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the search did not finish"
            );
            app.pump_search();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let hits = app
            .search
            .as_ref()
            .expect("the panel is open")
            .hits()
            .to_vec();
        // The text is in two files, and in three places: the file that is
        // open holds it twice.
        let files: std::collections::HashSet<_> = hits.iter().map(|hit| &hit.path).collect();
        assert_eq!(files.len(), 2, "the text is in two files");
        assert_eq!(hits.len(), 3, "the text is in three places");

        // The search is done: the focus is on the places that were found.
        // The tab key goes on to the replacement.
        app.handle_search_key(&named(NamedKey::Tab), None);
        for letter in ["p", "i", "n"] {
            app.handle_search_key(&character(letter), Some(letter));
        }
        app.handle_search_key(&named(NamedKey::Enter), None);

        // The file that is open changes in its buffer.
        assert_eq!(
            app.editor().text(),
            "the pin is here\nand the pin is there\n"
        );
        // The file that is not open goes to disk.
        assert_eq!(
            fs::read_to_string(&other).unwrap(),
            "a pin here\n",
            "the file on disk was changed"
        );
        // One undo takes the whole change back.
        app.editor_mut().undo().expect("the change can be taken back");
        assert_eq!(app.editor().text(), "the needle is here\nand the needle is there\n");
    }

    #[test]
    fn a_file_that_does_not_hold_the_text_any_more_is_not_changed() {
        let directory = scratch_project("search-replaces-changed");
        let file = directory.join("notes.txt");
        fs::write(&file, "the needle is here\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        // The search finds the text.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_search_key(&character("f"), None);
        app.modifiers = ModifiersState::default();
        for letter in ["n", "e", "e", "d", "l", "e"] {
            app.handle_search_key(&character(letter), Some(letter));
        }
        app.handle_search_key(&named(NamedKey::Enter), None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app
            .search
            .as_ref()
            .is_some_and(|panel| panel.is_running())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the search did not finish"
            );
            app.pump_search();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // The file changes after the search: it does not hold the text any
        // more, and the replacement leaves it alone rather than change it in
        // the wrong place.
        app.editor_mut().load_text("something else\n");
        let panel = app.search.take().expect("the panel is open");
        let (files, places, skipped) = app.replace_in_the_files(&panel, "needle", "pin");
        assert_eq!(files, 0, "the file that changed was not changed");
        assert_eq!(places, 0);
        assert_eq!(skipped, 1, "the file that changed was left alone");
        assert_eq!(app.editor().text(), "something else\n");
    }

    #[test]
    fn the_place_of_a_breakpoint_is_released_when_it_is_taken_away() {        let directory = scratch_dir("breakpoint-released");
        let file = directory.join("notes.txt");
        fs::write(&file, "one\ntwo\n").unwrap();

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.pane_mut().editor.set_carets(&[TextOffset(4)]);
        app.toggle_breakpoint();
        let with = app.pane_mut().editor.buffer().anchor_count();

        app.toggle_breakpoint();
        let without = app.pane_mut().editor.buffer().anchor_count();

        assert_eq!(
            without + 1,
            with,
            "the place of the breakpoint was let go"
        );
    }

    #[test]
    fn the_breakpoints_of_a_file_that_is_opened_again_follow_the_text() {
        let directory = scratch_dir("breakpoint-reopened");
        let file = directory.join("notes.txt");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();

        let mut app = NativeApplication::new(Some(file.clone())).expect("the application starts");
        let uri = app.uri_of_the_open_file().expect("the file has an address");
        app.pane_mut().editor.set_carets(&[TextOffset(8)]);
        app.toggle_breakpoint();

        // The file is closed and opened again: the places of the breakpoints
        // died with the buffer of the closed file.
        app.close_tab(0);
        app.open_file(&file);
        assert_eq!(app.breakpoints.lines_of(&uri), vec![3]);

        // A line is put above the breakpoint.
        app.pane_mut().editor.set_carets(&[TextOffset(0)]);
        app.pane_mut()
            .editor
            .insert("zero\n")
            .expect("the text fits");
        app.follow_the_breakpoints();

        assert_eq!(app.breakpoints.lines_of(&uri), vec![4]);
    }

    /// An application with a file that is open.
    fn app_with_file(name: &str) -> NativeApplication {
        let directory = scratch_dir(name);
        let file = directory.join("notes.txt");
        fs::write(&file, "one\ntwo\n").unwrap();
        NativeApplication::new(Some(file)).expect("the application starts")
    }

    #[test]
    fn the_terminal_key_opens_the_terminal() {
        let mut app = app_with_file("terminal-opens");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_terminal_key(&character("t"), None));
        assert!(app.terminal_input.is_some(), "the terminal is there");
        assert!(app.terminal_has_focus, "the terminal has the keyboard");
        assert!(app.task_panel, "the panel of the task is open");
    }

    #[test]
    fn what_is_typed_goes_into_the_terminal_and_not_into_the_document() {
        let mut app = app_with_file("terminal-types");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_terminal_key(&character("t"), None);
        app.modifiers = ModifiersState::default();

        assert!(app.handle_terminal_key(&character("e"), Some("e")));
        let terminal = app.terminal_input.as_ref().expect("the terminal is there");
        assert_eq!(terminal.text(), "e");
        assert_eq!(
            app.editor().text(),
            "one\ntwo\n",
            "the letter did not go into the document"
        );
    }

    #[test]
    fn the_escape_key_gives_the_keyboard_back_to_the_document() {
        let mut app = app_with_file("terminal-escape");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_terminal_key(&character("t"), None);
        app.modifiers = ModifiersState::default();

        assert!(app.handle_terminal_key(&named(NamedKey::Escape), None));
        assert!(
            !app.terminal_has_focus,
            "the keyboard is the document's again"
        );
        assert!(
            app.terminal_input.is_some(),
            "the terminal is still there: the shell goes on"
        );

        // What is typed now goes into the document. The terminal lets the key
        // go: what the document does with it is the window's business, so
        // what the terminal is asked for is that it does not take the key.
        assert!(!app.handle_terminal_key(&character("x"), Some("x")));
        let terminal = app.terminal_input.as_ref().expect("the terminal is there");
        assert_eq!(terminal.text(), "", "the letter did not go into the terminal");
        assert_eq!(
            app.editor().text(),
            "one\ntwo\n",
            "the terminal did not put the letter into the document either"
        );
    }

    #[test]
    fn the_space_bar_goes_into_the_terminal_and_not_into_the_document() {
        let mut app = app_with_file("terminal-space");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_terminal_key(&character("t"), None);
        app.modifiers = ModifiersState::default();

        // The window system names the space bar instead of making it a
        // character: without this the space falls through to the document,
        // and no command can have an argument.
        assert!(app.handle_terminal_key(&named(NamedKey::Space), None));
        let terminal = app.terminal_input.as_ref().expect("the terminal is there");
        assert_eq!(terminal.text(), " ");
        assert_eq!(
            app.editor().text(),
            "one\ntwo\n",
            "the space did not go into the document"
        );
    }

    #[test]
    fn the_terminal_key_takes_the_keyboard_back() {
        let mut app = app_with_file("terminal-back");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_terminal_key(&character("t"), None);
        app.modifiers = ModifiersState::default();
        app.handle_terminal_key(&named(NamedKey::Escape), None);
        assert!(!app.terminal_has_focus);

        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_terminal_key(&character("t"), None));
        assert!(app.terminal_has_focus, "the keyboard is in the terminal again");
        app.modifiers = ModifiersState::default();
        assert!(app.handle_terminal_key(&character("e"), Some("e")));
        let terminal = app.terminal_input.as_ref().expect("the terminal is there");
        assert_eq!(terminal.text(), "e");
    }

    #[test]
    fn a_task_cannot_start_while_the_terminal_is_open() {
        let mut app = app_with_file("terminal-task");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_terminal_key(&character("t"), None);

        app.start_build_task();
        assert!(
            app.status
                .as_deref()
                .is_some_and(|status| status.contains("终端")),
            "the editor has to say why: {:?}",
            app.status
        );
    }

    #[test]
    fn the_terminal_gives_the_keyboard_up_when_a_shortcut_is_pressed() {
        let mut app = app_with_file("terminal-shortcut");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_terminal_key(&character("t"), None);
        app.modifiers = ModifiersState::default();

        // Ctrl and a letter: the shortcut belongs to the editor.
        app.modifiers = ModifiersState::CONTROL;
        assert!(!app.handle_terminal_key(&character("f"), Some("f")));
        assert!(
            !app.terminal_has_focus,
            "the keyboard is the editor's again, so what is typed goes where the shortcut opened"
        );
    }

    #[test]
    fn the_enter_key_sends_the_line_to_the_shell() {
        let mut app = app_with_file("terminal-sends");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        app.handle_terminal_key(&character("t"), None);
        app.modifiers = ModifiersState::default();
        for letter in ["e", "c", "h", "o"] {
            app.handle_terminal_key(&character(letter), Some(letter));
        }
        // The window system names the space bar: without it the command
        // could not have an argument.
        app.handle_terminal_key(&named(NamedKey::Space), None);
        for letter in ["h", "e", "l", "l", "o"] {
            app.handle_terminal_key(&character(letter), Some(letter));
        }
        app.handle_terminal_key(&named(NamedKey::Enter), None);

        // The shell says it on a thread of its own.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let output = app
                .task
                .as_ref()
                .expect("the shell is running")
                .output();
            if output.lines.iter().any(|line| line.contains("hello")) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the shell did not say it: {:?}",
                output.lines
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
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
        assert_eq!(front_tab(&app.groups), 1, "the file that was opened is in front");
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
        assert_eq!(front_tab(&app.groups), 1);

        // The first file is opened again.
        assert!(app.open_file(&first));

        assert_eq!(app.tabs.len(), 2, "the file is not opened twice");
        assert_eq!(app.panes.len(), 2, "and it does not get a second buffer");
        assert_eq!(front_tab(&app.groups), 0, "the tab that was there comes to the front");
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
        assert_eq!(front_tab(&app.groups), 0);
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
        assert_eq!(tree.roots(), vec![directory.as_path()]);
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

    /// A workspace file with two folders in it, and the folders themselves.
    ///
    /// The folders are called `first` and `second`, and the second one is
    /// given the name 乙 in the workspace file.
    fn a_workspace_file(name: &str) -> (PathBuf, PathBuf) {
        let directory = scratch_dir(name);
        let first = directory.join("first");
        let second = directory.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("one.txt"), "one\n").unwrap();
        fs::write(second.join("two.txt"), "two\n").unwrap();
        let file = directory.join("project.code-workspace");
        fs::write(
            &file,
            "{ \"folders\": [ { \"path\": \"first\" }, { \"path\": \"second\", \"name\": \"乙\" } ] }",
        )
        .unwrap();
        (directory, file)
    }

    /// The names of the rows of the tree of files.
    fn the_names_of_the_tree(app: &NativeApplication) -> Vec<String> {
        app.tree
            .as_ref()
            .expect("the tree of files is there")
            .nodes()
            .iter()
            .map(|node| node.name.clone())
            .collect()
    }

    #[test]
    fn a_workspace_file_that_is_opened_names_the_folders_of_the_project() {
        let (_, file) = a_workspace_file("workspace-open");
        let mut app = NativeApplication::new(None).expect("the application starts");

        assert!(app.open_file(&file));

        assert_eq!(
            the_names_of_the_tree(&app),
            vec!["first", "one.txt", "乙", "two.txt"]
        );
        let said = app.status.clone().expect("the editor says what happened");
        assert!(said.contains("project"), "got {said}");
        assert!(said.contains("2 个文件夹"), "got {said}");
    }

    #[test]
    fn a_workspace_file_is_not_opened_as_a_text_file() {
        let (_, file) = a_workspace_file("workspace-not-text");
        let mut app = NativeApplication::new(None).expect("the application starts");
        let before = app.tabs.len();

        app.open_file(&file);

        assert_eq!(app.tabs.len(), before, "the workspace is not a tab of text");
    }

    #[test]
    fn the_side_bar_says_the_name_of_the_workspace() {
        let (_, file) = a_workspace_file("workspace-title");
        let mut app = NativeApplication::new(None).expect("the application starts");

        app.open_file(&file);

        assert_eq!(app.sidebar_title(), "project");
    }

    #[test]
    fn the_status_bar_says_which_workspace_is_open() {
        let (_, file) = a_workspace_file("workspace-status");
        let mut app = NativeApplication::new(None).expect("the application starts");

        app.open_file(&file);

        let status = app.status_bar_text();
        assert!(
            status.right.contains("工作区 project"),
            "got {}",
            status.right
        );
    }

    #[test]
    fn a_workspace_that_cannot_be_read_says_why() {
        let directory = scratch_dir("workspace-broken");
        let file = directory.join("project.code-workspace");
        fs::write(&file, "{ not json").unwrap();
        let mut app = NativeApplication::new(None).expect("the application starts");

        assert!(!app.open_file(&file));

        let said = app.status.clone().expect("the editor says what is wrong");
        assert!(said.contains("JSON"), "got {said}");
    }

    #[test]
    fn a_folder_of_the_workspace_that_is_not_there_is_said_to_be_missing() {
        let directory = scratch_dir("workspace-missing-folder");
        let file = directory.join("project.code-workspace");
        fs::write(&file, "{ \"folders\": [ { \"path\": \"nowhere\" } ] }").unwrap();
        let mut app = NativeApplication::new(None).expect("the application starts");

        app.open_file(&file);

        let said = app.status.clone().expect("the editor says what happened");
        assert!(said.contains("nowhere"), "got {said}");
        assert!(said.contains("不在了"), "got {said}");
    }

    #[test]
    fn the_tree_of_a_workspace_does_not_follow_the_file_that_is_opened() {
        let (directory, file) = a_workspace_file("workspace-tree");
        let other = directory.join("other.txt");
        fs::write(&other, "other\n").unwrap();
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.open_file(&file);
        let before = the_names_of_the_tree(&app);

        app.open_file(&other);

        assert_eq!(the_names_of_the_tree(&app), before);
    }

    #[test]
    fn the_workspace_is_written_back_into_its_own_file() {
        let (_, file) = a_workspace_file("workspace-save");
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.open_file(&file);

        assert!(app.run_named_command("workspace.save"));

        let said = app.status.clone().expect("the editor says what happened");
        assert!(said.contains("project.code-workspace"), "got {said}");
        let contents = workspace_file::read(&file).expect("the file is read");
        assert_eq!(contents.folders.len(), 2);
        assert_eq!(contents.folders[0].path, "first");
        assert_eq!(contents.folders[1].name.as_deref(), Some("乙"));
    }

    #[test]
    fn a_project_without_a_workspace_gets_one_of_its_own() {
        let directory = scratch_dir("workspace-create");
        fs::write(directory.join("one.txt"), "one\n").unwrap();
        let file = directory.join("one.txt");
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");

        assert!(app.run_named_command("workspace.save"));

        let written = directory.join(format!("workspace-create{}", workspace_file::SUFFIX));
        assert!(written.exists(), "the workspace file is there");
        let contents = workspace_file::read(&written).expect("the file is read");
        assert_eq!(contents.folders.len(), 1);
        assert_eq!(contents.folders[0].path, ".");
    }

    #[test]
    fn a_workspace_that_is_opened_on_the_command_line_is_the_tree_of_the_window() {
        let (_, file) = a_workspace_file("workspace-argument");

        let app = NativeApplication::new(Some(file)).expect("the application starts");

        assert_eq!(
            the_names_of_the_tree(&app),
            vec!["first", "one.txt", "乙", "two.txt"]
        );
        assert_eq!(app.sidebar_title(), "project");
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
        assert!(!app.panes[front_tab(&app.groups)].degraded, "the file is small enough");
        assert!(
            !app.panes[front_tab(&app.groups)].highlights.is_empty(),
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
        assert!(app.panes[front_tab(&app.groups)].degraded, "the file is not coloured");
        assert!(app.panes[front_tab(&app.groups)].highlights.is_empty());
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
        // Formatting is a command of a file: a file has to be open for it.
        let path = scratch("format.txt");
        fs::write(&path, "let value = 1;\n").unwrap();
        let mut app = NativeApplication::new(Some(path)).expect("the application starts");
        app.modifiers = ModifiersState::ALT | ModifiersState::SHIFT;

        assert!(app.run_the_command_of_the_key(&character("f")));
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
        assert!(!app.run_the_command_of_the_key(&character("f")));
        app.modifiers = ModifiersState::SHIFT;
        assert!(!app.run_the_command_of_the_key(&character("f")));
    }

    /// The edits of a rename, in the shape the server answers in.
    fn rename_edit(uri: &str, new_text: &str) -> WorkspaceEdit {
        let mut changes = std::collections::BTreeMap::new();
        changes.insert(
            uri.to_owned(),
            vec![TextEdit {
                range: codeweaver_lsp::protocol::Range {
                    start: codeweaver_lsp::protocol::Position::new(0, 3),
                    end: codeweaver_lsp::protocol::Position::new(0, 7),
                },
                new_text: new_text.to_owned(),
            }],
        );
        WorkspaceEdit {
            changes,
            document_changes: Vec::new(),
            resource_operations: Vec::new(),
        }
    }

    #[test]
    fn the_edits_of_a_rename_go_into_the_buffer() {
        let path = scratch("rename.txt");
        fs::write(&path, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        let uri = codeweaver_lsp::protocol::file_uri(&path);
        app.panes[0].document_uri = Some(uri.clone());
        let before = app.editor().version();

        let request = RenameRequest {
            uri: uri.clone(),
            version: before,
            new_name: "run".to_owned(),
        };
        assert!(
            app.apply_rename(rename_edit(&uri, "run"), &request),
            "the file changed"
        );
        assert_eq!(app.editor().text(), "fn run() {}\n");
        assert_eq!(
            app.editor().version(),
            before + 1,
            "the edits of one file are one change of the document"
        );
        // One undo takes the whole renaming back.
        app.editor_mut().undo().unwrap();
        assert_eq!(app.editor().text(), "fn main() {}\n");
    }

    #[test]
    fn a_rename_answer_for_another_text_is_not_put_in() {
        let path = scratch("rename-stale.txt");
        fs::write(&path, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        let uri = codeweaver_lsp::protocol::file_uri(&path);
        app.panes[0].document_uri = Some(uri.clone());
        let before = app.editor().version();

        // The request was made for another version of the text.
        let request = RenameRequest {
            uri: uri.clone(),
            version: before + 1,
            new_name: "run".to_owned(),
        };
        assert!(!app.apply_rename(rename_edit(&uri, "run"), &request));
        assert_eq!(app.editor().text(), "fn main() {}\n");
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("变了"),
            "the editor has to say why the renaming was not used"
        );
    }

    #[test]
    fn a_rename_of_a_file_that_is_not_open_goes_to_the_disk() {
        let directory = scratch_dir("rename-other");
        let main = directory.join("main.txt");
        let other = directory.join("other.txt");
        fs::write(&main, "fn main() {}\n").unwrap();
        fs::write(&other, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(main.clone())).expect("the application starts");
        let main_uri = codeweaver_lsp::protocol::file_uri(&main);
        let other_uri = codeweaver_lsp::protocol::file_uri(&other);
        app.panes[0].document_uri = Some(main_uri.clone());
        let before = app.editor().version();

        // The renaming reaches the file that is open and the file that is
        // not open in the editor.
        let mut edit = rename_edit(&main_uri, "run");
        edit.changes.insert(other_uri.clone(), edit.changes[&main_uri].clone());

        let request = RenameRequest {
            uri: main_uri.clone(),
            version: before,
            new_name: "run".to_owned(),
        };
        assert!(app.apply_rename(edit, &request), "the renaming happened");
        assert_eq!(app.editor().text(), "fn run() {}\n");
        assert_eq!(
            fs::read_to_string(&other).unwrap(),
            "fn run() {}\n",
            "the file that is not open in the editor was written to the disk"
        );
    }

    #[test]
    fn a_rename_that_does_not_fit_the_file_is_not_put_in() {
        let path = scratch("rename-mismatch.txt");
        fs::write(&path, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        let uri = codeweaver_lsp::protocol::file_uri(&path);
        app.panes[0].document_uri = Some(uri.clone());
        let before = app.editor().version();

        // The place of the edit is not in the file.
        let mut changes = std::collections::BTreeMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: codeweaver_lsp::protocol::Range {
                    start: codeweaver_lsp::protocol::Position::new(9, 0),
                    end: codeweaver_lsp::protocol::Position::new(9, 4),
                },
                new_text: "run".to_owned(),
            }],
        );
        let request = RenameRequest {
            uri: uri.clone(),
            version: before,
            new_name: "run".to_owned(),
        };
        assert!(
            !app.apply_rename(
                WorkspaceEdit {
                    changes,
                    document_changes: Vec::new(),
                    resource_operations: Vec::new(),
                },
                &request
            )
        );
        assert_eq!(app.editor().text(), "fn main() {}\n");
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("对不上"),
            "the editor has to say why the renaming was not used"
        );
    }

    #[test]
    fn the_rename_key_says_when_there_is_no_server() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.editor_mut().load_text("let value = 1;\n");
        // The caret is in a name, so the command can be used at all.
        app.pane_mut().editor.set_carets(&[TextOffset(5)]);

        assert!(app.run_the_command_of_the_key(&named(NamedKey::F2)));
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("没有语言服务器"),
            "the editor has to say why it cannot rename"
        );
        assert!(
            app.rename_box.is_none(),
            "there is nothing to rename without a server"
        );
    }

    #[test]
    fn the_rename_key_is_only_the_f2_key() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        assert!(!app.run_the_command_of_the_key(&character("f")));
        assert!(!app.run_the_command_of_the_key(&named(NamedKey::Enter)));
    }

    #[test]
    fn the_box_that_asks_for_the_name_takes_the_keys() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.rename_box = Some(RenameBox::new("value", 4));

        app.handle_rename_box_key(&character("s"), Some("s"));
        let rename = app.rename_box.as_ref().expect("the box is open");
        assert_eq!(rename.text(), "values");

        // The box goes away when the name is confirmed.
        app.handle_rename_box_key(&named(NamedKey::Enter), None);
        assert!(app.rename_box.is_none());
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("没有语言服务器"),
            "the editor has to say why it cannot rename"
        );
    }

    #[test]
    fn the_box_goes_away_when_the_renaming_is_cancelled() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.rename_box = Some(RenameBox::new("value", 4));

        app.handle_rename_box_key(&named(NamedKey::Escape), None);
        assert!(app.rename_box.is_none());
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("取消")
        );
    }

    #[test]
    fn a_name_that_is_not_changed_is_not_sent_to_the_server() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.rename_box = Some(RenameBox::new("value", 4));

        app.handle_rename_box_key(&named(NamedKey::Enter), None);
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("没有变"),
            "the editor has to say that nothing was renamed"
        );
    }
    /// A code action that changes the text of the file that is open.
    fn code_action(uri: &str, new_text: &str) -> CodeAction {
        CodeAction {
            title: "改个名".to_owned(),
            kind: Some("quickfix".to_owned()),
            edit: Some(rename_edit(uri, new_text)),
            command: None,
        }
    }

    /// A code action that only runs a command.
    fn command_action(title: &str) -> CodeAction {
        CodeAction {
            title: title.to_owned(),
            kind: None,
            edit: None,
            command: Some(codeweaver_lsp::protocol::ServerCommand {
                title: title.to_owned(),
                command: "some.command".to_owned(),
                arguments: Vec::new(),
            }),
        }
    }

    #[test]
    fn the_edits_of_a_code_action_go_into_the_buffer() {
        let path = scratch("action.txt");
        fs::write(&path, "fn main() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
        let uri = codeweaver_lsp::protocol::file_uri(&path);
        app.panes[0].document_uri = Some(uri.clone());
        let before = app.editor().version();

        let popup = ActionPopup::new(vec![code_action(&uri, "run")]).expect("there is a list");
        app.apply_code_action(&popup);

        assert_eq!(app.editor().text(), "fn run() {}\n");
        assert_eq!(
            app.editor().version(),
            before + 1,
            "the edits of the action are one change of the document"
        );
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("改个名"),
            "the editor has to say what it did"
        );
    }

    #[test]
    fn a_code_action_that_only_runs_a_command_says_so() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        let popup = ActionPopup::new(vec![command_action("整理导入")]).expect("there is a list");

        app.apply_code_action(&popup);

        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("没有语言服务器"),
            "the editor has to say why the command did not run"
        );
    }

    #[test]
    fn the_list_of_actions_is_picked_with_the_arrow_keys() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        let uri = "file:///tmp/main.rs";
        app.actions = Some(
            ActionPopup::new(vec![code_action(uri, "one"), code_action(uri, "two")])
                .expect("there is a list"),
        );

        app.handle_action_key(&named(NamedKey::ArrowDown));
        let actions = app.actions.as_ref().expect("the list is open");
        assert_eq!(actions.selected(), 1);
        app.handle_action_key(&named(NamedKey::ArrowUp));
        let actions = app.actions.as_ref().expect("the list is open");
        assert_eq!(actions.selected(), 0);
    }

    #[test]
    fn the_build_key_says_when_there_is_nothing_to_build() {
        // A project that has no build command has nothing to build.
        let directory = scratch_project_without_build("build-nothing");
        let file = directory.join("main.py");
        fs::write(&file, "print(1)\n").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;

        assert!(app.run_the_command_of_the_key(&character("b")));
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("没有可以跑的命令"),
            "the editor has to say that there is nothing to run"
        );
        assert!(!app.task_panel, "there is no output without a task");
    }

    #[test]
    fn a_second_task_is_not_started_while_one_is_running() {
        let directory = scratch_dir("task-second");
        let file = directory.join("notes.txt");
        fs::write(&file, "notes\n").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.task = Some(
            Task::start(
                "cmd",
                &["/c".to_owned(), "ping -n 20 127.0.0.1 > nul".to_owned()],
                &directory,
            )
            .expect("the task starts"),
        );
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;

        assert!(app.run_the_command_of_the_key(&character("b")));
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("已经有任务在跑了"),
            "the editor has to say that there is already a task"
        );
    }

    #[test]
    fn the_build_key_is_only_control_and_shift_with_b() {
        let directory = scratch_project_without_build("build-key");
        let file = directory.join("main.py");
        fs::write(&file, "print(1)\n").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");

        // Without the modifiers the key is not the key of the command.
        assert!(!app.run_the_command_of_the_key(&character("b")));
        app.modifiers = ModifiersState::CONTROL;
        // Ctrl+B is the key of the side bar now; the build wants the shift
        // with it.
        assert_ne!(
            app.command_of_the_key(&character("b")).as_deref(),
            Some("task.build")
        );
        // With them the command runs: the editor looks for the build command.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert_eq!(
            app.command_of_the_key(&character("b")).as_deref(),
            Some("task.build")
        );
        // But only with the `b`.
        assert_ne!(
            app.command_of_the_key(&character("a")).as_deref(),
            Some("task.build")
        );
    }

    #[test]
    fn the_output_panel_takes_the_room_at_the_bottom() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        assert_eq!(app.task_panel_height(), 0.0);
        app.task_panel = true;
        assert_eq!(app.task_panel_height(), TASK_PANEL_HEIGHT);
    }

    #[test]
    fn the_output_of_the_task_is_collected() {
        let directory = scratch_dir("task-collect");
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.task = Some(
            Task::start(
                "cmd",
                &[
                    "/c".to_owned(),
                    "echo the task says hello".to_owned(),
                ],
                &directory,
            )
            .expect("the task starts"),
        );

        // The task talks on a thread of its own, so the output is waited for.
        let mut noticed = false;
        for _ in 0..400 {
            if app.pump_task() {
                noticed = true;
            }
            if app
                .task_output
                .lines
                .iter()
                .any(|line| line.contains("the task says hello"))
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(noticed, "the editor has to notice what the task said");
        assert!(
            app.task_output
                .lines
                .iter()
                .any(|line| line.contains("the task says hello")),
            "got {:?}",
            app.task_output.lines
        );
    }

    #[test]
    fn the_escape_key_closes_the_output_of_the_task() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.task_panel = true;

        assert!(app.handle_task_key(&named(NamedKey::Escape)));
        assert!(!app.task_panel, "the panel is closed");
    }

    #[test]
    fn the_edits_of_a_code_action_go_into_the_file_they_were_asked_about() {
        let directory = scratch_dir("action-target");
        let first = directory.join("first.txt");
        let second = directory.join("second.txt");
        fs::write(&first, "fn main() {}\n").unwrap();
        fs::write(&second, "fn other() {}\n").unwrap();

        let mut app = NativeApplication::new(Some(first.clone())).expect("the application starts");
        let first_uri = codeweaver_lsp::protocol::file_uri(&first);
        app.panes[0].document_uri = Some(first_uri.clone());
        // The operations were asked about the first file.
        app.action_target = Some((first_uri.clone(), app.editor().version()));

        // The user looks at the other file before picking one.
        assert!(app.open_file(&second));
        let second_index = front_tab(&app.groups);

        let popup = ActionPopup::new(vec![code_action(&first_uri, "run")]).expect("there is a list");
        app.apply_code_action(&popup);

        assert_eq!(
            app.panes[0].editor.text(),
            "fn run() {}\n",
            "the edit went into the file the operations were asked about"
        );
        assert_eq!(
            app.panes[second_index].editor.text(),
            "fn other() {}\n",
            "the file on screen was not touched"
        );
    }

    #[test]
    fn the_code_action_key_says_when_there_is_no_server() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::ALT;

        assert!(app.handle_action_key(&named(NamedKey::Enter)));
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("没有语言服务器"),
            "the editor has to say why it cannot ask for code actions"
        );
    }

    #[test]
    fn the_code_action_key_is_only_alt_and_enter() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        assert!(!app.handle_action_key(&named(NamedKey::Enter)));
        app.modifiers = ModifiersState::ALT;
        assert!(!app.handle_action_key(&character("a")));
    }

    #[test]
    fn the_list_of_actions_goes_away_when_it_is_cancelled() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        let uri = "file:///tmp/main.rs";
        app.actions = Some(
            ActionPopup::new(vec![code_action(uri, "one")]).expect("there is a list"),
        );

        app.handle_action_key(&named(NamedKey::Escape));
        assert!(app.actions.is_none());
        assert!(
            app.status
                .as_ref()
                .expect("the editor said something")
                .contains("取消")
        );
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
        assert_eq!(front_tab(&app.groups), 1);

        // The strip is laid out and hit the same way as it is drawn.
        let size = Size::new(1200.0, 800.0);
        let layout = WindowLayout::of(size, 0.0, true, 1);
        let mut measurer = MonospaceMeasurer::new(13.0);
        let tabs = tabs_of_a_column(&app.tabs, &app.groups, 0);
        let active = front_of_a_column(&app.groups, 0);
        let place = layout.column(0).expect("there is a column");
        let strip = shell::TabStrip::layout(&tabs, active, place.tab_bar, &mut measurer);
        let first_tab = strip.items[0].rect;
        let point = Point {
            x: first_tab.origin.x + 4.0,
            y: first_tab.origin.y + 4.0,
        };

        app.activate_tab_at(point, &strip);

        assert_eq!(front_tab(&app.groups), 0, "the tab that was clicked is in front");
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
            app.panes[front_tab(&app.groups)].policy.max_bytes,
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
        assert!(app.panes[front_tab(&app.groups)].degraded, "the file is too big");

        // The limit is raised again.
        app.set_highlight_policy(HighlightPolicy::default());

        assert!(wait_for_syntax(&mut app), "the highlighter answers again");
        assert!(
            !app.panes[front_tab(&app.groups)].degraded,
            "the file is small enough to be coloured again"
        );
        assert!(
            !app.panes[front_tab(&app.groups)].highlights.is_empty(),
            "the text is coloured again"
        );
    }

    /// A repository with a file in it that the editor can open.
    ///
    /// The file is not one that a language server wants: these checks are
    /// about git and not about the server.
    ///
    /// Nothing comes back when git cannot be started here: the checks that
    /// need it are not run at all.
    fn git_project(name: &str) -> Option<(PathBuf, PathBuf)> {
        if git::git_is_blocked() {
            return None;
        }
        let directory = scratch_dir(name);
        let file = directory.join("notes.txt");
        fs::write(&file, "the first line\n").expect("the file is written");
        git::run(&directory, &["init", "--quiet"]).expect("the repository is made");
        git::run(&directory, &["config", "user.email", "check@example.com"])
            .expect("the email is set");
        git::run(&directory, &["config", "user.name", "check"]).expect("the name is set");
        git::run(&directory, &["add", "notes.txt"]).expect("the file is added");
        git::run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");
        Some((directory, file))
    }

    #[test]
    fn the_git_panel_opens_and_shows_what_changed() {
        let Some((_, file)) = git_project("git-panel") else {
            return;
        };
        fs::write(&file, "the second line\n").expect("the file is changed");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();

        let panel = app.git.as_ref().expect("the panel is open");
        assert_eq!(panel.entries.len(), 1, "the changed file is in the panel");
        assert_eq!(panel.entries[0].path, "notes.txt");
        assert!(!panel.entries[0].staged, "the change is not staged yet");
        assert!(
            panel.diff.contains("the second line"),
            "the difference is in the panel: {}",
            panel.diff
        );
    }

    #[test]
    fn the_letter_s_stages_the_file_that_is_picked() {
        let Some((_, file)) = git_project("git-stage-key") else {
            return;
        };
        fs::write(&file, "the second line\n").expect("the file is changed");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        assert!(app.handle_git_key(&character("s"), None));

        let panel = app.git.as_ref().expect("the panel is still open");
        assert!(panel.entries[0].staged, "the change is staged");
    }

    #[test]
    fn the_panel_commits_what_was_staged() {
        let Some((directory, file)) = git_project("git-commit-key") else {
            return;
        };
        fs::write(&file, "the second line\n").expect("the file is changed");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("s"), None);
        // C opens the box that asks for the message of the commit.
        app.handle_git_key(&character("c"), None);
        assert!(app.commit_box.is_some(), "the box asks for the message");

        app.handle_git_key(&character("o"), None);
        app.handle_git_key(&character("k"), None);
        app.handle_git_key(&named(NamedKey::Enter), None);

        assert!(app.commit_box.is_none(), "the box is closed");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("提交了"), "got {status}");
        let panel = app.git.as_ref().expect("the panel is still open");
        assert!(panel.entries.is_empty(), "got {:?}", panel.entries);

        // The commit is really in the repository.
        let said = git::run(&directory, &["log", "--oneline"]).expect("the log is read");
        assert!(said.contains("ok"), "got {said}");
    }

    #[test]
    fn an_empty_message_is_not_committed() {
        let Some((_, file)) = git_project("git-empty-message") else {
            return;
        };
        fs::write(&file, "the second line\n").expect("the file is changed");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("s"), None);
        app.handle_git_key(&character("c"), None);
        app.handle_git_key(&named(NamedKey::Enter), None);

        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("不能是空的"), "got {status}");
    }

    #[test]
    fn the_space_bar_goes_into_the_message_of_the_commit() {
        let Some((_, file)) = git_project("git-commit-space") else {
            return;
        };
        fs::write(&file, "the second line\n").expect("the file is changed");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("s"), None);
        app.handle_git_key(&character("c"), None);
        assert!(app.commit_box.is_some(), "the box asks for the message");

        // The window system names the space bar: without it no message could
        // have a word in it.
        app.handle_git_key(&named(NamedKey::Space), None);
        app.handle_git_key(&character("o"), None);
        app.handle_git_key(&character("k"), None);

        let typed = app
            .commit_box
            .as_ref()
            .expect("the box is open")
            .text()
            .to_owned();
        assert_eq!(typed, " ok");
    }

    #[test]
    fn the_letter_u_takes_the_file_out_of_the_staging_area() {
        let Some((_, file)) = git_project("git-unstage-key") else {
            return;
        };
        fs::write(&file, "the second line\n").expect("the file is changed");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("s"), None);
        let panel = app.git.as_ref().expect("the panel is still open");
        assert!(panel.entries[0].staged, "the change is staged");

        assert!(app.handle_git_key(&character("u"), None));

        let panel = app.git.as_ref().expect("the panel is still open");
        assert_eq!(panel.entries.len(), 1, "the file is still there");
        assert!(
            !panel.entries[0].staged,
            "the change is not staged any more"
        );
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("取消暂存"), "got {status}");
    }

    #[test]
    fn the_letter_u_says_so_when_there_is_nothing_to_unstage() {
        let Some((_, file)) = git_project("git-unstage-nothing") else {
            return;
        };
        fs::write(&file, "the second line\n").expect("the file is changed");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        assert!(app.handle_git_key(&character("u"), None));

        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("没有暂存"), "got {status}");
    }

    #[test]
    fn the_letter_l_shows_the_commits_of_the_repository() {
        let Some((_, file)) = git_project("git-log-key") else {
            return;
        };
        fs::write(&file, "the second line\n").expect("the file is changed");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        assert!(app.handle_git_key(&character("l"), None));

        let panel = app.git.as_ref().expect("the panel is still open");
        assert_eq!(panel.view(), GitView::Commits, "the commits are shown");
        assert_eq!(panel.commits.len(), 1, "there is one commit");
        assert_eq!(panel.commits[0].message, "the first one");
        assert!(
            panel.diff.contains("the first line"),
            "what the commit changed is shown: {}",
            panel.diff
        );

        // The escape key goes back to the files, and the panel stays open.
        assert!(app.handle_git_key(&named(NamedKey::Escape), None));
        let panel = app.git.as_ref().expect("the panel is still open");
        assert_eq!(panel.view(), GitView::Files, "the files are shown again");
        assert_eq!(panel.entries.len(), 1, "the changed file is still there");
    }

    /// A repository with a file in it and a remote of its own.
    ///
    /// The remote is a bare repository on this machine: the checks push to it
    /// and fetch from it without asking anything of the network.
    fn git_project_with_a_remote(name: &str) -> Option<(PathBuf, PathBuf)> {
        let (directory, file) = git_project(name)?;
        let remote = scratch_dir(&format!("{name}-remote"));
        git::run(&remote, &["init", "--quiet", "--bare"]).expect("the remote is made");
        git::run(
            &directory,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        )
        .expect("the remote is added");
        Some((directory, file))
    }

    #[test]
    fn the_git_branch_view_shows_the_branches_and_the_remotes() {
        let Some((_, file)) = git_project_with_a_remote("git-branch-view") else {
            return;
        };

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        assert!(app.handle_git_key(&character("b"), None));

        let panel = app.git.as_ref().expect("the panel is still open");
        assert_eq!(panel.view(), GitView::Branches, "the branches are shown");
        assert!(
            panel
                .branch_items
                .iter()
                .any(|item| matches!(item, git::BranchItem::Remote(_))),
            "the remote is in the list: {:?}",
            panel.branch_items
        );
        assert!(!panel.current_branch.is_empty(), "the branch is known");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("分支"), "got {status}");
    }

    #[test]
    fn the_git_branch_view_goes_back_to_the_changed_files() {
        let Some((_, file)) = git_project("git-branch-back") else {
            return;
        };

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("b"), None);
        assert!(app.handle_git_key(&character("b"), None));

        let panel = app.git.as_ref().expect("the panel is still open");
        assert_eq!(panel.view(), GitView::Files, "the files are shown again");
    }

    #[test]
    fn the_git_state_line_says_which_branch_the_program_is_on() {
        let Some((_, file)) = git_project("git-branch-state") else {
            return;
        };

        let app = NativeApplication::new(Some(file)).expect("the application starts");
        let branch = app.current_branch.clone().expect("the branch is known");
        assert!(!branch.is_empty(), "the branch has a name");
    }

    #[test]
    fn a_git_branch_can_be_made_from_the_panel() {
        let Some((directory, file)) = git_project("git-new-branch") else {
            return;
        };

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("b"), None);
        app.handle_git_key(&character("n"), None);
        assert!(app.git_box.is_some(), "the box asks for the name");

        for letter in ["t", "h", "e"] {
            app.handle_git_box_key(&character(letter), None);
        }
        app.handle_git_box_key(&named(NamedKey::Enter), None);

        assert!(app.git_box.is_none(), "the box is closed");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("建了"), "got {status}");
        let said = git::run(&directory, &["branch", "--list", "the"]).expect("the branches are read");
        assert!(said.contains("the"), "got {said}");
    }

    #[test]
    fn a_git_branch_can_be_gone_to_from_the_panel() {
        let Some((directory, file)) = git_project("git-switch-branch") else {
            return;
        };
        // Another branch is made, and the first one is gone back to.
        let first = git::current_branch(&directory).expect("the branch is known");
        git::create_branch(&directory, "the-other").expect("the branch is made");
        git::switch_to(&directory, &first).expect("the branch is gone to");

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("b"), None);

        // The other branch is picked: the list is walked until it is there.
        for _ in 0..8 {
            let picked = app
                .git
                .as_ref()
                .expect("the panel is open")
                .selected_branch()
                .map(|branch| branch.name.clone());
            if picked.as_deref() == Some("the-other") {
                break;
            }
            app.handle_git_key(&named(NamedKey::ArrowDown), None);
        }
        app.handle_git_key(&named(NamedKey::Enter), None);

        let now = git::current_branch(&directory).expect("the branch is known");
        assert_eq!(now, "the-other", "the branch is gone to");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("切到"), "got {status}");
    }

    #[test]
    fn the_git_push_is_asked_for_first() {
        let Some((_, file)) = git_project_with_a_remote("git-push-ask") else {
            return;
        };

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("b"), None);
        app.handle_git_key(&character("p"), None);

        let asking = app.git_box.as_ref().expect("the box asks first");
        assert!(
            matches!(asking.asking, git::GitAsking::Push { .. }),
            "got {:?}",
            asking.asking
        );

        // The escape key leaves it alone: nothing is pushed.
        app.handle_git_box_key(&named(NamedKey::Escape), None);
        assert!(app.git_box.is_none(), "the box is closed");
    }

    #[test]
    fn the_git_push_says_so_when_there_is_no_remote() {
        let Some((_, file)) = git_project("git-push-no-remote") else {
            return;
        };

        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.open_git_panel();
        app.handle_git_key(&character("b"), None);
        app.handle_git_key(&character("p"), None);

        assert!(app.git_box.is_none(), "there is nothing to confirm");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("没有远端"), "got {status}");
    }

    #[test]
    fn the_command_box_asks_for_a_command_and_remembers_it() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        // Ctrl+Shift+U is the key of the command that asks for a command.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("u")));
        assert!(app.command_box.is_some(), "the box asks for a command");
        app.modifiers = ModifiersState::default();

        for letter in ["c", "a", "t"] {
            app.handle_command_key(&character(letter), None);
        }
        let typed = app
            .command_box
            .as_ref()
            .expect("the box is open")
            .text()
            .to_owned();
        assert_eq!(typed, "cat");

        // Enter runs it. There is no file, so the editor says that it does
        // not know where to run it -- and the command is kept anyway.
        app.handle_command_key(&named(NamedKey::Enter), None);
        assert!(app.command_box.is_none(), "the box is closed");
        assert_eq!(app.command_history, vec!["cat".to_owned()]);
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("不知道在哪儿跑"), "got {status}");

        // The next time the box opens, the command is there: the up key
        // walks the history.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("u")));
        app.modifiers = ModifiersState::default();
        app.handle_command_key(&named(NamedKey::ArrowUp), None);
        let typed = app
            .command_box
            .as_ref()
            .expect("the box is open")
            .text()
            .to_owned();
        assert_eq!(typed, "cat");
    }

    #[test]
    fn a_command_that_is_not_written_is_not_run() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("u")));
        app.modifiers = ModifiersState::default();

        app.handle_command_key(&named(NamedKey::Enter), None);
        assert!(app.command_history.is_empty(), "nothing was run");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("不能是空的"), "got {status}");
    }

    #[test]
    fn the_command_box_takes_every_key_while_it_is_open() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("u")));
        app.modifiers = ModifiersState::default();

        // A key that is not the box's is swallowed by the box: the document
        // behind it must not see it.
        assert!(app.handle_command_key(&character("x"), None));
        assert_eq!(app.editor().text(), "", "the text is not touched");
        assert!(app.command_box.is_some(), "the box stays open");
    }

    #[test]
    fn the_close_tab_key_closes_the_tab_that_is_in_front() {
        let directory = scratch_dir("close-tab");
        let first = directory.join("one.txt");
        let second = directory.join("two.txt");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();

        let mut app = NativeApplication::new(Some(first)).expect("the application starts");
        app.open_file(&second);
        assert_eq!(app.tabs.len(), 2, "two tabs are open");

        // Ctrl+W is the key of the command that closes the tab.
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.run_the_command_of_the_key(&character("w")));
        assert_eq!(app.tabs.len(), 1, "the tab in front is closed");
    }

    #[test]
    fn the_palette_key_opens_the_panel_of_the_commands() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_palette_key(&character("p"), None));
        let palette = app.palette.as_ref().expect("the panel is open");
        assert!(!palette.is_empty(), "the editor has commands to show");

        // The keys go to the panel while it is open.
        app.modifiers = ModifiersState::default();
        assert!(app.handle_palette_key(&character("x"), Some("x")));
        let palette = app.palette.as_ref().expect("the panel is open");
        assert_eq!(palette.input().text(), "x");
    }

    #[test]
    fn a_command_that_is_picked_in_the_panel_runs() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_palette_key(&character("p"), None));
        app.modifiers = ModifiersState::default();

        // Write the name of the command that opens the settings.
        for letter in ["s", "e", "t", "t"] {
            app.handle_palette_key(&character(letter), Some(letter));
        }
        let palette = app.palette.as_ref().expect("the panel is open");
        assert_eq!(palette.rows().len(), 1, "only one command is left");

        // Enter runs it: the settings panel opens.
        app.handle_palette_key(&named(NamedKey::Enter), None);
        assert!(app.palette.is_none(), "the panel of the commands is closed");
        assert!(app.settings.is_some(), "the command ran");
    }

    #[test]
    fn the_escape_key_closes_the_panel_of_the_commands() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.handle_palette_key(&character("p"), None));
        app.modifiers = ModifiersState::default();

        assert!(app.handle_palette_key(&named(NamedKey::Escape), None));
        assert!(app.palette.is_none(), "the panel is closed");
    }

    #[test]
    fn a_command_that_needs_something_it_does_not_have_says_why() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        // No file is open: the commands that need one cannot be used.
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(!app.run_the_command_of_the_key(&character("b")));
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("没有打开的文件"), "got {status}");
    }

    /// An application in development mode, with its log in a folder of its
    /// own.
    fn app_in_development_mode(name: &str) -> (NativeApplication, PathBuf) {
        let directory = scratch_dir(name);
        let log = directory.join("editor.log");
        let app = NativeApplication::in_development_mode(None, log.clone())
            .expect("the application starts");
        (app, log)
    }

    #[test]
    fn the_development_mode_writes_what_the_editor_says() {
        let (mut app, log) = app_in_development_mode("dev-mode-log");
        app.status = Some("编辑器说了一句话".to_owned());
        app.log_the_status_if_it_changed();

        let text = fs::read_to_string(&log).expect("the log is read");
        assert!(text.contains("编辑器说了一句话"), "got {text}");
    }

    #[test]
    fn the_status_is_written_to_the_log_once_per_change() {
        let (mut app, log) = app_in_development_mode("dev-mode-once");
        app.status = Some("一句话".to_owned());
        app.log_the_status_if_it_changed();
        app.log_the_status_if_it_changed();
        app.log_the_status_if_it_changed();

        let text = fs::read_to_string(&log).expect("the log is read");
        assert_eq!(text.matches("一句话").count(), 1, "got {text}");
    }

    #[test]
    fn an_editor_that_is_not_in_development_mode_writes_no_log() {
        let app = NativeApplication::new(None).expect("the application starts");
        assert!(app.dev_log.is_none(), "there is no log without the mode");
        assert!(!app.dev, "the editor is not in development mode");
    }

    #[test]
    fn the_developer_key_opens_the_panel_that_shows_the_inside() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("d")));
        assert!(app.developer.is_some(), "the panel is open");
    }

    #[test]
    fn the_developer_panel_says_what_the_editor_is_made_of() {
        let app = NativeApplication::new(None).expect("the application starts");
        let lines = app.developer_lines();
        let text: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert!(
            text.iter().any(|line| line.contains("editor.format")),
            "the commands are listed: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("键位冲突")),
            "the conflicts are named: {text:?}"
        );
    }

    #[test]
    fn the_developer_panel_says_that_the_editor_is_not_in_development_mode() {
        let app = NativeApplication::new(None).expect("the application starts");
        let lines = app.developer_lines();
        let text: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert!(
            text.iter().any(|line| line.contains("没有以开发模式启动")),
            "the panel says why: {text:?}"
        );
    }

    #[test]
    fn the_escape_key_closes_the_developer_panel() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("d")));
        assert!(app.handle_developer_key(&named(NamedKey::Escape), 1.0));
        assert!(app.developer.is_none(), "the panel is closed");
    }

    #[test]
    fn a_panel_that_is_open_takes_every_key() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(app.run_the_command_of_the_key(&character("d")));
        // A key that is not the panel's is swallowed by the panel: the
        // document behind it must not see it.
        assert!(app.handle_developer_key(&character("x"), 1.0));
        assert!(app.developer.is_some(), "the panel stays open");
    }

    #[test]
    fn the_settings_panel_lists_the_keys_of_the_commands() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.open_settings();
        let settings = app.settings.as_ref().expect("the panel is open");
        let rows = settings.all_rows();
        assert!(rows.len() > 2, "the settings and the keys are there");
        assert!(
            rows.iter().any(|row| row.title.contains("格式化文档")),
            "a command is listed with its group"
        );
        assert!(
            rows.iter().any(|row| row.value.contains("Alt+Shift+F")),
            "the key of the command is there"
        );
        assert!(
            rows.iter().any(|row| row.value.contains("Ctrl+Shift+P")),
            "the key of the palette is there"
        );
    }

    #[test]
    fn a_key_is_given_to_a_command_in_the_settings_panel() {
        let mut app = NativeApplication::new(None).expect("the application starts");
        app.open_settings();
        let index = app
            .settings
            .as_ref()
            .expect("the panel is open")
            .all_rows()
            .iter()
            .position(|row| row.title.contains("格式化文档"))
            .expect("the command is listed");
        for _ in 0..index {
            app.handle_settings_key(&named(NamedKey::ArrowDown));
        }
        // Enter waits for the new key.
        app.handle_settings_key(&named(NamedKey::Enter));
        assert!(
            app.settings.as_ref().expect("the panel is open").recording(),
            "the panel waits for a key"
        );
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        assert!(app.handle_settings_key(&character("k")));
        app.modifiers = ModifiersState::default();

        let pressed = commands::PressedKey {
            letter: Some('k'),
            ctrl: true,
            alt: true,
            ..commands::PressedKey::default()
        };
        assert_eq!(
            app.commands.by_key(&pressed).map(|command| command.id.as_str()),
            Some("editor.format"),
            "the command has the new key"
        );
        let said = app.status.clone().expect("the editor says what it did");
        assert!(said.contains("Ctrl+Alt+K"), "got {said}");
        assert!(
            !app.settings.as_ref().expect("the panel is open").recording(),
            "the panel has its answer"
        );
    }

    #[test]
    fn the_keys_of_the_file_are_in_force_when_the_editor_starts() {
        let directory = scratch_dir("keybindings-start");
        let file = directory.join("notes.txt");
        fs::write(&file, "the first line\n").expect("the file is written");
        fs::write(
            directory.join("keybindings.json"),
            r#"[{ "key": "ctrl+alt+j", "command": "editor.find" }]"#,
        )
        .expect("the file of the keys is written");

        let app = NativeApplication::new(Some(file)).expect("the application starts");
        let pressed = commands::PressedKey {
            letter: Some('j'),
            ctrl: true,
            alt: true,
            ..commands::PressedKey::default()
        };
        assert_eq!(
            app.commands.by_key(&pressed).map(|command| command.id.as_str()),
            Some("editor.find"),
            "the file of the keys is in force"
        );
    }

    #[test]
    fn a_file_of_keys_that_names_a_command_that_is_not_there_is_said_out_loud() {
        let directory = scratch_dir("keybindings-wrong");
        let file = directory.join("notes.txt");
        fs::write(&file, "the first line\n").expect("the file is written");
        fs::write(
            directory.join("keybindings.json"),
            r#"[{ "key": "ctrl+alt+j", "command": "editor.noSuchCommand" }]"#,
        )
        .expect("the file of the keys is written");

        let app = NativeApplication::new(Some(file)).expect("the application starts");
        let said = app.status.clone().expect("the editor says what is wrong");
        assert!(said.contains("editor.noSuchCommand"), "got {said}");
    }

    #[test]
    fn the_window_can_be_split_into_two_columns() {
        let directory = scratch_dir("split");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        assert_eq!(app.groups.len(), 1, "the window starts with one column");

        assert!(app.run_named_command("view.splitRight"));
        assert_eq!(app.groups.len(), 2, "the window has two columns now");

        // The new column shows a file of its own: the columns do not show the
        // same file twice.
        let front = front_tab(&app.groups);
        assert_ne!(front, 0, "the new column shows a file of its own");
        assert!(
            app.panes[front].document.is_none(),
            "the file of the new column is a new one"
        );

        // The two columns do not lie on top of each other.
        let layout = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 2);
        let left = layout.column(0).expect("there is a column");
        let right = layout.column(1).expect("there is a column");
        assert!(left.body.max_x() <= right.body.origin.x);
        assert!(right.tab_bar.origin.x > left.tab_bar.origin.x);
    }

    #[test]
    fn clicking_in_a_column_gives_it_the_keyboard() {
        let directory = scratch_dir("split-click");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.run_named_command("view.splitRight");
        assert_eq!(app.groups.active(), 1, "the new column has the keyboard");

        // The click is in the first column: that column gets the keyboard.
        // The layout is worked out here and not by the application: a test
        // has no window, and the size of a window that is not there is
        // nothing at all.
        let layout = WindowLayout::of(Size::new(1280.0, 800.0), 0.0, true, 2);
        let left = layout.column(0).expect("there is a column");
        let clicked = Point {
            x: left.body.origin.x + 10.0,
            y: left.body.origin.y + 10.0,
        };
        let column = app.column_of_the_click(clicked, &layout);
        assert_eq!(column, Some(0), "the click is in the first column");
        assert_eq!(app.groups.active(), 0, "the column that was clicked has the keyboard");
        let status = app.status_bar_text();
        assert!(
            status.right.contains("栏 1/2"),
            "the status bar says which column it is, got {}",
            status.right
        );
    }

    #[test]
    fn the_place_of_a_column_is_kept_when_another_column_is_used() {
        let directory = scratch_dir("split-caret");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.run_named_command("view.splitRight");
        assert_eq!(app.groups.active(), 1, "the new column has the keyboard");

        // The caret of the first column is on the third line.
        let left = app.groups.front_of(0).expect("the first column has a tab");
        app.panes[left].editor.set_carets(&[TextOffset(8)]);
        let before = app.panes[left].editor.caret().0;
        assert_eq!(before, 8, "the caret is where it was put");

        // The other column takes the keyboard, and then the first one gets it
        // back: the place of the caret is where the file was left.
        app.groups.activate(1);
        assert_eq!(app.groups.active(), 1, "the second column has the keyboard");
        app.groups.activate(0);
        assert_eq!(
            app.panes[left].editor.caret().0,
            before,
            "the caret of the column is still where it was"
        );
    }

    #[test]
    fn every_column_shows_the_breakpoints_of_its_own_file() {
        let directory = scratch_dir("split-breakpoints");
        let first = directory.join("first.rs");
        let second = directory.join("second.rs");
        fs::write(&first, "one\ntwo\n").unwrap();
        fs::write(&second, "three\nfour\n").unwrap();
        let mut app = NativeApplication::new(Some(first.clone())).expect("the application starts");
        // A breakpoint on the first line of the first file.
        app.toggle_breakpoint();

        // The second file is opened in the second column, with a breakpoint of
        // its own on its second line.
        app.run_named_command("view.splitRight");
        app.open_file(&second);
        app.pane_mut().editor.set_carets(&[TextOffset(6)]);
        app.toggle_breakpoint();

        let columns = app.columns_of_the_window();
        assert_eq!(columns.len(), 2, "the window has two columns");
        let lines_of = |column: &ColumnData| {
            column
                .breakpoints
                .iter()
                .map(|mark| mark.line)
                .collect::<Vec<_>>()
        };
        assert_eq!(lines_of(&columns[0]), vec![1], "the first column has its own");
        assert_eq!(lines_of(&columns[1]), vec![2], "the second column has its own");
    }

    #[test]
    fn each_column_has_its_own_file() {
        let directory = scratch_dir("split-files");
        let first = directory.join("first.rs");
        let second = directory.join("second.rs");
        fs::write(&first, "one\n").unwrap();
        fs::write(&second, "two\n").unwrap();
        let mut app = NativeApplication::new(Some(first.clone())).expect("the application starts");
        app.run_named_command("view.splitRight");

        // The file is opened in the column that has the keyboard.
        app.open_file(&second);
        let right = app.groups.active();
        let tab = app.groups.front_of(right).expect("the column has a tab");
        assert_eq!(
            app.panes[tab].document.as_ref().map(|document| document.path()),
            Some(second.as_path()),
            "the file is in the column that has the keyboard"
        );

        // The other column still shows the file it had.
        let left = 1 - right;
        let tab = app.groups.front_of(left).expect("the column has a tab");
        assert_eq!(
            app.panes[tab].document.as_ref().map(|document| document.path()),
            Some(first.as_path()),
            "the other column still shows its own file"
        );
    }

    #[test]
    fn the_column_goes_away_when_its_last_tab_is_closed() {
        let directory = scratch_dir("split-close");
        let file = directory.join("main.rs");
        fs::write(&file, "one\ntwo\nthree").unwrap();
        let mut app = NativeApplication::new(Some(file)).expect("the application starts");
        app.run_named_command("view.splitRight");
        assert_eq!(app.groups.len(), 2);

        let tab = app
            .groups
            .front_of(1)
            .expect("the second column has a tab");
        let id = app.tabs.tabs()[tab].id;
        app.close_tab_by_id(id);
        assert_eq!(app.groups.len(), 1, "the column went away with its tab");
        assert_eq!(front_tab(&app.groups), 0, "the file that is left is on screen");
    }

    /// The three keys of the column selection.
    fn column_keys() -> ModifiersState {
        ModifiersState::CONTROL | ModifiersState::SHIFT | ModifiersState::ALT
    }

    #[test]
    fn the_column_selection_takes_the_same_column_on_two_lines() {
        let mut app = app_with_body("abcd\nefgh\n");
        // The caret is in the second column of the first line.
        app.pane_mut().editor.set_carets(&[TextOffset(1)]);
        app.modifiers = column_keys();
        assert!(app.run_the_command_of_the_key(&named(NamedKey::ArrowDown)));

        assert_eq!(app.editor().cursor_count(), 2, "the box covers two lines");
        let buffer = app.editor().buffer();
        let ranges: Vec<_> = app
            .editor()
            .selections()
            .iter()
            .map(|selection| selection.range(buffer))
            .collect();
        assert_eq!(ranges[0], TextOffset(1)..TextOffset(1));
        assert_eq!(ranges[1], TextOffset(6)..TextOffset(6));
    }

    #[test]
    fn the_column_selection_grows_with_every_press() {
        let mut app = app_with_body("abcd\nefgh\nijkl\n");
        app.pane_mut().editor.set_carets(&[TextOffset(1)]);
        app.modifiers = column_keys();
        assert!(app.run_the_command_of_the_key(&named(NamedKey::ArrowDown)));
        assert!(app.run_the_command_of_the_key(&named(NamedKey::ArrowDown)));
        assert_eq!(app.editor().cursor_count(), 3, "the box covers three lines");

        // The box gets wider: the same columns on every line.
        assert!(app.run_the_command_of_the_key(&named(NamedKey::ArrowRight)));
        let buffer = app.editor().buffer();
        let ranges: Vec<_> = app
            .editor()
            .selections()
            .iter()
            .map(|selection| selection.range(buffer))
            .collect();
        assert_eq!(ranges[0], TextOffset(1)..TextOffset(2));
        assert_eq!(ranges[1], TextOffset(6)..TextOffset(7));
        assert_eq!(ranges[2], TextOffset(11)..TextOffset(12));
    }

    #[test]
    fn the_column_selection_needs_the_three_keys() {
        let mut app = app_with_body("abcd\nefgh\n");
        app.modifiers = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert!(!app.run_the_command_of_the_key(&named(NamedKey::ArrowDown)));
        app.modifiers = ModifiersState::CONTROL | ModifiersState::ALT;
        assert!(!app.run_the_command_of_the_key(&named(NamedKey::ArrowDown)));
    }

    #[test]
    fn the_column_selection_is_over_when_the_keys_are_let_go() {
        let mut app = app_with_body("abcd\nefgh\n");
        app.modifiers = column_keys();
        assert!(app.run_the_command_of_the_key(&named(NamedKey::ArrowDown)));
        assert!(app.pane_mut().column.is_some(), "the box is open");

        // The keys are let go: what is left is a plain selection.
        app.modifiers = ModifiersState::default();
        app.end_the_column_selection();
        assert!(app.pane_mut().column.is_none(), "the box is over");
        assert_eq!(app.editor().cursor_count(), 2, "the selection is still there");
    }

    #[test]
    fn the_escape_key_cancels_the_column_selection() {
        let mut app = app_with_body("abcd\nefgh\n");
        app.modifiers = column_keys();
        assert!(app.run_the_command_of_the_key(&named(NamedKey::ArrowDown)));
        assert!(app.pane_mut().column.is_some(), "the box is open");

        app.modifiers = ModifiersState::default();
        app.cancel_the_selection();
        assert!(app.pane_mut().column.is_none(), "the box is over");
        assert_eq!(app.editor().cursor_count(), 1, "one caret is left");
    }

    #[test]
    fn the_word_cursor_takes_the_next_place_of_the_same_word() {
        let mut app = app_with_body("one two one");
        app.pane_mut().editor.set_carets(&[TextOffset(1)]);
        app.modifiers = ModifiersState::CONTROL;

        // The first press takes the word under the caret.
        assert!(app.run_the_command_of_the_key(&character("d")));
        assert_eq!(app.editor().cursor_count(), 1, "the word is selected");
        assert_eq!(
            app.editor().selection().range(app.editor().buffer()),
            TextOffset(0)..TextOffset(3)
        );

        // The second press adds the next place of that word.
        assert!(app.run_the_command_of_the_key(&character("d")));
        assert_eq!(app.editor().cursor_count(), 2, "the next place has a caret too");
        let buffer = app.editor().buffer();
        let places: Vec<_> = app
            .editor()
            .selections()
            .iter()
            .map(|selection| selection.range(buffer))
            .collect();
        assert_eq!(places[0], TextOffset(0)..TextOffset(3));
        assert_eq!(places[1], TextOffset(8)..TextOffset(11));
    }

    #[test]
    fn the_word_cursor_says_when_there_is_no_next_one() {
        let mut app = app_with_body("one two");
        app.pane_mut().editor.set_carets(&[TextOffset(1)]);
        app.modifiers = ModifiersState::CONTROL;
        assert!(app.run_the_command_of_the_key(&character("d")));
        assert!(app.run_the_command_of_the_key(&character("d")));
        assert_eq!(app.editor().cursor_count(), 1, "there is only one place");
        assert!(
            app.status
                .as_ref()
                .expect("the editor says something")
                .contains("没有下一个"),
            "the editor has to say that there is no next one"
        );
    }
}
