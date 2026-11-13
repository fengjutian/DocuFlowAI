//! The debugger: the breakpoints, the session and the panel.
//!
//! The editor talks to a debug adapter over the Debug Adapter Protocol. The
//! adapter is a program of its own: it starts the program that is being
//! debugged, stops it at the breakpoints, and says what the variables of a
//! frame are.
//!
//! The editor never waits for the adapter: the events are read once per
//! frame, the way the output of the agent and of a task is read.

use std::collections::{BTreeMap, BTreeSet};

use codeweaver_dap::protocol::Variable;
use codeweaver_dap::{AdapterConfig, Client, DapError};

/// The height of the panel of the debugger.
pub const DEBUG_PANEL_HEIGHT: f32 = 220.0;

/// What the debugger is doing now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugState {
    /// There is no session.
    #[default]
    Idle,
    /// The adapter is starting and the program has not begun yet.
    Starting,
    /// The program is running.
    Running,
    /// The program is stopped and waits for a step.
    Stopped,
    /// The session is over.
    Finished,
}

impl DebugState {
    /// What the state is, in words.
    pub fn text(&self) -> &'static str {
        match self {
            DebugState::Idle => "没有调试",
            DebugState::Starting => "正在起调试会话",
            DebugState::Running => "程序在跑",
            DebugState::Stopped => "停住了",
            DebugState::Finished => "调试结束了",
        }
    }
}

/// Which way the program walks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugStep {
    /// One line, without going into what it calls.
    Over,
    /// One line, into what it calls.
    In,
    /// Until the frame it is in is over.
    Out,
}

/// The breakpoints of the editor.
///
/// The breakpoints belong to the files and not to the session: they are
/// there whether a session is running or not.
#[derive(Clone, Debug, Default)]
pub struct Breakpoints {
    files: BTreeMap<String, BTreeSet<u32>>,
}

impl Breakpoints {
    /// Puts a breakpoint on a line, or takes the one that is there away.
    ///
    /// Whether there is a breakpoint on the line now is said back: the
    /// window draws what the editor says.
    pub fn toggle(&mut self, uri: &str, line: u32) -> bool {
        let lines = self.files.entry(uri.to_owned()).or_default();
        if lines.remove(&line) {
            if lines.is_empty() {
                self.files.remove(uri);
            }
            false
        } else {
            lines.insert(line);
            true
        }
    }

    /// The lines of the breakpoints of a file.
    pub fn lines_of(&self, uri: &str) -> Vec<u32> {
        self.files
            .get(uri)
            .map(|lines| lines.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Every file that has breakpoints, and the lines of them.
    pub fn all(&self) -> Vec<(String, Vec<u32>)> {
        self.files
            .iter()
            .map(|(uri, lines)| (uri.clone(), lines.iter().copied().collect()))
            .collect()
    }

    /// How many breakpoints there are.
    pub fn len(&self) -> usize {
        self.files.values().map(|lines| lines.len()).sum()
    }

    /// Whether there are no breakpoints at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The debug session.
pub struct Debugger {
    /// The connection to the adapter.
    pub client: Client,
    /// What the session is doing.
    pub state: DebugState,
    /// The thread the program is on.
    pub thread_id: i64,
    /// What the adapter has said, the oldest first.
    pub output: Vec<String>,
    /// The variables of the frame the program is stopped in.
    pub variables: Vec<Variable>,
    /// The name of the frame the variables belong to.
    pub frame: Option<String>,
    /// The program that is being debugged.
    pub program: String,
}

impl Debugger {
    /// Starts the adapter and says hello to it.
    ///
    /// The adapter says that it is ready when it is; the breakpoints go
    /// after that, and then the program starts.
    pub fn start(config: &AdapterConfig, program: &str) -> Result<Self, DapError> {
        let client = Client::start(config)?;
        client.initialize()?;
        Ok(Debugger {
            client,
            state: DebugState::Starting,
            thread_id: 0,
            output: Vec::new(),
            variables: Vec::new(),
            frame: None,
            program: program.to_owned(),
        })
    }

    /// Says what the adapter has said.
    ///
    /// The text of the adapter may hold many lines, and the panel shows one
    /// line at a time.
    pub fn push_output(&mut self, text: &str) {
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            self.output.push(line.to_owned());
        }
        // The panel shows the last lines only: the output of a long session
        // must not grow without end.
        let limit = 500;
        if self.output.len() > limit {
            self.output.drain(..self.output.len() - limit);
        }
    }
}

/// The panel of the debugger, as the window draws it.
pub struct DebugPanel<'a> {
    /// What the session is doing.
    pub state: DebugState,
    /// What the adapter has said.
    pub output: &'a [String],
    /// The variables of the frame the program is stopped in.
    pub variables: &'a [Variable],
    /// The name of the frame the variables belong to.
    pub frame: Option<&'a str>,
    /// The program that is being debugged.
    pub program: &'a str,
}

/// The height of one row of the panel.
const ROW_HEIGHT: f32 = 18.0;

/// Draws the panel of the debugger.
///
/// The panel is at the bottom of the window, under the text of the document.
pub fn draw_panel(
    scene: &mut codeweaver_render::FrameScene,
    viewport: codeweaver_render::Rect,
    size: codeweaver_render::Size,
    panel: &DebugPanel<'_>,
) {
    use codeweaver_render::{Color, Rect, TextFamily, TextStyle};

    let rect = Rect::new(
        0.0,
        size.height - DEBUG_PANEL_HEIGHT,
        size.width,
        DEBUG_PANEL_HEIGHT,
    );
    scene.push_quad(rect, Color::rgb(0.055, 0.063, 0.082), viewport);
    scene.push_quad(
        Rect::new(rect.origin.x, rect.origin.y, rect.size.width, 1.0),
        Color::rgb(0.16, 0.18, 0.22),
        viewport,
    );

    // What the session is doing.
    let title = format!("调试 · {} · {}", panel.program, panel.state.text());
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
            color: Color::rgb(0.72, 0.78, 0.88),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );

    // The variables of the frame the program is stopped in.
    let mut y = rect.origin.y + 32.0;
    if panel.state == DebugState::Stopped {
        let heading = match panel.frame {
            Some(frame) => format!("变量 · {frame}"),
            None => "变量".to_owned(),
        };
        scene.push_text(
            heading,
            Rect::new(
                rect.origin.x + 12.0,
                y,
                (rect.size.width - 24.0).max(1.0),
                20.0,
            ),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.60, 0.65, 0.74),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
        y += ROW_HEIGHT;
        if panel.variables.is_empty() {
            scene.push_text(
                "没有变量",
                Rect::new(
                    rect.origin.x + 12.0,
                    y,
                    (rect.size.width - 24.0).max(1.0),
                    20.0,
                ),
                TextStyle {
                    clip: viewport,
                    color: Color::rgb(0.52, 0.57, 0.66),
                    font_size: 13.0,
                    line_height: 18.0,
                    family: TextFamily::SansSerif,
                },
            );
            y += ROW_HEIGHT;
        } else {
            for variable in panel.variables {
                scene.push_text(
                    variable.line(),
                    Rect::new(
                        rect.origin.x + 12.0,
                        y,
                        (rect.size.width - 24.0).max(1.0),
                        20.0,
                    ),
                    TextStyle {
                        clip: viewport,
                        color: Color::rgb(0.82, 0.86, 0.92),
                        font_size: 13.0,
                        line_height: 18.0,
                        family: TextFamily::SansSerif,
                    },
                );
                y += ROW_HEIGHT;
            }
        }
    }

    // What the adapter has said, the newest at the bottom.
    let room = ((rect.origin.y + rect.size.height - 8.0 - y) / ROW_HEIGHT).floor().max(0.0) as usize;
    let shown = panel.output.len().min(room);
    let first = panel.output.len() - shown;
    for (index, line) in panel.output.iter().skip(first).enumerate() {
        scene.push_text(
            line.clone(),
            Rect::new(
                rect.origin.x + 12.0,
                y + index as f32 * ROW_HEIGHT,
                (rect.size.width - 24.0).max(1.0),
                20.0,
            ),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.62, 0.67, 0.76),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_breakpoint_is_put_on_a_line_and_taken_away_again() {
        let mut breakpoints = Breakpoints::default();
        assert!(breakpoints.toggle("file:///main.rs", 7), "there is one now");
        assert_eq!(breakpoints.lines_of("file:///main.rs"), vec![7]);
        assert_eq!(breakpoints.len(), 1);

        assert!(!breakpoints.toggle("file:///main.rs", 7), "there is none now");
        assert!(breakpoints.lines_of("file:///main.rs").is_empty());
        assert!(breakpoints.is_empty());
    }

    #[test]
    fn the_breakpoints_of_two_files_are_kept_apart() {
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 7);
        breakpoints.toggle("file:///other.rs", 3);

        assert_eq!(breakpoints.lines_of("file:///main.rs"), vec![7]);
        assert_eq!(breakpoints.lines_of("file:///other.rs"), vec![3]);
        assert_eq!(breakpoints.len(), 2);

        let all = breakpoints.all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, "file:///main.rs");
        assert_eq!(all[0].1, vec![7]);
    }

    #[test]
    fn a_file_without_breakpoints_has_none() {
        let breakpoints = Breakpoints::default();
        assert!(breakpoints.lines_of("file:///main.rs").is_empty());
        assert!(breakpoints.all().is_empty());
    }

    #[test]
    fn the_state_says_what_it_is_in_words() {
        assert_eq!(DebugState::Stopped.text(), "停住了");
        assert_eq!(DebugState::Running.text(), "程序在跑");
        assert_eq!(DebugState::Finished.text(), "调试结束了");
    }
}
