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

use codeweaver_dap::protocol::{StackFrame, Variable};
use codeweaver_dap::{AdapterConfig, Client, DapError};
use codeweaver_text::{Anchor, Buffer};

/// The height of the panel of the debugger.
pub const DEBUG_PANEL_HEIGHT: f32 = 220.0;
/// The height of one row of the panel.
const ROW_HEIGHT: f32 = 18.0;
/// The place of the title of the panel, from the top of it.
const TITLE_TOP: f32 = 8.0;
/// The place of the headings of the two columns, from the top of the panel.
const HEADING_TOP: f32 = 32.0;
/// The place of the first row of the two columns, from the top of the panel.
const ROW_TOP: f32 = 50.0;
/// The place of the column of the stack, from the left of the window.
const STACK_LEFT: f32 = 12.0;
/// The width of the column of the stack.
const STACK_WIDTH: f32 = 300.0;
/// The place of the column of the variables, from the left of the window.
const VARIABLE_LEFT: f32 = 328.0;
/// How many frames are shown at once.
const SHOWN_FRAMES: usize = 4;
/// How many variables are shown at once.
const SHOWN_VARIABLES: usize = 6;

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
///
/// Every breakpoint of an open file also has a *place*: an anchor in the
/// buffer of that file. The anchor follows the text, so a breakpoint that
/// was above a line that was added moves down with the text it is on. A
/// breakpoint that kept its number would be on the wrong line.
#[derive(Clone, Debug, Default)]
pub struct Breakpoints {
    files: BTreeMap<String, BTreeSet<u32>>,
    /// The place of every breakpoint, by the file and the line it is on.
    places: BTreeMap<String, BTreeMap<u32, Anchor>>,
    /// The condition of every breakpoint, by the file and the line it is on.
    ///
    /// A condition says what has to be true for the program to stop there.
    /// A breakpoint without one stops the program every time it comes to that
    /// line.
    conditions: BTreeMap<String, BTreeMap<u32, String>>,
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
            // A breakpoint that is gone takes its condition with it: the
            // condition belongs to the breakpoint.
            self.forget_the_condition(uri, line);
            false
        } else {
            lines.insert(line);
            true
        }
    }

    /// The condition of the breakpoint of a line, when it has one.
    pub fn condition_of(&self, uri: &str, line: u32) -> Option<&str> {
        self.conditions
            .get(uri)
            .and_then(|conditions| conditions.get(&line))
            .map(String::as_str)
    }

    /// Gives a condition to the breakpoint of a line, or takes it away.
    ///
    /// An empty condition is no condition: the breakpoint stops the program
    /// every time it comes to that line.
    pub fn set_condition(&mut self, uri: &str, line: u32, condition: Option<&str>) {
        let condition = condition.map(str::trim).filter(|text| !text.is_empty());
        match condition {
            Some(text) => {
                self.conditions
                    .entry(uri.to_owned())
                    .or_default()
                    .insert(line, text.to_owned());
            }
            None => self.forget_the_condition(uri, line),
        }
    }

    /// Takes the condition of the breakpoint of a line away.
    fn forget_the_condition(&mut self, uri: &str, line: u32) {
        let Some(conditions) = self.conditions.get_mut(uri) else {
            return;
        };
        conditions.remove(&line);
        if conditions.is_empty() {
            self.conditions.remove(uri);
        }
    }

    /// The conditions of the breakpoints of a file, by the line they are on.
    pub fn conditions_of(&self, uri: &str) -> BTreeMap<u32, String> {
        self.conditions.get(uri).cloned().unwrap_or_default()
    }

    /// The breakpoints of a file with their conditions.
    ///
    /// This is what the adapter is told: the line the program is to stop at,
    /// and what has to be true for it to stop there.
    pub fn with_conditions_of(&self, uri: &str) -> Vec<BreakpointRequest> {
        let conditions = self.conditions_of(uri);
        self.lines_of(uri)
            .into_iter()
            .map(|line| {
                let condition = conditions.get(&line).cloned();
                (line, condition)
            })
            .collect()
    }

    /// Every file that has breakpoints, with their conditions.
    pub fn all_with_conditions(&self) -> Vec<(String, Vec<BreakpointRequest>)> {
        self.all()
            .into_iter()
            .map(|(uri, lines)| {
                let conditions = self.conditions_of(&uri);
                let breakpoints = lines
                    .into_iter()
                    .map(|line| {
                        let condition = conditions.get(&line).cloned();
                        (line, condition)
                    })
                    .collect();
                (uri, breakpoints)
            })
            .collect()
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

    /// Remembers where the breakpoint of a line is.
    ///
    /// The place is an anchor in the buffer of the file: it follows the text
    /// when the file changes.
    pub fn remember(&mut self, uri: &str, line: u32, anchor: Anchor) {
        self.places
            .entry(uri.to_owned())
            .or_default()
            .insert(line, anchor);
    }

    /// Forgets the place of the breakpoint of a line.
    ///
    /// The anchor is given back so that the buffer of the file can let it
    /// go.
    pub fn forget(&mut self, uri: &str, line: u32) -> Option<Anchor> {
        let places = self.places.get_mut(uri)?;
        let anchor = places.remove(&line);
        if places.is_empty() {
            self.places.remove(uri);
        }
        anchor
    }

    /// Forgets the places of the breakpoints of a file.
    ///
    /// The anchors of a file that was closed died with its buffer; when the
    /// file is opened again, its breakpoints get new ones.
    pub fn forget_the_places(&mut self, uri: &str) {
        self.places.remove(uri);
    }

    /// Moves the breakpoints of a file to the lines the text is on now.
    ///
    /// Returns whether a breakpoint moved.
    pub fn follow(&mut self, uri: &str, buffer: &Buffer) -> bool {
        let Some(places) = self.places.remove(uri) else {
            // No breakpoint of this file follows the text.
            return false;
        };
        // The lines that are not tracked stay where they are: a breakpoint of
        // a file that is not open has no text to follow.
        let mut lines = self.files.get(uri).cloned().unwrap_or_default();
        let mut moved = false;
        let mut next = BTreeMap::new();
        // The condition belongs to the breakpoint: it goes where the
        // breakpoint goes.
        let mut next_conditions = self.conditions.get(uri).cloned().unwrap_or_default();
        for (line, anchor) in &places {
            // The anchor knows where the text is; the line is where that
            // text is.
            let now = match buffer.resolve_anchor(anchor) {
                Some(offset) => buffer.line_of_offset(offset.as_usize()) as u32 + 1,
                // The anchor is gone: the line the breakpoint was on is the
                // best that is left of it.
                None => *line,
            };
            if now != *line {
                moved = true;
                lines.remove(line);
                lines.insert(now);
                if let Some(condition) = next_conditions.remove(line) {
                    next_conditions.insert(now, condition);
                }
            }
            next.insert(now, *anchor);
        }
        self.files.insert(uri.to_owned(), lines);
        self.places.insert(uri.to_owned(), next);
        if next_conditions.is_empty() {
            self.conditions.remove(uri);
        } else {
            self.conditions.insert(uri.to_owned(), next_conditions);
        }
        moved
    }
}

/// A breakpoint as the adapter is told about it.
///
/// The line the program is to stop at, and what has to be true for it to
/// stop there. A breakpoint without a condition stops the program every time
/// it comes to that line.
pub type BreakpointRequest = (u32, Option<String>);

/// A breakpoint as the window draws it.
///
/// The window draws the breakpoints of the file that is on screen in its
/// margin. A breakpoint with a condition is drawn in another colour: it is
/// another kind of breakpoint, and the person has to see which one it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BreakpointMark {
    /// The line the breakpoint is on.
    pub line: u32,
    /// Whether the breakpoint has a condition.
    pub conditional: bool,
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
    /// The number of the frame the variables belong to.
    ///
    /// An expression is asked about in a frame: without its number the
    /// adapter does not know what the variables of the expression are.
    pub frame_id: Option<i64>,
    /// The frames of the stack, the top one first.
    ///
    /// The stack is read again every time the program stops: the program
    /// walks on, and the stack it is in is another one.
    pub frames: Vec<StackFrame>,
    /// Which frame of the stack is picked, counted from the top one.
    ///
    /// The variables that are shown are the ones of the frame that is
    /// picked, and an expression is asked about in it.
    pub selected_frame: usize,
    /// The expressions that were asked about and have no answer yet.
    ///
    /// The answer of the adapter does not carry the expression, so the
    /// editor keeps it: the answer has to be said with what was asked.
    pub evaluating: Vec<String>,
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
            frame_id: None,
            frames: Vec::new(),
            selected_frame: 0,
            evaluating: Vec::new(),
            program: program.to_owned(),
        })
    }

    /// Picks a frame of the stack.
    ///
    /// The variables of the frame are asked for again: the variables of one
    /// frame are not the variables of another, and the ones that are on
    /// screen belong to the frame that was picked before.
    ///
    /// Returns whether the frame was picked: a frame that is already the one
    /// that is shown is not picked again.
    pub fn select_frame(&mut self, index: usize) -> bool {
        let Some((id, name)) = pick_frame(&self.frames, index) else {
            return false;
        };
        if self.selected_frame == index {
            return false;
        }
        self.selected_frame = index;
        self.frame = Some(name);
        self.frame_id = Some(id);
        // The variables of the frame that was picked before are of no use
        // now: they would say something that is not true any more.
        self.variables.clear();
        if let Err(error) = self.client.scopes(id) {
            self.push_output(&format!("变量读不到：{error}"));
        }
        true
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

/// Whether the answer of the adapter is about the frame that is on the
/// screen.
///
/// The answer of a frame that was picked again while the answer was on its
/// way is of no use: the variables of a frame that is not on the screen
/// would be put on the screen.
pub fn the_answer_is_about_this_frame(answer: i64, shown: Option<i64>) -> bool {
    shown == Some(answer)
}

/// Picks a frame of the stack.
///
/// The frame that is picked is the one whose variables are shown, and the
/// one an expression is asked about in: the number of the frame and its name
/// are what is needed for both.
pub fn pick_frame(frames: &[StackFrame], index: usize) -> Option<(i64, String)> {
    let frame = frames.get(index)?;
    Some((frame.id, frame.name.clone()))
}

/// The frame under a point of the window, when there is one.
///
/// The frames of the stack are in the column on the left of the panel: a
/// click on one of them picks that frame.
///
/// The point is in the pixels of the window, and the size is the size of
/// the window.
pub fn frame_at(
    point: codeweaver_render::Point,
    size: codeweaver_render::Size,
    frames: usize,
) -> Option<usize> {
    if point.x < STACK_LEFT || point.x > STACK_LEFT + STACK_WIDTH {
        return None;
    }
    let top = size.height - DEBUG_PANEL_HEIGHT + ROW_TOP;
    if point.y < top {
        return None;
    }
    let row = ((point.y - top) / ROW_HEIGHT).floor();
    if row < 0.0 {
        return None;
    }
    let index = row as usize;
    // Only the frames that are on the screen can be picked: a frame that is
    // below the last one that is shown is not there to be picked.
    (index < frames.min(SHOWN_FRAMES)).then_some(index)
}

/// The panel of the debugger, as the window draws it.
pub struct DebugPanel<'a> {
    /// What the session is doing.
    pub state: DebugState,
    /// What the adapter has said.
    pub output: &'a [String],
    /// The variables of the frame that is picked.
    pub variables: &'a [Variable],
    /// The name of the frame the variables belong to.
    pub frame: Option<&'a str>,
    /// The frames of the stack, the top one first.
    pub frames: &'a [StackFrame],
    /// Which frame of the stack is picked, counted from the top one.
    pub selected_frame: usize,
    /// The program that is being debugged.
    pub program: &'a str,
}

/// Draws the panel of the debugger.
///
/// The panel is at the bottom of the window, under the text of the document.
///
/// It has two columns: the stack of the program on the left, and the
/// variables of the frame that is picked on the right. What the adapter said
/// is under both of them.
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
            rect.origin.y + TITLE_TOP,
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

    // How many rows the two columns take: what the adapter said goes under
    // the taller one of them.
    let mut rows = 0;

    if panel.state == DebugState::Stopped {
        // The stack of the program.
        scene.push_text(
            "调用栈",
            Rect::new(
                rect.origin.x + STACK_LEFT,
                rect.origin.y + HEADING_TOP,
                STACK_WIDTH,
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
        let shown = panel.frames.len().min(SHOWN_FRAMES);
        for (index, frame) in panel.frames.iter().take(shown).enumerate() {
            let y = rect.origin.y + ROW_TOP + index as f32 * ROW_HEIGHT;
            if index == panel.selected_frame {
                // The frame that is picked is marked: the person has to see
                // which frame the variables on the right belong to.
                scene.push_quad(
                    Rect::new(
                        rect.origin.x + STACK_LEFT,
                        y + 2.0,
                        3.0,
                        ROW_HEIGHT - 4.0,
                    ),
                    Color::rgb(0.30, 0.58, 0.96),
                    viewport,
                );
            }
            scene.push_text(
                frame.line(),
                Rect::new(
                    rect.origin.x + STACK_LEFT + 12.0,
                    y,
                    (STACK_WIDTH - 12.0).max(1.0),
                    20.0,
                ),
                TextStyle {
                    clip: viewport,
                    color: Color::rgb(0.74, 0.80, 0.70),
                    font_size: 13.0,
                    line_height: 18.0,
                    family: TextFamily::SansSerif,
                },
            );
        }
        rows = shown;
        if panel.frames.len() > SHOWN_FRAMES {
            // The stack is deeper than the room there is: the panel says so
            // instead of keeping quiet about it.
            let more = panel.frames.len() - SHOWN_FRAMES;
            scene.push_text(
                format!("…还有 {more} 帧"),
                Rect::new(
                    rect.origin.x + STACK_LEFT + 12.0,
                    rect.origin.y + ROW_TOP + shown as f32 * ROW_HEIGHT,
                    (STACK_WIDTH - 12.0).max(1.0),
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
            rows += 1;
        }

        // The variables of the frame that is picked.
        let heading = match panel.frame {
            Some(frame) => format!("变量 · {frame}"),
            None => "变量".to_owned(),
        };
        scene.push_text(
            heading,
            Rect::new(
                rect.origin.x + VARIABLE_LEFT,
                rect.origin.y + HEADING_TOP,
                (rect.size.width - VARIABLE_LEFT - 12.0).max(1.0),
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
        if panel.variables.is_empty() {
            scene.push_text(
                "没有变量",
                Rect::new(
                    rect.origin.x + VARIABLE_LEFT,
                    rect.origin.y + ROW_TOP,
                    (rect.size.width - VARIABLE_LEFT - 12.0).max(1.0),
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
            rows = rows.max(1);
        } else {
            let shown = panel.variables.len().min(SHOWN_VARIABLES);
            for (index, variable) in panel.variables.iter().take(shown).enumerate() {
                scene.push_text(
                    variable.line(),
                    Rect::new(
                        rect.origin.x + VARIABLE_LEFT,
                        rect.origin.y + ROW_TOP + index as f32 * ROW_HEIGHT,
                        (rect.size.width - VARIABLE_LEFT - 12.0).max(1.0),
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
            }
            rows = rows.max(shown);
            if panel.variables.len() > SHOWN_VARIABLES {
                let more = panel.variables.len() - SHOWN_VARIABLES;
                scene.push_text(
                    format!("…还有 {more} 个变量"),
                    Rect::new(
                        rect.origin.x + VARIABLE_LEFT,
                        rect.origin.y + ROW_TOP + shown as f32 * ROW_HEIGHT,
                        (rect.size.width - VARIABLE_LEFT - 12.0).max(1.0),
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
                rows = rows.max(shown + 1);
            }
        }
    }

    // What the adapter has said, the newest at the bottom.
    let y = rect.origin.y + ROW_TOP + rows as f32 * ROW_HEIGHT;
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
    use codeweaver_text::{Bias, TextEdit, TextOffset, Transaction};

    /// A buffer with three lines, and the place of the third one.
    fn buffer_of_three_lines() -> (Buffer, Anchor) {
        let mut buffer = Buffer::new("one\ntwo\nthree\n");
        let offset = buffer.line_range(2).start;
        let anchor = buffer.anchor(offset, Bias::Right);
        (buffer, anchor)
    }

    /// Puts a line at the top of the text.
    fn put_a_line_above(buffer: &mut Buffer, text: &str) {
        let version = buffer.version();
        buffer
            .apply(Transaction::new(
                version,
                vec![TextEdit::new(TextOffset(0)..TextOffset(0), text)],
            ))
            .expect("the edit fits");
    }

    #[test]
    fn a_breakpoint_follows_the_text_that_is_above_it() {
        let (mut buffer, anchor) = buffer_of_three_lines();
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 3);
        breakpoints.remember("file:///main.rs", 3, anchor);

        put_a_line_above(&mut buffer, "zero\n");

        assert!(
            breakpoints.follow("file:///main.rs", &buffer),
            "the breakpoint moved"
        );
        assert_eq!(breakpoints.lines_of("file:///main.rs"), vec![4]);
    }

    #[test]
    fn a_breakpoint_stays_when_the_text_below_it_changes() {
        let (mut buffer, anchor) = buffer_of_three_lines();
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 3);
        breakpoints.remember("file:///main.rs", 3, anchor);

        // A line is put at the end of the text.
        let version = buffer.version();
        let end = buffer.len();
        buffer
            .apply(Transaction::new(
                version,
                vec![TextEdit::new(end..end, "four\n")],
            ))
            .expect("the edit fits");

        assert!(
            !breakpoints.follow("file:///main.rs", &buffer),
            "the breakpoint did not move"
        );
        assert_eq!(breakpoints.lines_of("file:///main.rs"), vec![3]);
    }

    #[test]
    fn a_breakpoint_of_a_file_that_is_not_open_stays_where_it_is() {
        let (buffer, _anchor) = buffer_of_three_lines();
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 3);

        assert!(!breakpoints.follow("file:///main.rs", &buffer));
        assert_eq!(breakpoints.lines_of("file:///main.rs"), vec![3]);
    }

    #[test]
    fn the_place_of_a_breakpoint_is_forgotten_when_it_is_taken_away() {
        let (_buffer, anchor) = buffer_of_three_lines();
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 3);
        breakpoints.remember("file:///main.rs", 3, anchor);

        assert!(breakpoints.forget("file:///main.rs", 3).is_some());
        assert!(
            breakpoints.forget("file:///main.rs", 3).is_none(),
            "there is no place left"
        );
    }

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

    #[test]
    fn a_breakpoint_can_have_a_condition() {
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 7);
        breakpoints.set_condition("file:///main.rs", 7, Some("count > 3"));

        assert_eq!(
            breakpoints.condition_of("file:///main.rs", 7),
            Some("count > 3")
        );
        assert_eq!(breakpoints.conditions_of("file:///main.rs").len(), 1);
    }

    #[test]
    fn an_empty_condition_is_no_condition() {
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 7);
        breakpoints.set_condition("file:///main.rs", 7, Some("   "));

        assert_eq!(breakpoints.condition_of("file:///main.rs", 7), None);
        assert!(breakpoints.conditions_of("file:///main.rs").is_empty());
    }

    #[test]
    fn a_breakpoint_that_is_taken_away_takes_its_condition_with_it() {
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 7);
        breakpoints.set_condition("file:///main.rs", 7, Some("count > 3"));

        assert!(!breakpoints.toggle("file:///main.rs", 7));
        assert_eq!(
            breakpoints.condition_of("file:///main.rs", 7),
            None,
            "the condition belonged to the breakpoint"
        );
    }

    #[test]
    fn the_condition_follows_the_breakpoint_that_moves() {
        let (mut buffer, anchor) = buffer_of_three_lines();
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 3);
        breakpoints.remember("file:///main.rs", 3, anchor);
        breakpoints.set_condition("file:///main.rs", 3, Some("count > 3"));

        put_a_line_above(&mut buffer, "zero\n");

        assert!(breakpoints.follow("file:///main.rs", &buffer));
        assert_eq!(breakpoints.lines_of("file:///main.rs"), vec![4]);
        assert_eq!(
            breakpoints.condition_of("file:///main.rs", 4),
            Some("count > 3"),
            "the condition goes where the breakpoint goes"
        );
    }

    #[test]
    fn a_condition_can_be_taken_away() {
        let mut breakpoints = Breakpoints::default();
        breakpoints.toggle("file:///main.rs", 7);
        breakpoints.set_condition("file:///main.rs", 7, Some("count > 3"));
        breakpoints.set_condition("file:///main.rs", 7, None);

        assert_eq!(breakpoints.condition_of("file:///main.rs", 7), None);
        assert!(
            breakpoints.lines_of("file:///main.rs") == vec![7],
            "the breakpoint is still there"
        );
    }

    /// The stack of a program that stopped: three frames.
    fn a_stack_of_three_frames() -> Vec<StackFrame> {
        vec![
            StackFrame {
                id: 1,
                name: "main".to_owned(),
                line: Some(12),
                column: Some(1),
                source: Some(codeweaver_dap::protocol::Source {
                    path: Some("file:///work/main.rs".to_owned()),
                    name: None,
                }),
            },
            StackFrame {
                id: 2,
                name: "helper".to_owned(),
                line: Some(8),
                column: Some(1),
                source: Some(codeweaver_dap::protocol::Source {
                    path: Some("file:///work/main.rs".to_owned()),
                    name: None,
                }),
            },
            StackFrame {
                id: 3,
                name: "outer".to_owned(),
                line: Some(3),
                column: Some(1),
                source: Some(codeweaver_dap::protocol::Source {
                    path: Some("file:///work/main.rs".to_owned()),
                    name: None,
                }),
            },
        ]
    }

    /// The window the panel of the debugger is drawn in.
    fn a_window() -> codeweaver_render::Size {
        codeweaver_render::Size {
            width: 1200.0,
            height: 750.0,
        }
    }

    #[test]
    fn a_frame_is_picked_with_its_number_and_its_name() {
        let frames = a_stack_of_three_frames();

        let (id, name) = pick_frame(&frames, 1).expect("the frame is there");
        assert_eq!(id, 2);
        assert_eq!(name, "helper");
    }

    #[test]
    fn a_frame_that_is_not_there_is_not_picked() {
        let frames = a_stack_of_three_frames();
        assert!(pick_frame(&frames, 3).is_none());
        assert!(pick_frame(&frames, 99).is_none());
    }

    #[test]
    fn only_the_answer_of_the_frame_on_the_screen_is_used() {
        // The frame that is on the screen is the one that was picked.
        assert!(the_answer_is_about_this_frame(2, Some(2)));
        // The answer of another frame would say something that is not true.
        assert!(!the_answer_is_about_this_frame(1, Some(2)));
        // Without a frame on the screen no answer is of any use.
        assert!(!the_answer_is_about_this_frame(1, None));
    }

    #[test]
    fn the_pointer_on_a_frame_picks_that_frame() {
        let size = a_window();
        // The panel is at the bottom of the window, and the first frame is
        // on the first row of the column of the stack.
        let top = size.height - DEBUG_PANEL_HEIGHT + ROW_TOP;

        assert_eq!(frame_at(codeweaver_render::Point { x: 40.0, y: top + 2.0 }, size, 3), Some(0));
        assert_eq!(
            frame_at(codeweaver_render::Point { x: 40.0, y: top + ROW_HEIGHT + 2.0 }, size, 3),
            Some(1)
        );
        assert_eq!(
            frame_at(
                codeweaver_render::Point { x: 40.0, y: top + ROW_HEIGHT * 2.0 + 2.0 },
                size,
                3
            ),
            Some(2)
        );
    }

    #[test]
    fn a_pointer_that_is_not_on_a_frame_picks_nothing() {
        let size = a_window();
        let top = size.height - DEBUG_PANEL_HEIGHT + ROW_TOP;

        // The column of the variables is not the column of the stack.
        assert_eq!(
            frame_at(
                codeweaver_render::Point { x: VARIABLE_LEFT + 10.0, y: top + 2.0 },
                size,
                3
            ),
            None
        );
        // The text of the document is not the panel.
        assert_eq!(
            frame_at(codeweaver_render::Point { x: 40.0, y: 200.0 }, size, 3),
            None
        );
        // A frame that is not on the screen cannot be picked.
        assert_eq!(
            frame_at(
                codeweaver_render::Point { x: 40.0, y: top + ROW_HEIGHT * 4.0 },
                size,
                3
            ),
            None
        );
    }
}
