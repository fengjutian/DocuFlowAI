//! The panel that finds text in the files of the project.
//!
//! The search runs on a thread of its own: a project with many files takes
//! a while to be walked, and the window has to stay alive while it happens.
//! The answer comes in through a channel, and the window collects it once
//! per frame, the way the output of the task is collected.
//!
//! The panel covers the window, the way the panel of the changes does: it is
//! a thing the user works in for a moment, and the text of the document is
//! not what they are looking at while it is open.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{channel, Receiver},
};

use codeweaver_render::{Color, FrameScene, Rect, Size, TextFamily, TextStyle};
use codeweaver_ui::TextMeasurer;
use codeweaver_workspace::search::{search, SearchHit, SearchOptions, SearchResults};
use winit::keyboard::{Key, NamedKey};

use crate::rename::RenameBox;
use crate::key_letter;

/// The place of the box, from the left of the window.
const BOX_LEFT: f32 = 16.0;
/// The place of the box, from the top of the window.
const BOX_TOP: f32 = 16.0;
/// The width of the box.
const BOX_WIDTH: f32 = 640.0;
/// The height of the box.
const BOX_HEIGHT: f32 = 40.0;
/// The place of the list of the places, from the top of the window.
const LIST_TOP: f32 = 66.0;
/// The place of the box of the replacement, from the top of the window.
const REPLACE_TOP: f32 = BOX_TOP + BOX_HEIGHT + 8.0;
/// How much lower the list is when the panel replaces as well as finds.
const REPLACE_ROOM: f32 = 48.0;
/// The height of one row of the list of the places.
const ROW_HEIGHT: f32 = 22.0;
/// The width of the column that says where the place is.
const PLACE_WIDTH: f32 = 300.0;

/// The width of one button that turns a rule on and off.
const TOGGLE_WIDTH: f32 = 34.0;
/// The gap between the buttons.
const TOGGLE_GAP: f32 = 6.0;

/// The keys the panel takes.
pub const HINT: &str =
    "↑↓ 挑结果 · Enter 打开 · Tab 换地方 · Alt+C 大小写 · Alt+W 全字 · Esc 关掉";

/// The keys the panel takes, when it replaces as well as finds.
pub const REPLACE_HINT: &str =
    "↑↓ 挑结果 · Enter 打开 · Tab 换地方 · Alt+C 大小写 · Alt+W 全字 · 替换那一栏回车全换 · Esc 关掉";

/// Where the focus of the panel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchFocus {
    /// The text that is looked for is being written.
    Input,
    /// The list of the places is being picked from.
    Results,
    /// The text that replaces what was found is being written.
    Replace,
}

/// What the panel wants the window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchCommand {
    /// Nothing outside the panel has to happen.
    None,
    /// The panel is closed.
    Close,
    /// The file of the place that is picked is to be opened.
    Open(SearchHit),
    /// Everything that was found is to be replaced by something else.
    ReplaceAll {
        /// The text that was looked for.
        needle: String,
        /// The text that takes its place.
        replacement: String,
    },
}

/// The panel that finds text in the files of the project.
pub struct SearchPanel {
    /// The box the text is written in.
    box_: RenameBox,
    /// The box the replacement is written in.
    ///
    /// It is only shown when the panel replaces as well as finds.
    replace: RenameBox,
    /// Whether the panel replaces as well as finds.
    replacing: bool,
    /// Whether the case of the letters counts.
    ///
    /// It does not count by default: the person who looks for `needle` is
    /// looking for `Needle` too.
    case_sensitive: bool,
    /// Whether the text has to be a whole word.
    ///
    /// It does not have to be by default: `cat` is found in `catalog` too.
    whole_word: bool,
    /// The root of the project that is searched.
    root: PathBuf,
    /// The places the text was found.
    hits: Vec<SearchHit>,
    /// Which place is picked, counted from the first one.
    selected: usize,
    /// Where the focus is.
    focus: SearchFocus,
    /// What the panel says about the search.
    status: String,
    /// Whether a search is running.
    running: bool,
    /// Where the answer of the thread comes in.
    answers: Option<Receiver<SearchResults>>,
}

impl SearchPanel {
    /// Opens the panel over the project rooted at `root`.
    ///
    /// The text that was looked for last time is in the box already: the
    /// person who searches twice does not want to type it twice.
    pub fn new(root: PathBuf, needle: &str) -> Self {
        Self::open(root, needle, false)
    }

    /// Opens the panel over the project rooted at `root`, to replace as well
    /// as to find.
    ///
    /// The box of the replacement is there from the start: the person who
    /// wants to replace knows what they want to replace it with.
    pub fn with_replace(root: PathBuf, needle: &str) -> Self {
        Self::open(root, needle, true)
    }

    /// Opens the panel, with or without the box of the replacement.
    fn open(root: PathBuf, needle: &str, replacing: bool) -> Self {
        let mut box_ = RenameBox::message("查找");
        if !needle.is_empty() {
            box_.set_text(needle);
        }
        SearchPanel {
            box_,
            replace: RenameBox::message("替换"),
            replacing,
            case_sensitive: false,
            whole_word: false,
            root,
            hits: Vec::new(),
            selected: 0,
            focus: SearchFocus::Input,
            status: if replacing {
                "写要找的文字，回车开始找；Tab 换到替换那一栏".to_owned()
            } else {
                "写要找的文字，回车开始找".to_owned()
            },
            running: false,
            answers: None,
        }
    }

    /// The text that replaces what was found.
    pub fn replacement(&self) -> &str {
        self.replace.text()
    }

    /// The box the replacement is written in, for the window to draw.
    pub fn replace_field(&self) -> &RenameBox {
        &self.replace
    }

    /// Whether the panel replaces as well as finds.
    pub fn is_replacing(&self) -> bool {
        self.replacing
    }

    /// Whether the case of the letters counts.
    pub fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// Whether the text has to be a whole word.
    pub fn is_whole_word(&self) -> bool {
        self.whole_word
    }

    /// Turns the case sensitivity on or off, and looks for the text again.
    ///
    /// The search runs again: the places that were found with the other rule
    /// are not the places of this one.
    pub fn toggle_case_sensitive(&mut self) {
        self.case_sensitive = !self.case_sensitive;
        self.look_again();
    }

    /// Turns the whole word rule on or off, and looks for the text again.
    pub fn toggle_whole_word(&mut self) {
        self.whole_word = !self.whole_word;
        self.look_again();
    }

    /// Looks for the text again, when there is text to look for.
    ///
    /// The rule is remembered even when there is nothing to look for: the
    /// next search uses it.
    fn look_again(&mut self) {
        if self.box_.text().trim().is_empty() {
            return;
        }
        self.start_search();
    }

    /// What the rules of the search are, in words.
    ///
    /// The rules that are in force are said out loud: a search that finds
    /// less than the person expects is because of a rule they did not see.
    fn rules_of(&self) -> String {
        let mut rules = Vec::new();
        if self.case_sensitive {
            rules.push("区分大小写");
        }
        if self.whole_word {
            rules.push("全字");
        }
        if rules.is_empty() {
            return String::new();
        }
        format!("（{}）", rules.join(" · "))
    }

    /// The keys the panel takes, said at the bottom of it.
    pub fn hint(&self) -> &'static str {
        if self.replacing {
            REPLACE_HINT
        } else {
            HINT
        }
    }

    /// The text that is looked for.
    pub fn needle(&self) -> &str {
        self.box_.text()
    }

    /// The box the text is written in, for the window to draw.
    pub fn field(&self) -> &RenameBox {
        &self.box_
    }

    /// Adds text at the caret of the text that is being written.
    ///
    /// The text that an input method composes goes into the box that has the
    /// focus, not into the document behind it.
    pub fn type_text(&mut self, text: &str) {
        match self.focus {
            SearchFocus::Replace => self.replace.type_text(text),
            _ => self.box_.type_text(text),
        }
    }

    /// The box that has the focus.
    fn focused(&mut self) -> &mut RenameBox {
        match self.focus {
            SearchFocus::Replace => &mut self.replace,
            _ => &mut self.box_,
        }
    }

    /// The root of the project that is searched.
    ///
    /// The places are told in words relative to it: `src/main.rs:12` says
    /// more than a path that starts at the drive of the machine.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The places the text was found.
    pub fn hits(&self) -> &[SearchHit] {
        &self.hits
    }

    /// Which place is picked.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The place that is picked.
    pub fn selected_hit(&self) -> Option<&SearchHit> {
        self.hits.get(self.selected)
    }

    /// Where the focus is.
    pub fn focus(&self) -> SearchFocus {
        self.focus
    }

    /// What the panel says about the search.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Whether a search is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Where the place is, in words: the path inside the project, and the line.
    pub fn place_of(&self, hit: &SearchHit) -> String {
        let path = hit.path.strip_prefix(&self.root).unwrap_or(&hit.path);
        format!("{}:{}", path.display(), hit.line)
    }

    /// Picks the next place, going round to the first one.
    pub fn select_next(&mut self) {
        if self.hits.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.hits.len();
    }

    /// Picks the place before this one, going round to the last one.
    pub fn select_previous(&mut self) {
        if self.hits.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.hits.len() - 1
        } else {
            self.selected - 1
        };
    }

    /// Starts the search.
    ///
    /// The search runs on a thread of its own; the answer comes in later.
    /// Returns whether the search was started: an empty text is not searched
    /// for.
    pub fn start_search(&mut self) -> bool {
        let needle = self.box_.text().trim().to_owned();
        if needle.is_empty() {
            self.status = "要找的文字不能是空的".to_owned();
            return false;
        }
        let root = self.root.clone();
        let (sender, answers) = channel();
        self.status = format!("正在找 {needle}…");
        // The thread is left to finish on its own: the panel may be gone by
        // then, and the answer is dropped then.
        let options = SearchOptions {
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            ..SearchOptions::default()
        };
        std::thread::spawn(move || {
            let results = search(&root, &needle, options);
            let _ = sender.send(results);
        });
        self.answers = Some(answers);
        self.running = true;
        self.hits.clear();
        self.selected = 0;
        true
    }

    /// Collects the answer of the search, if it has come.
    ///
    /// Returns whether something changed, so that the window knows that it
    /// has to draw again.
    pub fn pump(&mut self) -> bool {
        if !self.running {
            return false;
        }
        let Some(answers) = self.answers.as_ref() else {
            return false;
        };
        let Ok(results) = answers.try_recv() else {
            return false;
        };
        self.running = false;
        self.hits = results.hits;
        self.selected = 0;
        let rules = self.rules_of();
        self.status = if results.truncated {
            format!("找到 {} 处（太多了，只看前面这些）{rules}", self.hits.len())
        } else if self.hits.is_empty() {
            format!("没有找到{rules}")
        } else {
            format!("在 {} 个文件里找到 {} 处{rules}", results.files, self.hits.len())
        };
        true
    }

    /// Gives a key to the panel.
    ///
    /// The panel takes every key while it is open: a word that is typed into
    /// the document behind it is not a word the person is looking for.
    pub fn handle_key<T: AsRef<str>>(
        &mut self,
        key: &Key<T>,
        text: Option<&str>,
        control: bool,
        alt: bool,
    ) -> SearchCommand {
        // The rules of the search have their own keys: Alt+C for the case
        // and Alt+W for the whole word.
        //
        // Nothing else is written into the box while Alt is held: a word
        // that is typed with Alt held is not a word the person is looking
        // for.
        if alt {
            if let Some(letter) = key_letter(key) {
                if letter.eq_ignore_ascii_case(&'c') {
                    self.toggle_case_sensitive();
                } else if letter.eq_ignore_ascii_case(&'w') {
                    self.toggle_whole_word();
                }
            }
            return SearchCommand::None;
        }
        match key {
            Key::Named(NamedKey::Escape) => return SearchCommand::Close,
            Key::Named(NamedKey::Tab) => {
                self.focus = match self.focus {
                    SearchFocus::Input => SearchFocus::Results,
                    SearchFocus::Results => {
                        if self.replacing {
                            SearchFocus::Replace
                        } else {
                            SearchFocus::Input
                        }
                    }
                    SearchFocus::Replace => SearchFocus::Input,
                };
                return SearchCommand::None;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.select_next();
                return SearchCommand::None;
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.select_previous();
                return SearchCommand::None;
            }
            Key::Named(NamedKey::Enter) => {
                return match self.focus {
                    SearchFocus::Input => {
                        if self.start_search() {
                            // The places that were found are what the person
                            // looks at now.
                            self.focus = SearchFocus::Results;
                        }
                        SearchCommand::None
                    }
                    SearchFocus::Results => {
                        if self.is_running() {
                            // The places that are there are the ones of the
                            // search that is still running: opening one of
                            // them now would open the wrong place.
                            self.status = "还在找".to_owned();
                            return SearchCommand::None;
                        }
                        match self.selected_hit().cloned() {
                            Some(hit) => SearchCommand::Open(hit),
                            None => SearchCommand::None,
                        }
                    }
                    SearchFocus::Replace => {
                        let needle = self.box_.text().trim().to_owned();
                        if needle.is_empty() {
                            self.status = "要找的文字不能是空的".to_owned();
                            return SearchCommand::None;
                        }
                        if self.is_running() {
                            self.status = "还在找".to_owned();
                            return SearchCommand::None;
                        }
                        if self.hits.is_empty() {
                            self.status = "没有找到可以替换的地方".to_owned();
                            return SearchCommand::None;
                        }
                        let replacement = self.replace.text().to_owned();
                        SearchCommand::ReplaceAll {
                            needle,
                            replacement,
                        }
                    }
                };
            }
            Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                // A key that changes the text belongs to the text.
                if self.focus != SearchFocus::Replace {
                    self.focus = SearchFocus::Input;
                }
                self.focused().handle_key(key, text, control);
                return SearchCommand::None;
            }
            Key::Character(character) if !control => {
                let character = character.as_ref();
                if character.chars().any(char::is_control) {
                    // The key did not produce text of its own; the text of
                    // the event is what the user typed.
                    if let Some(typed) = text.filter(|typed| !typed.chars().any(char::is_control)) {
                        self.focus = SearchFocus::Input;
                        self.box_.type_text(typed);
                    }
                } else {
                    if self.focus != SearchFocus::Replace {
                        self.focus = SearchFocus::Input;
                    }
                    self.focused().type_text(character);
                }
                return SearchCommand::None;
            }
            _ => {}
        }
        self.focused().handle_key(key, text, control);
        SearchCommand::None
    }
}

/// The ground the panel of the search stands on.
pub fn draw_background(scene: &mut FrameScene, viewport: Rect) {
    scene.push_quad(viewport, Color::rgb(0.055, 0.063, 0.082), viewport);
}

/// What is written in one of the boxes of the panel.
struct BoxText<'a> {
    /// The name of the box.
    label: &'a str,
    /// The text that is written in it.
    text: &'a str,
    /// Whether the box has the focus.
    focused: bool,
}

/// Draws one of the boxes of the panel.
///
/// The box has a name of its own, the text that is written in it, and a
/// caret when it has the focus: the person has to see where what they type
/// goes.
fn draw_box(
    scene: &mut FrameScene,
    viewport: Rect,
    top: f32,
    width: f32,
    box_text: BoxText<'_>,
    measurer: &mut dyn TextMeasurer,
) {
    let label = box_text.label;
    let text = box_text.text;
    let box_ = Rect::new(BOX_LEFT, top, width, BOX_HEIGHT);
    scene.push_quad(box_, Color::rgb(0.10, 0.12, 0.16), viewport);
    scene.push_quad(
        Rect::new(box_.origin.x, box_.origin.y, box_.size.width, 1.0),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );

    // The name of the box.
    let label_width = measurer.measure(label, 13.0).width;
    scene.push_text(
        label.to_owned(),
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

    // The text that is written in the box.
    let field = Rect::new(
        box_.origin.x + 16.0 + label_width,
        box_.origin.y + 8.0,
        (box_.size.width - label_width - 32.0).max(1.0),
        24.0,
    );
    let empty = text.is_empty();
    let shown = if empty { "（还没写）" } else { text };
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
    if box_text.focused {
        // The caret stands after the text that was typed.
        let width = measurer.measure(text, 14.0).width;
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

/// Draws one of the buttons that turn a rule of the search on and off.
///
/// The button that is on is drawn in the colour of the caret: the person
/// has to see which rules are in force.
fn draw_toggle(
    scene: &mut FrameScene,
    viewport: Rect,
    place: Rect,
    label: &str,
    on: bool,
    measurer: &mut dyn TextMeasurer,
) {
    scene.push_quad(place, Color::rgb(0.10, 0.12, 0.16), viewport);
    let border = if on {
        Color::rgb(0.30, 0.58, 0.96)
    } else {
        Color::rgb(0.22, 0.25, 0.31)
    };
    scene.push_quad(
        Rect::new(place.origin.x, place.origin.y, place.size.width, 2.0),
        border,
        viewport,
    );
    scene.push_quad(
        Rect::new(
            place.origin.x,
            place.origin.y + place.size.height - 2.0,
            place.size.width,
            2.0,
        ),
        border,
        viewport,
    );
    scene.push_quad(
        Rect::new(place.origin.x, place.origin.y, 2.0, place.size.height),
        border,
        viewport,
    );
    scene.push_quad(
        Rect::new(
            place.origin.x + place.size.width - 2.0,
            place.origin.y,
            2.0,
            place.size.height,
        ),
        border,
        viewport,
    );

    let width = measurer.measure(label, 13.0).width;
    scene.push_text(
        label.to_owned(),
        Rect::new(
            place.origin.x + (place.size.width - width) / 2.0,
            place.origin.y + 11.0,
            width + 2.0,
            20.0,
        ),
        TextStyle {
            clip: place,
            color: if on {
                Color::rgb(0.86, 0.89, 0.94)
            } else {
                Color::rgb(0.45, 0.50, 0.58)
            },
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );
}

/// Draws the panel of the search.
pub fn draw_panel(
    scene: &mut FrameScene,
    viewport: Rect,
    size: Size,
    panel: &SearchPanel,
    measurer: &mut dyn TextMeasurer,
) {
    draw_background(scene, viewport);

    // The box the text is written in.
    let width = BOX_WIDTH.min((size.width - BOX_LEFT * 2.0).max(1.0));
    draw_box(
        scene,
        viewport,
        BOX_TOP,
        width,
        BoxText {
            label: &panel.field().title(),
            text: panel.needle(),
            focused: panel.focus() == SearchFocus::Input,
        },
        measurer,
    );

    // The buttons that turn the rules of the search on and off.
    let toggles = BOX_LEFT + width + 8.0;
    draw_toggle(
        scene,
        viewport,
        Rect::new(toggles, BOX_TOP, TOGGLE_WIDTH, BOX_HEIGHT),
        "Aa",
        panel.is_case_sensitive(),
        measurer,
    );
    draw_toggle(
        scene,
        viewport,
        Rect::new(
            toggles + TOGGLE_WIDTH + TOGGLE_GAP,
            BOX_TOP,
            TOGGLE_WIDTH,
            BOX_HEIGHT,
        ),
        "Ab",
        panel.is_whole_word(),
        measurer,
    );

    // The box the replacement is written in, when the panel replaces as well
    // as finds.
    let room = if panel.is_replacing() { REPLACE_ROOM } else { 0.0 };
    if panel.is_replacing() {
        draw_box(
            scene,
            viewport,
            REPLACE_TOP,
            width,
            BoxText {
                label: &panel.replace_field().title(),
                text: panel.replacement(),
                focused: panel.focus() == SearchFocus::Replace,
            },
            measurer,
        );
    }

    // What the search says.
    scene.push_text(
        panel.status().to_owned(),
        Rect::new(
            BOX_LEFT,
            LIST_TOP + room,
            (size.width - BOX_LEFT * 2.0).max(1.0),
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

    // The places the text was found.
    let list = Rect::new(
        BOX_LEFT,
        LIST_TOP + room + 24.0,
        (size.width - BOX_LEFT * 2.0).max(1.0),
        (size.height - LIST_TOP - room - 60.0).max(1.0),
    );
    let rows = (list.size.height / ROW_HEIGHT).floor().max(1.0) as usize;
    let shown = panel.hits().len().min(rows);
    let first = if shown == 0 {
        0
    } else {
        panel
            .selected()
            .saturating_sub(shown - 1)
            .min(panel.hits().len() - 1)
    };
    for (index, hit) in panel.hits().iter().enumerate().skip(first).take(shown) {
        let row = index - first;
        let y = list.origin.y + row as f32 * ROW_HEIGHT;
        if index == panel.selected() {
            scene.push_quad(
                Rect::new(list.origin.x - 4.0, y, list.size.width, ROW_HEIGHT),
                Color::rgb(0.18, 0.30, 0.52),
                viewport,
            );
        }
        // Where the place is.
        scene.push_text(
            panel.place_of(hit),
            Rect::new(list.origin.x, y + 1.0, PLACE_WIDTH, 20.0),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.62, 0.70, 0.86),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
        // The text of the line the place is on.
        scene.push_text(
            hit.text.clone(),
            Rect::new(
                list.origin.x + PLACE_WIDTH + 4.0,
                y + 1.0,
                (list.size.width - PLACE_WIDTH - 8.0).max(1.0),
                20.0,
            ),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.86, 0.89, 0.94),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    }

    // The keys the panel takes are said at the bottom of it.
    scene.push_text(
        panel.hint().to_owned(),
        Rect::new(
            BOX_LEFT,
            size.height - 36.0,
            (size.width - BOX_LEFT * 2.0).max(1.0),
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

    let _ = measurer;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A directory of scratch files, empty and unique per check.
    fn scratch(name: &str) -> PathBuf {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(name);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory
    }

    /// A key that is a letter.
    fn character(letter: &str) -> Key<&'_ str> {
        Key::Character(letter)
    }

    /// A key that is named.
    fn named(key: NamedKey) -> Key<&'static str> {
        Key::Named(key)
    }

    #[test]
    fn the_panel_starts_with_the_text_that_was_looked_for() {
        let panel = SearchPanel::new(PathBuf::from("C:\\project"), "needle");
        assert_eq!(panel.needle(), "needle");
        assert_eq!(panel.focus(), SearchFocus::Input);
        assert!(panel.hits().is_empty());
        assert!(!panel.is_running());
    }

    #[test]
    fn the_panel_starts_empty_when_there_was_no_text() {
        let panel = SearchPanel::new(PathBuf::from("C:\\project"), "");
        assert_eq!(panel.needle(), "");
        assert_eq!(panel.field().title(), "查找");
    }

    #[test]
    fn the_panel_that_replaces_has_a_box_for_the_replacement() {
        let panel = SearchPanel::with_replace(PathBuf::from("C:\\project"), "needle");
        assert!(panel.is_replacing());
        assert_eq!(panel.needle(), "needle");
        assert_eq!(panel.replacement(), "");
        assert_eq!(panel.replace_field().title(), "替换");
    }

    #[test]
    fn the_tab_key_moves_to_the_replacement_when_the_panel_replaces() {
        let mut panel = SearchPanel::with_replace(PathBuf::from("C:\\project"), "");
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        assert_eq!(panel.focus(), SearchFocus::Results);
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        assert_eq!(panel.focus(), SearchFocus::Replace);
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        assert_eq!(panel.focus(), SearchFocus::Input);
    }

    #[test]
    fn what_is_typed_in_the_replacement_stays_in_the_replacement() {
        let mut panel = SearchPanel::with_replace(PathBuf::from("C:\\project"), "needle");
        // The tab key twice: the text, the places, the replacement.
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        assert_eq!(panel.focus(), SearchFocus::Replace);

        panel.handle_key(&character("p"), Some("p"), false, false);
        assert_eq!(panel.replacement(), "p");
        assert_eq!(panel.needle(), "needle", "the text that is looked for is not touched");
    }

    #[test]
    fn there_is_nothing_to_replace_when_nothing_was_found() {
        let mut panel = SearchPanel::with_replace(PathBuf::from("C:\\project"), "needle");
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        panel.handle_key(&character("p"), Some("p"), false, false);

        assert_eq!(
            panel.handle_key(&named(NamedKey::Enter), None, false, false),
            SearchCommand::None
        );
        assert!(
            panel.status().contains("没有找到"),
            "the panel has to say why: {}",
            panel.status()
        );
    }

    #[test]
    fn typing_goes_into_the_text_that_is_looked_for() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "");
        assert_eq!(
            panel.handle_key(&character("n"), Some("n"), false, false),
            SearchCommand::None
        );
        assert_eq!(panel.needle(), "n");
        assert_eq!(panel.focus(), SearchFocus::Input);
    }

    #[test]
    fn the_escape_key_closes_the_panel() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "");
        assert_eq!(
            panel.handle_key(&named(NamedKey::Escape), None, false, false),
            SearchCommand::Close
        );
    }

    #[test]
    fn the_tab_key_moves_between_the_text_and_the_places() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "");
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        assert_eq!(panel.focus(), SearchFocus::Results);
        panel.handle_key(&named(NamedKey::Tab), None, false, false);
        assert_eq!(panel.focus(), SearchFocus::Input);
    }

    #[test]
    fn the_search_finds_the_text_and_says_so() {
        let root = scratch("search-panel-finds-the-text");
        fs::write(root.join("notes.txt"), "the first line\nthe needle is here\n").unwrap();

        let mut panel = SearchPanel::new(root.clone(), "needle");
        panel.start_search();
        assert!(panel.is_running());

        // The answer comes in on the thread, so it is waited for.
        for _ in 0..200 {
            if panel.pump() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(!panel.is_running());
        assert_eq!(panel.hits().len(), 1);
        assert_eq!(panel.hits()[0].line, 2);
        assert!(
            panel.status().contains("找到 1 处"),
            "the panel says: {}",
            panel.status()
        );
        assert_eq!(panel.place_of(&panel.hits()[0]), "notes.txt:2");
    }

    #[test]
    fn the_search_that_finds_nothing_says_so() {
        let root = scratch("search-panel-finds-nothing");
        fs::write(root.join("notes.txt"), "something else\n").unwrap();

        let mut panel = SearchPanel::new(root.clone(), "needle");
        panel.start_search();
        for _ in 0..200 {
            if panel.pump() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(panel.hits().is_empty());
        assert_eq!(panel.status(), "没有找到");
    }

    #[test]
    fn an_empty_text_is_not_searched_for() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "");
        panel.start_search();
        assert!(!panel.is_running());
        assert_eq!(panel.status(), "要找的文字不能是空的");
    }

    /// Waits for the search of the panel to finish.
    fn wait_for_the_search(panel: &mut SearchPanel) {
        for _ in 0..200 {
            if panel.pump() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the search did not finish in time");
    }

    #[test]
    fn the_case_of_the_letters_can_be_asked_for() {
        let root = scratch("search-panel-counts-the-case");
        fs::write(root.join("a.txt"), "The Needle\nthe needle\n").unwrap();

        let mut panel = SearchPanel::new(root.clone(), "needle");
        panel.start_search();
        wait_for_the_search(&mut panel);
        assert_eq!(panel.hits().len(), 2, "the case does not count by default");

        panel.toggle_case_sensitive();
        wait_for_the_search(&mut panel);
        assert_eq!(panel.hits().len(), 1, "only the exact case is found");
        assert_eq!(panel.hits()[0].text, "the needle");
    }

    #[test]
    fn the_text_can_be_asked_to_be_a_whole_word() {
        let root = scratch("search-panel-finds-the-whole-word");
        fs::write(root.join("a.txt"), "the cat sat\nthe catalog is here\n").unwrap();

        let mut panel = SearchPanel::new(root.clone(), "cat");
        panel.start_search();
        wait_for_the_search(&mut panel);
        assert_eq!(panel.hits().len(), 2);

        panel.toggle_whole_word();
        wait_for_the_search(&mut panel);
        assert_eq!(panel.hits().len(), 1, "only the whole word is found");
        assert_eq!(panel.hits()[0].text, "the cat sat");
    }

    #[test]
    fn the_alt_c_key_turns_the_case_rule_on() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "needle");
        assert!(!panel.is_case_sensitive());

        panel.handle_key(&character("c"), None, false, true);

        assert!(panel.is_case_sensitive());
    }

    #[test]
    fn the_alt_w_key_turns_the_whole_word_rule_on() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "needle");
        assert!(!panel.is_whole_word());

        panel.handle_key(&character("w"), None, false, true);

        assert!(panel.is_whole_word());
    }

    #[test]
    fn the_rules_that_are_in_force_are_said_out_loud() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "needle");
        panel.toggle_case_sensitive();
        wait_for_the_search(&mut panel);
        panel.toggle_whole_word();
        wait_for_the_search(&mut panel);

        let rules = panel.status();
        assert!(rules.contains("区分大小写"), "the status says: {rules}");
        assert!(rules.contains("全字"), "the status says: {rules}");
    }

    #[test]
    fn the_places_are_picked_one_after_the_other() {
        let root = scratch("search-panel-picks-the-places");
        fs::write(root.join("a.txt"), "needle\nneedle\n").unwrap();

        let mut panel = SearchPanel::new(root.clone(), "needle");
        panel.start_search();
        for _ in 0..200 {
            if panel.pump() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(panel.hits().len(), 2);

        assert_eq!(panel.selected(), 0);
        panel.select_next();
        assert_eq!(panel.selected(), 1);
        // Round to the first one.
        panel.select_next();
        assert_eq!(panel.selected(), 0);
        // And round to the last one.
        panel.select_previous();
        assert_eq!(panel.selected(), 1);
    }

    #[test]
    fn the_enter_key_opens_the_place_that_is_picked() {
        let root = scratch("search-panel-opens-the-place");
        fs::write(root.join("a.txt"), "the needle is here\n").unwrap();

        let mut panel = SearchPanel::new(root.clone(), "needle");
        panel.start_search();
        for _ in 0..200 {
            if panel.pump() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // The focus is on the text first: the enter key starts the search.
        panel.focus = SearchFocus::Results;
        match panel.handle_key(&named(NamedKey::Enter), None, false, false) {
            SearchCommand::Open(hit) => {
                assert_eq!(hit.path, root.join("a.txt"));
                assert_eq!(hit.line, 1);
            }
            other => panic!("the enter key did not open the place: {other:?}"),
        }
    }
}
