use std::ops::Range;

use codeweaver_text::{ApplyError, Buffer, BufferSnapshot, TextEdit, TextOffset, Transaction};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: TextOffset,
    pub head: TextOffset,
}

impl Selection {
    pub fn caret(offset: TextOffset) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }
    pub fn range(&self) -> Range<TextOffset> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Debug)]
struct HistoryEntry {
    transaction: Transaction,
    selection: Selection,
}

#[derive(Clone, Debug, Default)]
pub struct Editor {
    buffer: Buffer,
    selection: Selection,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
}

impl Editor {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            buffer: Buffer::new(text),
            ..Self::default()
        }
    }
    pub fn snapshot(&self) -> BufferSnapshot {
        self.buffer.snapshot()
    }
    pub fn text(&self) -> &str {
        self.buffer.text()
    }
    pub fn selection(&self) -> &Selection {
        &self.selection
    }
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn insert(&mut self, text: &str) -> Result<(), ApplyError> {
        let range = self.selection.range();
        let new_offset = TextOffset(range.start.0 + text.len());
        self.apply_user_edit(TextEdit::new(range, text), Selection::caret(new_offset))
    }

    pub fn backspace(&mut self) -> Result<(), ApplyError> {
        let range = if self.selection.is_empty() {
            let caret = self.selection.head.0;
            if caret == 0 {
                return Ok(());
            }
            let previous = self.buffer.text()[..caret]
                .char_indices()
                .next_back()
                .map_or(0, |(offset, _)| offset);
            TextOffset(previous)..TextOffset(caret)
        } else {
            self.selection.range()
        };
        let caret = range.start;
        self.apply_user_edit(TextEdit::new(range, ""), Selection::caret(caret))
    }

    pub fn move_left(&mut self) {
        if !self.selection.is_empty() {
            self.selection = Selection::caret(self.selection.range().start);
            return;
        }
        let caret = self.selection.head.0;
        let previous = self.buffer.text()[..caret]
            .char_indices()
            .next_back()
            .map_or(0, |(offset, _)| offset);
        self.selection = Selection::caret(TextOffset(previous));
    }

    pub fn move_right(&mut self) {
        if !self.selection.is_empty() {
            self.selection = Selection::caret(self.selection.range().end);
            return;
        }
        let caret = self.selection.head.0;
        let next = self.buffer.text()[caret..]
            .chars()
            .next()
            .map_or(caret, |character| caret + character.len_utf8());
        self.selection = Selection::caret(TextOffset(next));
    }

    pub fn undo(&mut self) -> Result<(), ApplyError> {
        self.replay_history(true)
    }
    pub fn redo(&mut self) -> Result<(), ApplyError> {
        self.replay_history(false)
    }

    fn apply_user_edit(
        &mut self,
        edit: TextEdit,
        next_selection: Selection,
    ) -> Result<(), ApplyError> {
        let previous_selection = self.selection.clone();
        let applied = self
            .buffer
            .apply(Transaction::new(self.buffer.version(), vec![edit]))?;
        self.undo_stack.push(HistoryEntry {
            transaction: applied.inverse,
            selection: previous_selection,
        });
        self.redo_stack.clear();
        self.selection = next_selection;
        Ok(())
    }

    fn replay_history(&mut self, undo: bool) -> Result<(), ApplyError> {
        let entry = if undo {
            self.undo_stack.pop()
        } else {
            self.redo_stack.pop()
        };
        let Some(entry) = entry else {
            return Ok(());
        };
        let current_selection = self.selection.clone();
        let applied = self.buffer.apply(entry.transaction)?;
        let opposite = HistoryEntry {
            transaction: applied.inverse,
            selection: current_selection,
        };
        if undo {
            self.redo_stack.push(opposite);
        } else {
            self.undo_stack.push(opposite);
        }
        self.selection = entry.selection;
        Ok(())
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
        assert_eq!(editor.selection().head, TextOffset(4));
    }

    #[test]
    fn undo_and_redo_restore_text_and_selection() {
        let mut editor = Editor::new("");
        editor.insert("中文").unwrap();
        editor.insert("!").unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.text(), "中文");
        assert_eq!(editor.selection().head, TextOffset(6));
        editor.redo().unwrap();
        assert_eq!(editor.text(), "中文!");
        assert_eq!(editor.selection().head, TextOffset(7));
    }

    #[test]
    fn moving_crosses_whole_unicode_scalars() {
        let mut editor = Editor::new("");
        editor.insert("A中B").unwrap();
        editor.move_left();
        editor.move_left();
        assert_eq!(editor.selection().head, TextOffset(1));
        editor.move_right();
        assert_eq!(editor.selection().head, TextOffset(4));
    }
}
