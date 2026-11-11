use std::ops::Range;

use codeweaver_text::{
    Anchor, ApplyError, Bias, Buffer, BufferSnapshot, TextEdit, TextOffset, Transaction,
};

/// The current selection, expressed as two anchors.
///
/// Anchors are re-resolved against the buffer after every edit, so the
/// selection keeps pointing at the same text instead of at a stale byte
/// offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The fixed end of the selection.
    pub anchor: Anchor,
    /// The end that follows the caret.
    pub head: Anchor,
}

impl Selection {
    /// The selected range, in byte offsets.
    pub fn range(&self, buffer: &Buffer) -> Range<TextOffset> {
        let anchor = buffer.resolve_anchor(&self.anchor).unwrap_or_default();
        let head = buffer.resolve_anchor(&self.head).unwrap_or_default();
        anchor.min(head)..anchor.max(head)
    }

    /// The caret position, in bytes.
    pub fn head(&self, buffer: &Buffer) -> TextOffset {
        buffer.resolve_anchor(&self.head).unwrap_or_default()
    }

    pub fn is_empty(&self, buffer: &Buffer) -> bool {
        self.range(buffer).is_empty()
    }
}

/// One step of the undo or redo history.
///
/// Every entry describes a single step in one direction:
///
/// - `transaction` transforms the state before the step into the state after
///   it;
/// - `caret_before` is the caret in the state before the step;
/// - `caret_after` is the caret in the state after the step.
///
/// The undo stack therefore holds the inverse of every edit, and the redo
/// stack holds the edits themselves. Because the history is only ever walked
/// in strict LIFO order, the byte offsets recorded here always belong to the
/// coordinate space the step is applied in.
#[derive(Clone, Debug)]
struct HistoryEntry {
    transaction: Transaction,
    caret_before: TextOffset,
    caret_after: TextOffset,
}

/// The editing state of one document.
#[derive(Clone, Debug)]
pub struct Editor {
    buffer: Buffer,
    selection: Option<Selection>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new("")
    }
}

impl Editor {
    pub fn new(text: impl Into<String>) -> Self {
        let mut editor = Editor {
            buffer: Buffer::new(text),
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        };
        editor.reset_selection(TextOffset(0));
        editor
    }

    /// Replaces the buffer's contents with `text`.
    ///
    /// Loading a file starts a fresh editing session: the undo history is
    /// cleared, because undoing past the moment the file was opened would
    /// restore text that was never in the buffer, and the selection moves to
    /// the start of the document.
    pub fn load_text(&mut self, text: impl Into<String>) {
        self.buffer = Buffer::new(text);
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.reset_selection(TextOffset(0));
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    pub fn snapshot(&self) -> BufferSnapshot {
        self.buffer.snapshot()
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    /// The version of the buffer, without copying the text.
    ///
    /// `snapshot()` copies the whole document; anything that only needs the
    /// version must use this instead.
    pub fn version(&self) -> codeweaver_text::BufferVersion {
        self.buffer.version()
    }

    /// The text before the caret.
    pub fn text_before_caret(&self) -> String {
        let caret = self.caret();
        self.buffer.slice(TextOffset(0)..caret)
    }

    /// The text after the caret.
    pub fn text_after_caret(&self) -> String {
        let caret = self.caret();
        self.buffer.slice(caret..self.buffer.len())
    }

    pub fn selection(&self) -> Selection {
        self.selection
            .expect("the selection is initialised in Editor::new")
    }

    /// The caret position, in bytes.
    pub fn caret(&self) -> TextOffset {
        self.selection().head(&self.buffer)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Inserts `text` at the caret, replacing the selection when there is one.
    pub fn insert(&mut self, text: &str) -> Result<(), ApplyError> {
        let range = self.selection().range(&self.buffer);
        let caret = TextOffset(range.start.0 + text.len());
        self.commit_edit(range, text, caret)
    }

    /// Deletes the selection, or the character before the caret.
    pub fn backspace(&mut self) -> Result<(), ApplyError> {
        let selection = self.selection();
        let range = if selection.is_empty(&self.buffer) {
            let caret = selection.head(&self.buffer);
            if caret.0 == 0 {
                return Ok(());
            }
            let previous = self.buffer.prev_char_boundary(caret);
            previous..caret
        } else {
            selection.range(&self.buffer)
        };
        let caret = range.start;
        self.commit_edit(range, "", caret)
    }

    pub fn move_left(&mut self) {
        let range = self.selection().range(&self.buffer);
        let caret = if range.is_empty() {
            self.buffer.prev_char_boundary(range.start)
        } else {
            range.start
        };
        self.reset_selection(caret);
    }

    pub fn move_right(&mut self) {
        let range = self.selection().range(&self.buffer);
        let caret = if range.is_empty() {
            self.buffer.next_char_boundary(range.start)
        } else {
            range.end
        };
        self.reset_selection(caret);
    }

    /// Selects the range `start..end`.
    pub fn select_range(&mut self, start: TextOffset, end: TextOffset) {
        self.release_selection();
        let anchor = self.buffer.anchor(start, Bias::Left);
        let head = self.buffer.anchor(end, Bias::Right);
        self.selection = Some(Selection { anchor, head });
    }

    pub fn undo(&mut self) -> Result<(), ApplyError> {
        let Some(entry) = self.undo_stack.pop() else {
            return Ok(());
        };
        let applied = self.buffer.replay(entry.transaction.clone())?;
        self.reset_selection(entry.caret_after);
        self.redo_stack.push(HistoryEntry {
            transaction: applied.inverse,
            caret_before: entry.caret_after,
            caret_after: entry.caret_before,
        });
        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), ApplyError> {
        let Some(entry) = self.redo_stack.pop() else {
            return Ok(());
        };
        let applied = self.buffer.replay(entry.transaction.clone())?;
        self.reset_selection(entry.caret_after);
        self.undo_stack.push(HistoryEntry {
            transaction: applied.inverse,
            caret_before: entry.caret_after,
            caret_after: entry.caret_before,
        });
        Ok(())
    }

    /// Applies an edit and records it in the undo history.
    fn commit_edit(
        &mut self,
        range: Range<TextOffset>,
        replacement: &str,
        caret: TextOffset,
    ) -> Result<(), ApplyError> {
        let caret_before = self.caret();
        let version = self.buffer.version();
        let applied = self.buffer.apply(Transaction::new(
            version,
            vec![TextEdit::new(range, replacement)],
        ))?;
        // The undo step runs the edit backwards, so it starts at the caret the
        // edit produced and ends at the caret the edit started from.
        self.undo_stack.push(HistoryEntry {
            transaction: applied.inverse,
            caret_before: caret,
            caret_after: caret_before,
        });
        // A new edit invalidates the redo branch.
        self.redo_stack.clear();
        self.reset_selection(caret);
        Ok(())
    }

    /// Points the selection at `caret`, replacing the previous anchors.
    fn reset_selection(&mut self, caret: TextOffset) {
        self.release_selection();
        let anchor = self.buffer.anchor(caret, Bias::Right);
        let head = self.buffer.anchor(caret, Bias::Right);
        self.selection = Some(Selection { anchor, head });
    }

    fn release_selection(&mut self) {
        if let Some(previous) = self.selection.take() {
            self.buffer.release_anchor(&previous.anchor);
            self.buffer.release_anchor(&previous.head);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_and_deletes_unicode_at_utf8_boundaries() {
        let mut editor = Editor::new("");
        editor.insert("A中文").unwrap();
        editor.backspace().unwrap();
        assert_eq!(editor.text(), "A中");
        assert_eq!(editor.caret(), TextOffset(4));
    }

    #[test]
    fn undo_and_redo_restore_text_and_selection() {
        let mut editor = Editor::new("");
        editor.insert("中文").unwrap();
        editor.insert("!").unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.text(), "中文");
        assert_eq!(editor.caret(), TextOffset(6));
        editor.redo().unwrap();
        assert_eq!(editor.text(), "中文!");
        assert_eq!(editor.caret(), TextOffset(7));
    }

    #[test]
    fn moving_crosses_whole_unicode_scalars() {
        let mut editor = Editor::new("");
        editor.insert("A中B").unwrap();
        editor.move_left();
        editor.move_left();
        assert_eq!(editor.caret(), TextOffset(1));
        editor.move_right();
        assert_eq!(editor.caret(), TextOffset(4));
    }

    #[test]
    fn typing_over_a_selection_replaces_it_and_collapses_the_caret() {
        let mut editor = Editor::new("");
        editor.insert("hello").unwrap();
        editor.select_range(TextOffset(2), TextOffset(5));
        assert_eq!(
            editor.selection().range(editor.buffer()),
            TextOffset(2)..TextOffset(5)
        );

        editor.insert("X").unwrap();
        assert_eq!(editor.text(), "heX");
        assert_eq!(editor.caret(), TextOffset(3));
        assert!(editor.selection().range(editor.buffer()).is_empty());
    }

    #[test]
    fn backspace_deletes_the_selected_range() {
        let mut editor = Editor::new("");
        editor.insert("hello").unwrap();
        editor.select_range(TextOffset(1), TextOffset(4));
        editor.backspace().unwrap();
        assert_eq!(editor.text(), "ho");
        assert_eq!(editor.caret(), TextOffset(1));
    }

    #[test]
    fn undo_restores_the_text_and_the_caret() {
        let mut editor = Editor::new("");
        editor.insert("hello").unwrap();
        editor.backspace().unwrap();
        assert_eq!(editor.text(), "hell");
        editor.undo().unwrap();
        assert_eq!(editor.text(), "hello");
        assert_eq!(editor.caret(), TextOffset(5));
    }

    #[test]
    fn a_new_edit_drops_the_redo_branch() {
        let mut editor = Editor::new("");
        editor.insert("a").unwrap();
        editor.undo().unwrap();
        assert!(editor.can_redo());
        editor.insert("b").unwrap();
        assert!(!editor.can_redo());
        assert_eq!(editor.text(), "b");
    }

    #[test]
    fn backspace_at_the_start_of_the_text_does_nothing() {
        let mut editor = Editor::new("x");
        editor.move_left();
        editor.backspace().unwrap();
        assert_eq!(editor.text(), "x");
        assert_eq!(editor.caret(), TextOffset(0));
    }

    #[test]
    fn loading_text_replaces_the_buffer_and_clears_the_history() {
        let mut editor = Editor::new("old");
        editor.insert("!").unwrap();
        assert!(editor.can_undo());

        editor.load_text("new content");
        assert_eq!(editor.text(), "new content");
        assert_eq!(editor.caret(), TextOffset(0));
        assert!(
            !editor.can_undo(),
            "the old history must not survive a load"
        );
        assert!(!editor.can_redo());
    }

    #[test]
    fn loading_text_keeps_the_buffer_editable() {
        let mut editor = Editor::new("");
        editor.load_text("hello");
        editor.move_right();
        editor.insert("!").unwrap();
        assert_eq!(editor.text(), "h!ello");
        editor.undo().unwrap();
        assert_eq!(editor.text(), "hello");
    }
}
