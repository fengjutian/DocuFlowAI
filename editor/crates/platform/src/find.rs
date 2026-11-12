//! The find bar: searching the text and replacing what was found.
//!
//! The find bar sits in the corner of the window. It holds the text that is
//! searched for, the text that replaces it, and the matches that were found.
//! The text itself stays in the editor's core: the find bar only knows where
//! the matches are.
//!
//! The bar is built on the editor's multi-cursor model: replacing every match
//! is one transaction, and "select every match" turns the matches into
//! selections, so an edit then reaches all of them at once.

use std::ops::Range;

use codeweaver_editor_core::Editor;
use codeweaver_text::TextOffset;

/// Which field the user is typing into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FindField {
    /// The text that is searched for.
    #[default]
    Query,
    /// The text that replaces what was found.
    Replacement,
}

/// The find bar.
#[derive(Clone, Debug, Default)]
pub struct FindBar {
    /// The text that is searched for.
    query: String,
    /// The text that replaces what was found.
    replacement: String,
    /// The field the user is typing into.
    field: FindField,
    /// Whether the search looks for the exact case.
    case_sensitive: bool,
    /// Whether the bar replaces as well as finds.
    replacing: bool,
    /// The matches, in the order of the text.
    matches: Vec<Range<TextOffset>>,
    /// Which match is the current one.
    current: usize,
}

impl FindBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// The text that is searched for.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The text that replaces what was found.
    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    /// The field the user is typing into.
    pub fn field(&self) -> FindField {
        self.field
    }

    /// Whether the bar replaces as well as finds.
    pub fn replacing(&self) -> bool {
        self.replacing
    }

    /// Whether the search looks for the exact case.
    pub fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// The matches, in the order of the text.
    pub fn matches(&self) -> &[Range<TextOffset>] {
        &self.matches
    }

    /// The match that is current, if there is one.
    pub fn current(&self) -> Option<&Range<TextOffset>> {
        self.matches.get(self.current)
    }

    /// Which match is current, counted from the first one.
    pub fn current_index(&self) -> usize {
        self.current
    }

    /// How many matches the search found.
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Opens the bar and searches for what the editor has selected.
    ///
    /// The selected text is what the user is most likely looking for, so it
    /// becomes the query.
    pub fn open(&mut self, editor: &mut Editor, replacing: bool) {
        self.replacing = replacing;
        self.field = FindField::Query;
        let selected = editor.selection_ranges();
        if selected.len() == 1 {
            let range = selected[0].clone();
            if range.start < range.end {
                let text = editor.buffer().slice(range);
                // A selection that reaches over a line break is not a query.
                if !text.contains('\n') {
                    self.query = text;
                }
            }
        }
        self.search(editor);
    }

    /// Adds text to the field the user is typing into.
    pub fn type_text(&mut self, editor: &Editor, text: &str) {
        match self.field {
            FindField::Query => self.query.push_str(text),
            FindField::Replacement => self.replacement.push_str(text),
        }
        self.search(editor);
    }

    /// Removes the last character of the field the user is typing into.
    pub fn backspace(&mut self, editor: &Editor) {
        match self.field {
            FindField::Query => {
                self.query.pop();
            }
            FindField::Replacement => {
                self.replacement.pop();
            }
        }
        self.search(editor);
    }

    /// Switches between the query and the replacement field.
    pub fn toggle_field(&mut self) {
        self.field = match self.field {
            FindField::Query => FindField::Replacement,
            FindField::Replacement => FindField::Query,
        };
    }

    /// Turns the case sensitivity on or off and searches again.
    pub fn toggle_case_sensitive(&mut self, editor: &Editor) {
        self.case_sensitive = !self.case_sensitive;
        self.search(editor);
    }

    /// Searches the text again.
    ///
    /// The current match becomes the first one at or after the caret, so
    /// opening the bar where the user is working starts there.
    pub fn search(&mut self, editor: &Editor) {
        self.matches = editor.buffer().find_all(&self.query, self.case_sensitive);
        let caret = editor.caret().0;
        self.current = self
            .matches
            .iter()
            .position(|range| range.start.0 >= caret)
            .unwrap_or(0);
    }

    /// Moves to the next match.
    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current = (self.current + 1) % self.matches.len();
    }

    /// Moves to the previous match.
    pub fn previous_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current = if self.current == 0 {
            self.matches.len() - 1
        } else {
            self.current - 1
        };
    }

    /// Replaces the current match with the replacement text.
    ///
    /// Returns `true` when something was replaced.
    pub fn replace_current(&mut self, editor: &mut Editor) -> bool {
        let Some(range) = self.matches.get(self.current).cloned() else {
            return false;
        };
        let replacement = self.replacement.clone();
        if let Err(error) = editor.replace_ranges(vec![range], &replacement) {
            eprintln!("could not replace the match: {error}");
            return false;
        }
        self.search(editor);
        true
    }

    /// Replaces every match with the replacement text.
    ///
    /// All of them are replaced in one step, so one undo puts the text back.
    /// Returns how many were replaced.
    pub fn replace_all(&mut self, editor: &mut Editor) -> usize {
        if self.matches.is_empty() {
            return 0;
        }
        let ranges = self.matches.clone();
        let count = ranges.len();
        let replacement = self.replacement.clone();
        if let Err(error) = editor.replace_ranges(ranges, &replacement) {
            eprintln!("could not replace the matches: {error}");
            return 0;
        }
        self.search(editor);
        count
    }

    /// Turns every match into a selection.
    ///
    /// The user can then edit all of them at once, which is what the
    /// multi-cursor model is for. Returns how many matches were selected.
    pub fn select_all_matches(&mut self, editor: &mut Editor) -> usize {
        if self.matches.is_empty() {
            return 0;
        }
        let ranges = self.matches.clone();
        let count = ranges.len();
        editor.set_selection_ranges(&ranges);
        count
    }

    /// What the bar shows about the search: "3/12", or that nothing was found.
    pub fn status(&self) -> String {
        if self.query.is_empty() {
            return String::new();
        }
        if self.matches.is_empty() {
            return "无结果".to_owned();
        }
        format!("{}/{}", self.current + 1, self.matches.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor with the given text and the caret at the given offset.
    fn editor_with(text: &str, caret: usize) -> Editor {
        let mut editor = Editor::new(text);
        editor.select_range(TextOffset(caret), TextOffset(caret));
        editor
    }

    #[test]
    fn a_search_finds_the_matches_in_the_text() {
        let mut editor = editor_with("one two one", 0);
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        find.type_text(&editor, "one");
        assert_eq!(find.match_count(), 2);
        assert_eq!(find.matches()[0], TextOffset(0)..TextOffset(3));
        assert_eq!(find.matches()[1], TextOffset(8)..TextOffset(11));
    }

    #[test]
    fn the_current_match_starts_at_the_caret() {
        let mut editor = editor_with("one two one", 8);
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        find.type_text(&editor, "one");
        assert_eq!(find.current(), Some(&(TextOffset(8)..TextOffset(11))));
    }

    #[test]
    fn the_next_match_wraps_around() {
        let mut editor = editor_with("one two one", 0);
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        find.type_text(&editor, "one");
        find.next_match();
        assert_eq!(find.current(), Some(&(TextOffset(8)..TextOffset(11))));
        find.next_match();
        assert_eq!(find.current(), Some(&(TextOffset(0)..TextOffset(3))));
        find.previous_match();
        assert_eq!(find.current(), Some(&(TextOffset(8)..TextOffset(11))));
    }

    #[test]
    fn the_query_starts_from_the_selected_text() {
        let mut editor = editor_with("one two one", 0);
        editor.select_range(TextOffset(4), TextOffset(7));
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        assert_eq!(find.query(), "two");
        assert_eq!(find.match_count(), 1);
    }

    #[test]
    fn a_selection_that_reaches_over_a_line_break_is_not_a_query() {
        let mut editor = editor_with("one\ntwo", 0);
        editor.select_range(TextOffset(2), TextOffset(5));
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        assert_eq!(find.query(), "");
    }

    #[test]
    fn replacing_the_current_match_changes_the_text() {
        let mut editor = editor_with("one two one", 0);
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        find.type_text(&editor, "one");
        find.toggle_field();
        find.type_text(&editor, "three");
        assert!(find.replace_current(&mut editor));
        assert_eq!(editor.text(), "three two one");
        // The caret sits after the text that replaced the match, so the
        // search looks for the match that comes next.
        assert_eq!(find.current(), Some(&(TextOffset(10)..TextOffset(13))));
    }

    #[test]
    fn replacing_every_match_is_one_step() {
        let mut editor = editor_with("one two one", 0);
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        find.type_text(&editor, "one");
        find.toggle_field();
        find.type_text(&editor, "three");
        assert_eq!(find.replace_all(&mut editor), 2);
        assert_eq!(editor.text(), "three two three");
        // One undo puts the whole text back.
        editor.undo().unwrap();
        assert_eq!(editor.text(), "one two one");
    }

    #[test]
    fn selecting_every_match_makes_a_selection_at_each_of_them() {
        let mut editor = editor_with("one two one", 0);
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        find.type_text(&editor, "one");
        assert_eq!(find.select_all_matches(&mut editor), 2);
        assert_eq!(editor.cursor_count(), 2);
        // Editing now reaches every match at once.
        editor.insert("X").unwrap();
        assert_eq!(editor.text(), "X two X");
    }

    #[test]
    fn a_search_ignores_the_case_until_it_is_asked_not_to() {
        let mut editor = editor_with("One ONE one", 0);
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        find.type_text(&editor, "one");
        assert_eq!(find.match_count(), 3);
        find.toggle_case_sensitive(&editor);
        assert_eq!(find.match_count(), 1);
    }

    #[test]
    fn the_status_says_how_many_matches_were_found() {
        let mut editor = editor_with("one two one", 0);
        let mut find = FindBar::new();
        find.open(&mut editor, false);
        assert_eq!(find.status(), "");
        find.type_text(&editor, "one");
        assert_eq!(find.status(), "1/2");
        find.type_text(&editor, "zzz");
        assert_eq!(find.status(), "无结果");
    }
}
