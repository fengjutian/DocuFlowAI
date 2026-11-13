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
/// The height of one row of the list of the places.
const ROW_HEIGHT: f32 = 22.0;
/// The width of the column that says where the place is.
const PLACE_WIDTH: f32 = 300.0;

/// The keys the panel takes.
pub const HINT: &str = "↑↓ 挑结果 · Enter 打开 · Tab 换地方 · Esc 关掉";

/// Where the focus of the panel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchFocus {
    /// The text that is looked for is being written.
    Input,
    /// The list of the places is being picked from.
    Results,
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
}

/// The panel that finds text in the files of the project.
pub struct SearchPanel {
    /// The box the text is written in.
    box_: RenameBox,
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
        let mut box_ = RenameBox::message("查找");
        if !needle.is_empty() {
            box_.set_text(needle);
        }
        SearchPanel {
            box_,
            root,
            hits: Vec::new(),
            selected: 0,
            focus: SearchFocus::Input,
            status: "写要找的文字，回车开始找".to_owned(),
            running: false,
            answers: None,
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

    /// Adds text at the caret of the text that is looked for.
    ///
    /// The text that an input method composes goes into the box, not into
    /// the document behind it.
    pub fn type_text(&mut self, text: &str) {
        self.box_.type_text(text);
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
        std::thread::spawn(move || {
            let results = search(&root, &needle, SearchOptions::default());
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
        self.status = if results.truncated {
            format!("找到 {} 处（太多了，只看前面这些）", self.hits.len())
        } else if self.hits.is_empty() {
            "没有找到".to_owned()
        } else {
            format!("在 {} 个文件里找到 {} 处", results.files, self.hits.len())
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
    ) -> SearchCommand {
        match key {
            Key::Named(NamedKey::Escape) => return SearchCommand::Close,
            Key::Named(NamedKey::Tab) => {
                self.focus = match self.focus {
                    SearchFocus::Input => SearchFocus::Results,
                    SearchFocus::Results => SearchFocus::Input,
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
                };
            }
            Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                // A key that changes the text belongs to the text.
                self.focus = SearchFocus::Input;
                self.box_.handle_key(key, text, control);
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
                    self.focus = SearchFocus::Input;
                    self.box_.type_text(character);
                }
                return SearchCommand::None;
            }
            _ => {}
        }
        self.box_.handle_key(key, text, control);
        SearchCommand::None
    }
}

/// The ground the panel of the search stands on.
pub fn draw_background(scene: &mut FrameScene, viewport: Rect) {
    scene.push_quad(viewport, Color::rgb(0.055, 0.063, 0.082), viewport);
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
    let box_ = Rect::new(
        BOX_LEFT,
        BOX_TOP,
        BOX_WIDTH.min((size.width - BOX_LEFT * 2.0).max(1.0)),
        BOX_HEIGHT,
    );
    scene.push_quad(box_, Color::rgb(0.10, 0.12, 0.16), viewport);
    scene.push_quad(
        Rect::new(box_.origin.x, box_.origin.y, box_.size.width, 1.0),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );

    // The name of the box.
    let label = panel.field().title();
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

    // The text that is looked for.
    let field = Rect::new(
        box_.origin.x + 16.0 + label_width,
        box_.origin.y + 8.0,
        (box_.size.width - label_width - 32.0).max(1.0),
        24.0,
    );
    let needle = panel.needle();
    let empty = needle.is_empty();
    let shown = if empty { "（还没写）" } else { needle };
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
    if panel.focus() == SearchFocus::Input {
        // The caret stands after the text that was typed.
        let width = measurer.measure(needle, 14.0).width;
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

    // What the search says.
    scene.push_text(
        panel.status().to_owned(),
        Rect::new(
            BOX_LEFT,
            LIST_TOP,
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
        LIST_TOP + 24.0,
        (size.width - BOX_LEFT * 2.0).max(1.0),
        (size.height - LIST_TOP - 60.0).max(1.0),
    );
    let room = (list.size.height / ROW_HEIGHT).floor().max(1.0) as usize;
    let shown = panel.hits().len().min(room);
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
        HINT.to_owned(),
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
    fn typing_goes_into_the_text_that_is_looked_for() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "");
        assert_eq!(
            panel.handle_key(&character("n"), Some("n"), false),
            SearchCommand::None
        );
        assert_eq!(panel.needle(), "n");
        assert_eq!(panel.focus(), SearchFocus::Input);
    }

    #[test]
    fn the_escape_key_closes_the_panel() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "");
        assert_eq!(
            panel.handle_key(&named(NamedKey::Escape), None, false),
            SearchCommand::Close
        );
    }

    #[test]
    fn the_tab_key_moves_between_the_text_and_the_places() {
        let mut panel = SearchPanel::new(PathBuf::from("C:\\project"), "");
        panel.handle_key(&named(NamedKey::Tab), None, false);
        assert_eq!(panel.focus(), SearchFocus::Results);
        panel.handle_key(&named(NamedKey::Tab), None, false);
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
        match panel.handle_key(&named(NamedKey::Enter), None, false) {
            SearchCommand::Open(hit) => {
                assert_eq!(hit.path, root.join("a.txt"));
                assert_eq!(hit.line, 1);
            }
            other => panic!("the enter key did not open the place: {other:?}"),
        }
    }
}
