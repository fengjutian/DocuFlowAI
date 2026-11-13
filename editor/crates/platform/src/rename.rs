//! The box that asks for the new name of a thing.
//!
//! The name is settled before the language server is asked to do the
//! renaming: the user may want to change it again, and the server should
//! only be troubled once the name is what the user wants.
//!
//! The box knows which name it is about, so that the editor can say where
//! the name is in the document without keeping that in two places.

use codeweaver_text::Buffer;

/// The box that asks for the new name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenameBox {
    /// The name as it is now.
    original: String,
    /// The name as the user has it.
    current: String,
    /// Where the caret is in the name, counted in bytes.
    caret: usize,
    /// The byte offset where the name starts in the document.
    start: usize,
}

impl RenameBox {
    /// Opens the box with the name as it is now.
    ///
    /// `start` is where the name begins in the document: the place the
    /// language server is asked about.
    pub fn new(original: impl Into<String>, start: usize) -> Self {
        let original = original.into();
        let caret = original.len();
        RenameBox {
            current: original.clone(),
            original,
            caret,
            start,
        }
    }

    /// The name as it is now.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// The name as the user has it.
    pub fn text(&self) -> &str {
        &self.current
    }

    /// Where the name starts in the document.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Where the caret is in the name.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Whether the name is still the one it was.
    pub fn is_unchanged(&self) -> bool {
        self.current == self.original
    }

    /// Adds text at the caret.
    pub fn type_text(&mut self, text: &str) {
        self.current.insert_str(self.caret, text);
        self.caret += text.len();
    }

    /// Removes the character before the caret.
    pub fn backspace(&mut self) {
        if self.caret == 0 {
            return;
        }
        let start = self.previous_boundary(self.caret);
        self.current.replace_range(start..self.caret, "");
        self.caret = start;
    }

    /// Removes the character at the caret.
    pub fn delete(&mut self) {
        if self.caret >= self.current.len() {
            return;
        }
        let end = self.next_boundary(self.caret);
        self.current.replace_range(self.caret..end, "");
    }

    /// Moves the caret one character to the left.
    pub fn left(&mut self) {
        if self.caret == 0 {
            return;
        }
        self.caret = self.previous_boundary(self.caret);
    }

    /// Moves the caret one character to the right.
    pub fn right(&mut self) {
        if self.caret >= self.current.len() {
            return;
        }
        self.caret = self.next_boundary(self.caret);
    }

    /// Moves the caret to the beginning of the name.
    pub fn home(&mut self) {
        self.caret = 0;
    }

    /// Moves the caret to the end of the name.
    pub fn end(&mut self) {
        self.caret = self.current.len();
    }

    /// The place before the given one that is between two characters.
    fn previous_boundary(&self, place: usize) -> usize {
        let mut start = place.saturating_sub(1);
        while start > 0 && !self.current.is_char_boundary(start) {
            start -= 1;
        }
        start
    }

    /// The place after the given one that is between two characters.
    fn next_boundary(&self, place: usize) -> usize {
        let mut end = place + 1;
        while end < self.current.len() && !self.current.is_char_boundary(end) {
            end += 1;
        }
        end.min(self.current.len())
    }
}

/// The name that is under the caret, and where it is.
///
/// The name is the run of letters, digits and underscores around the caret:
/// the same thing a completion replaces. The caret may be in the middle of
/// the name or at its end; either way the whole name is found.
///
/// Returns the byte range of the name in the document, or nothing when the
/// caret is not on a name at all.
pub fn word_at_caret(buffer: &Buffer, caret: usize) -> Option<(usize, usize)> {
    let line = buffer.line_of_offset(caret);
    let text = buffer.line_text(line);
    let start = buffer.line_range(line).start.0;
    let caret = caret.min(start + text.len());
    let column = caret - start;
    // The caret is between two characters, so the text in front of it can be
    // cut there.
    let column = text
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= column)
        .last()
        .unwrap_or(0);
    let column = if column > text.len() { text.len() } else { column };

    let is_name_character =
        |character: char| character.is_alphanumeric() || character == '_';

    let mut left = column;
    for (index, character) in text[..column].char_indices().rev() {
        if !is_name_character(character) {
            break;
        }
        left = index;
    }
    let mut right = column;
    for (index, character) in text[column..].char_indices() {
        if !is_name_character(character) {
            break;
        }
        // The index is counted from the caret, so the place in the line is
        // the caret plus it.
        right = column + index + character.len_utf8();
    }
    if left == right {
        return None;
    }
    Some((start + left, start + right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_under_the_caret_is_found() {
        let buffer = Buffer::new("let value = 1;\n");
        // The caret is in the middle of "value".
        let (start, end) = word_at_caret(&buffer, 6).expect("there is a name");
        assert_eq!((start, end), (4, 9));
    }

    #[test]
    fn the_name_is_found_when_the_caret_is_at_its_end() {
        let buffer = Buffer::new("let value = 1;\n");
        let (start, end) = word_at_caret(&buffer, 9).expect("there is a name");
        assert_eq!((start, end), (4, 9));
    }

    #[test]
    fn there_is_no_name_where_there_is_none() {
        let buffer = Buffer::new("let value = 1;\n");
        // The caret is in front of the equals sign.
        assert_eq!(word_at_caret(&buffer, 10), None);
        // The caret is between two words.
        let buffer = Buffer::new("let  value = 1;\n");
        assert_eq!(word_at_caret(&buffer, 4), None);
    }

    #[test]
    fn a_name_with_underscores_and_digits_is_one_name() {
        let buffer = Buffer::new("let my_value2 = 1;\n");
        let (start, end) = word_at_caret(&buffer, 6).expect("there is a name");
        assert_eq!((start, end), (4, 13));
    }

    #[test]
    fn the_box_starts_with_the_name_as_it_is() {
        let box_ = RenameBox::new("value", 4);
        assert_eq!(box_.text(), "value");
        assert_eq!(box_.original(), "value");
        assert_eq!(box_.start(), 4);
        assert!(box_.is_unchanged());
    }

    #[test]
    fn typing_changes_the_name_and_where_the_caret_is() {
        let mut box_ = RenameBox::new("value", 0);
        box_.type_text("s");
        assert_eq!(box_.text(), "values");
        assert_eq!(box_.caret(), 6);
        assert!(!box_.is_unchanged());
    }

    #[test]
    fn the_caret_can_go_back_and_forth_in_the_name() {
        let mut box_ = RenameBox::new("value", 0);
        box_.left();
        assert_eq!(box_.caret(), 4);
        box_.home();
        assert_eq!(box_.caret(), 0);
        box_.right();
        assert_eq!(box_.caret(), 1);
        box_.end();
        assert_eq!(box_.caret(), 5);
    }

    #[test]
    fn the_name_can_be_edited_in_the_middle() {
        let mut box_ = RenameBox::new("value", 0);
        box_.left();
        box_.type_text("s");
        assert_eq!(box_.text(), "valuse");
        box_.backspace();
        assert_eq!(box_.text(), "value");
        assert_eq!(box_.caret(), 4);
    }

    #[test]
    fn nothing_happens_at_the_ends_of_the_name() {
        let mut box_ = RenameBox::new("value", 0);
        box_.home();
        box_.backspace();
        assert_eq!(box_.text(), "value");
        box_.left();
        assert_eq!(box_.caret(), 0);
        box_.end();
        box_.delete();
        assert_eq!(box_.text(), "value");
    }

    #[test]
    fn a_name_with_a_character_outside_the_basic_plane_survives_the_edits() {
        let mut box_ = RenameBox::new("a😀b", 0);
        box_.left();
        box_.backspace();
        assert_eq!(box_.text(), "ab");
        assert_eq!(box_.caret(), 1);
    }
}
