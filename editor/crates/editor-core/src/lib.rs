use std::ops::Range;

use codeweaver_text::{
    Anchor, ApplyError, Bias, Buffer, BufferChange, BufferSnapshot, TextEdit, TextOffset,
    Transaction,
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
/// - `carets_from` are the carets of the state before the step;
/// - `carets_to` are the carets of the state after the step.
///
/// The undo stack therefore holds the inverse of every edit, and the redo
/// stack holds the edits themselves. Because the history is only ever walked
/// in strict LIFO order, the byte offsets recorded here always belong to the
/// coordinate space the step is applied in.
#[derive(Clone, Debug)]
struct HistoryEntry {
    transaction: Transaction,
    carets_from: Vec<TextOffset>,
    carets_to: Vec<TextOffset>,
}

/// The editing state of one document.
#[derive(Clone, Debug)]
pub struct Editor {
    buffer: Buffer,
    /// The selections, in the order of the text.
    ///
    /// There is always at least one. The first one is the primary selection;
    /// the others are the extra carets of a multi-cursor edit.
    selections: Vec<Selection>,
    /// The undo stack holds the inverse of every edit, and the redo
    /// stack holds the edits themselves. Because the history is only ever walked
    /// in strict LIFO order, the byte offsets recorded here always belong to the
    /// coordinate space the step is applied in.
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    /// What the last change to the buffer was.
    ///
    /// The syntax highlighter needs to know how the text moved to follow the
    /// tree of the last parse, and the buffer only keeps the text that is
    /// there now.
    last_change: Option<BufferChange>,
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
            selections: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_change: None,
        };
        editor.set_carets(&[TextOffset(0)]);
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
        self.last_change = None;
        self.set_carets(&[TextOffset(0)]);
    }

    /// What the last change to the buffer was.
    ///
    /// The change holds the edits that were applied and the text that they
    /// replaced. A parser that keeps a tree of the text needs both to follow
    /// the text as it moves.
    pub fn last_change(&self) -> Option<&BufferChange> {
        self.last_change.as_ref()
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

    /// Every selection, in the order of the text.
    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    /// The primary selection: the topmost one.
    pub fn selection(&self) -> &Selection {
        &self.selections[0]
    }

    /// The caret position of the primary selection, in bytes.
    pub fn caret(&self) -> TextOffset {
        self.selection().head(&self.buffer)
    }

    /// The position of every caret, in bytes.
    pub fn carets(&self) -> Vec<TextOffset> {
        self.selections
            .iter()
            .map(|selection| selection.head(&self.buffer))
            .collect()
    }

    /// How many carets there are.
    pub fn cursor_count(&self) -> usize {
        self.selections.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Inserts `text` at every caret, replacing the selections when there are
    /// any.
    pub fn insert(&mut self, text: &str) -> Result<(), ApplyError> {
        let ranges = self
            .selections
            .iter()
            .map(|selection| selection.range(&self.buffer))
            .collect();
        self.replace_ranges(ranges, text)
    }

    /// Deletes the selection, or the character before the caret, at every
    /// caret.
    pub fn backspace(&mut self) -> Result<(), ApplyError> {
        let ranges = self
            .selections
            .iter()
            .map(|selection| {
                let range = selection.range(&self.buffer);
                if !range.is_empty() {
                    return range;
                }
                let caret = selection.head(&self.buffer);
                if caret.0 == 0 {
                    return TextOffset(0)..TextOffset(0);
                }
                let previous = self.buffer.prev_char_boundary(caret);
                previous..caret
            })
            .collect();
        self.replace_ranges(ranges, "")
    }

    /// Deletes the character after every caret.
    pub fn delete_forward(&mut self) -> Result<(), ApplyError> {
        let ranges = self
            .selections
            .iter()
            .map(|selection| {
                let range = selection.range(&self.buffer);
                if !range.is_empty() {
                    return range;
                }
                let caret = selection.head(&self.buffer);
                if caret.0 >= self.buffer.len().0 {
                    return caret..caret;
                }
                let next = self.buffer.next_char_boundary(caret);
                caret..next
            })
            .collect();
        self.replace_ranges(ranges, "")
    }

    /// Extends every selection one character to the left.
    ///
    /// The anchor of a selection stays where it is and its head moves, which
    /// is what holding Shift does.
    pub fn extend_left(&mut self) {
        let heads: Vec<TextOffset> = self
            .selections
            .iter()
            .map(|selection| self.buffer.prev_char_boundary(selection.head(&self.buffer)))
            .collect();
        self.set_selection_heads(&heads);
    }

    /// Extends every selection one character to the right.
    pub fn extend_right(&mut self) {
        let heads: Vec<TextOffset> = self
            .selections
            .iter()
            .map(|selection| self.buffer.next_char_boundary(selection.head(&self.buffer)))
            .collect();
        self.set_selection_heads(&heads);
    }

    /// Moves the head of every selection, keeping the anchors.
    pub fn set_selection_heads(&mut self, heads: &[TextOffset]) {
        for (selection, head) in self.selections.iter_mut().zip(heads) {
            self.buffer.release_anchor(&selection.head);
            selection.head = self.buffer.anchor(*head, Bias::Right);
        }
    }

    pub fn move_left(&mut self) {
        let carets: Vec<TextOffset> = self
            .selections
            .iter()
            .map(|selection| {
                let range = selection.range(&self.buffer);
                if range.is_empty() {
                    self.buffer.prev_char_boundary(range.start)
                } else {
                    range.start
                }
            })
            .collect();
        self.set_carets(&carets);
    }

    pub fn move_right(&mut self) {
        let carets: Vec<TextOffset> = self
            .selections
            .iter()
            .map(|selection| {
                let range = selection.range(&self.buffer);
                if range.is_empty() {
                    self.buffer.next_char_boundary(range.start)
                } else {
                    range.end
                }
            })
            .collect();
        self.set_carets(&carets);
    }

    /// Selects the range `start..end`, dropping the other carets.
    pub fn select_range(&mut self, start: TextOffset, end: TextOffset) {
        self.release_selections();
        let anchor = self.buffer.anchor(start, Bias::Left);
        let head = self.buffer.anchor(end, Bias::Right);
        self.selections = vec![Selection { anchor, head }];
    }

    /// Selects the range `start..end` in the selection at `index`.
    ///
    /// The other selections are left alone, which is what a drag does: it
    /// moves the head of the selection it started on.
    pub fn select_range_at(&mut self, index: usize, start: TextOffset, end: TextOffset) {
        let Some(selection) = self.selections.get(index).copied() else {
            return;
        };
        self.buffer.release_anchor(&selection.anchor);
        self.buffer.release_anchor(&selection.head);
        let anchor = self.buffer.anchor(start, Bias::Left);
        let head = self.buffer.anchor(end, Bias::Right);
        self.selections[index] = Selection { anchor, head };
    }

    /// Adds a caret at `offset`, keeping the other selections.
    ///
    /// Returns where the caret ended up: the index of the new selection, or
    /// the index of the caret that was already there.
    pub fn add_cursor(&mut self, offset: TextOffset) -> usize {
        if let Some(index) = self
            .selections
            .iter()
            .position(|selection| selection.head(&self.buffer) == offset)
        {
            return index;
        }
        let anchor = self.buffer.anchor(offset, Bias::Right);
        let head = self.buffer.anchor(offset, Bias::Right);
        let selection = Selection { anchor, head };
        let mut position = self.selections.len();
        for (index, existing) in self.selections.iter().enumerate() {
            if existing.head(&self.buffer) > offset {
                position = index;
                break;
            }
        }
        self.selections.insert(position, selection);
        position
    }

    /// Adds a caret at every offset, keeping the other selections.
    ///
    /// The selections are kept in the order of the text, so the first one is
    /// always the topmost caret. An offset that already has a caret on it is
    /// skipped: two carets in the same place are one caret.
    pub fn add_cursors(&mut self, offsets: &[TextOffset]) {
        for offset in offsets {
            if self
                .selections
                .iter()
                .any(|selection| selection.head(&self.buffer) == *offset)
            {
                continue;
            }
            let anchor = self.buffer.anchor(*offset, Bias::Right);
            let selection = Selection {
                anchor,
                head: anchor,
            };
            let mut position = self.selections.len();
            for (index, existing) in self.selections.iter().enumerate() {
                if existing.head(&self.buffer) > *offset {
                    position = index;
                    break;
                }
            }
            self.selections.insert(position, selection);
        }
    }

    /// Drops every caret but the primary one.
    pub fn collapse_to_primary(&mut self) {
        if self.selections.len() <= 1 {
            return;
        }
        for selection in &self.selections[1..] {
            self.buffer.release_anchor(&selection.anchor);
            self.buffer.release_anchor(&selection.head);
        }
        self.selections.truncate(1);
    }

    /// Points the selections at `carets`, dropping the selections
    /// themselves.
    pub fn set_carets(&mut self, carets: &[TextOffset]) {
        self.release_selections();
        let mut selections = Vec::with_capacity(carets.len());
        for caret in carets {
            // The anchor and the head are two anchors, not one: they are
            // released one by one, and they may end up in different places.
            let anchor = self.buffer.anchor(*caret, Bias::Right);
            let head = self.buffer.anchor(*caret, Bias::Right);
            selections.push(Selection { anchor, head });
        }
        if selections.is_empty() {
            let anchor = self.buffer.anchor(TextOffset(0), Bias::Right);
            let head = self.buffer.anchor(TextOffset(0), Bias::Right);
            selections.push(Selection { anchor, head });
        }
        self.selections = selections;
    }

    /// The ranges of every selection, in the order of the text.
    pub fn selection_ranges(&self) -> Vec<Range<TextOffset>> {
        self.selections
            .iter()
            .map(|selection| selection.range(&self.buffer))
            .collect()
    }

    /// Selects every range, dropping the other selections.
    ///
    /// The ranges that cover the same text are merged, so two carets in the
    /// same place are one caret. This is what "select all the matches" does:
    /// every match becomes a selection, and editing then reaches all of them
    /// at once.
    pub fn set_selection_ranges(&mut self, ranges: &[Range<TextOffset>]) {
        self.release_selections();
        let ranges = merge_ranges(ranges.to_vec());
        let mut selections = Vec::with_capacity(ranges.len());
        for range in ranges {
            let anchor = self.buffer.anchor(range.start, Bias::Left);
            let head = self.buffer.anchor(range.end, Bias::Right);
            selections.push(Selection { anchor, head });
        }
        if selections.is_empty() {
            let anchor = self.buffer.anchor(TextOffset(0), Bias::Right);
            let head = self.buffer.anchor(TextOffset(0), Bias::Right);
            selections.push(Selection { anchor, head });
        }
        self.selections = selections;
    }

    pub fn undo(&mut self) -> Result<(), ApplyError> {
        let Some(entry) = self.undo_stack.pop() else {
            return Ok(());
        };
        let applied = self.buffer.replay(entry.transaction.clone())?;
        self.last_change = Some(applied.change.clone());
        self.set_carets(&entry.carets_to);
        self.redo_stack.push(HistoryEntry {
            transaction: applied.inverse,
            carets_from: entry.carets_to,
            carets_to: entry.carets_from,
        });
        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), ApplyError> {
        let Some(entry) = self.redo_stack.pop() else {
            return Ok(());
        };
        let applied = self.buffer.replay(entry.transaction.clone())?;
        self.last_change = Some(applied.change.clone());
        self.set_carets(&entry.carets_to);
        self.undo_stack.push(HistoryEntry {
            transaction: applied.inverse,
            carets_from: entry.carets_to,
            carets_to: entry.carets_from,
        });
        Ok(())
    }

    /// Replaces the given ranges with `replacement` and puts a caret after
    /// the text that replaced each of them.
    ///
    /// The ranges are replaced in one transaction, so the text changes in one
    /// step and the undo history has one entry for all of them.
    pub fn replace_ranges(
        &mut self,
        ranges: Vec<Range<TextOffset>>,
        replacement: &str,
    ) -> Result<(), ApplyError> {
        let ranges = merge_ranges(ranges);
        if ranges.is_empty() {
            return Ok(());
        }
        let carets_from = self.carets();

        // The ranges are sorted, so every range before a caret shifts it by
        // the size of the change that came before it.
        let mut shift = 0isize;
        let mut edits = Vec::with_capacity(ranges.len());
        let mut carets_to = Vec::with_capacity(ranges.len());
        for range in &ranges {
            edits.push(TextEdit::new(range.clone(), replacement));
            let caret = range.start.0 as isize + shift + replacement.len() as isize;
            carets_to.push(TextOffset(caret as usize));
            shift += replacement.len() as isize - (range.end.0 - range.start.0) as isize;
        }

        let version = self.buffer.version();
        let applied = self.buffer.apply(Transaction::new(version, edits))?;
        self.last_change = Some(applied.change.clone());
        // The undo step runs the edit backwards, so it starts at the carets
        // the edit produced and ends at the carets the edit started from.
        self.undo_stack.push(HistoryEntry {
            transaction: applied.inverse,
            carets_from: carets_to.clone(),
            carets_to: carets_from,
        });
        // A new edit invalidates the redo branch.
        self.redo_stack.clear();
        self.set_carets(&carets_to);
        Ok(())
    }

    /// Replaces the given ranges with their own text and keeps the carets
    /// where they were.
    ///
    /// The edits are applied in one transaction, so the text changes in one
    /// step and the undo history has one entry for all of them. This is what
    /// a formatter needs: the server answers with a list of edits, and the
    /// user undoes the whole formatting with one undo.
    pub fn replace_edits(
        &mut self,
        edits: Vec<(Range<TextOffset>, String)>,
    ) -> Result<(), ApplyError> {
        if edits.is_empty() {
            return Ok(());
        }
        // The edits are sorted, so the shift of a caret can be worked out
        // from the edits that come before it.
        let mut sorted = edits;
        sorted.sort_by_key(|(range, _)| (range.start, range.end));
        let carets_from = self.carets();

        let mut text_edits = Vec::with_capacity(sorted.len());
        let mut changes = Vec::with_capacity(sorted.len());
        for (range, text) in &sorted {
            text_edits.push(TextEdit::new(range.clone(), text));
            changes.push((range.start.0, range.end.0, text.len()));
        }

        // A caret moves with the text that came before it.
        let mut carets_to = Vec::with_capacity(carets_from.len());
        for caret in &carets_from {
            let mut moved = 0isize;
            for (start, end, length) in &changes {
                if *start <= caret.0 {
                    moved += *length as isize - (*end - *start) as isize;
                }
            }
            carets_to.push(TextOffset((caret.0 as isize + moved) as usize));
        }

        let version = self.buffer.version();
        let applied = self.buffer.apply(Transaction::new(version, text_edits))?;
        self.last_change = Some(applied.change.clone());
        // The undo step runs the edits backwards, so it starts at the carets
        // the edits produced and ends at the carets they started from.
        self.undo_stack.push(HistoryEntry {
            transaction: applied.inverse,
            carets_from: carets_to.clone(),
            carets_to: carets_from,
        });
        // A new edit invalidates the redo branch.
        self.redo_stack.clear();
        self.set_carets(&carets_to);
        Ok(())
    }

    /// Releases the anchors of every selection.
    fn release_selections(&mut self) {
        for selection in self.selections.drain(..) {
            self.buffer.release_anchor(&selection.anchor);
            self.buffer.release_anchor(&selection.head);
        }
    }
}

/// Merges the ranges that cover the same text into one.
///
/// Two carets in the same place, or a caret inside another caret's selection,
/// would make the transaction invalid: the text only has one place to put the
/// new text. Merging keeps the edit well defined, and the carets that were
/// merged end up in the same place anyway.
fn merge_ranges(ranges: Vec<Range<TextOffset>>) -> Vec<Range<TextOffset>> {
    let mut sorted = ranges;
    sorted.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<TextOffset>> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match merged.last_mut() {
            Some(last) if covers_same_text(last, &range) => {
                last.end = last.end.max(range.end);
            }
            _ => merged.push(range),
        }
    }
    merged
}

/// Whether two ranges cover the same text.
fn covers_same_text(a: &Range<TextOffset>, b: &Range<TextOffset>) -> bool {
    if a.start < a.end && b.start < b.end {
        return a.start < b.end && b.start < a.end;
    }
    // At least one of them is empty: it lies inside the other when its point
    // is inside the other range.
    let (point, range) = if a.is_empty() {
        (a.start, b)
    } else {
        (b.start, a)
    };
    range.start <= point && point <= range.end
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

    #[test]
    fn typing_at_two_carets_writes_at_both_of_them() {
        let mut editor = Editor::new("one two");
        editor.move_right();
        editor.add_cursor(TextOffset(5));
        assert_eq!(editor.cursor_count(), 2);

        editor.insert("X").unwrap();
        assert_eq!(editor.text(), "oXne tXwo");
        assert_eq!(editor.carets(), vec![TextOffset(2), TextOffset(7)]);
    }

    #[test]
    fn a_caret_in_the_same_place_as_another_is_not_added_twice() {
        let mut editor = Editor::new("one");
        editor.move_right();
        editor.add_cursor(TextOffset(1));
        assert_eq!(editor.cursor_count(), 1);
    }

    #[test]
    fn a_caret_inside_another_selection_is_merged_into_it() {
        let mut editor = Editor::new("hello");
        editor.select_range(TextOffset(1), TextOffset(4));
        editor.add_cursor(TextOffset(2));
        assert_eq!(editor.cursor_count(), 2);

        editor.insert("X").unwrap();
        assert_eq!(editor.text(), "hXo");
        assert_eq!(editor.cursor_count(), 1);
    }

    #[test]
    fn backspace_at_two_carets_deletes_at_both_of_them() {
        let mut editor = Editor::new("one two");
        editor.move_right();
        editor.add_cursor(TextOffset(5));
        editor.backspace().unwrap();
        assert_eq!(editor.text(), "ne wo");
        assert_eq!(editor.carets(), vec![TextOffset(0), TextOffset(3)]);
    }

    #[test]
    fn the_carets_are_kept_in_the_order_of_the_text() {
        let mut editor = Editor::new("one two");
        editor.add_cursor(TextOffset(4));
        editor.add_cursor(TextOffset(2));
        assert_eq!(
            editor.carets(),
            vec![TextOffset(0), TextOffset(2), TextOffset(4)]
        );
    }

    #[test]
    fn dropping_the_extra_carets_keeps_the_primary_one() {
        let mut editor = Editor::new("one two");
        editor.add_cursor(TextOffset(4));
        editor.collapse_to_primary();
        assert_eq!(editor.cursor_count(), 1);
        assert_eq!(editor.caret(), TextOffset(0));
    }

    #[test]
    fn undo_puts_every_caret_back() {
        let mut editor = Editor::new("one two");
        editor.move_right();
        editor.add_cursor(TextOffset(5));
        editor.insert("X").unwrap();
        assert_eq!(editor.text(), "oXne tXwo");
        assert_eq!(editor.carets(), vec![TextOffset(2), TextOffset(7)]);

        editor.undo().unwrap();
        assert_eq!(editor.text(), "one two");
        assert_eq!(editor.carets(), vec![TextOffset(1), TextOffset(5)]);
    }

    #[test]
    fn a_multi_caret_edit_is_one_step_of_the_history() {
        let mut editor = Editor::new("one two");
        editor.move_right();
        editor.add_cursor(TextOffset(5));
        editor.insert("X").unwrap();
        editor.undo().unwrap();
        assert_eq!(editor.text(), "one two");
        assert!(!editor.can_undo(), "the whole edit was undone at once");
    }

    #[test]
    fn moving_left_moves_every_caret() {
        let mut editor = Editor::new("one two");
        editor.move_right();
        editor.add_cursor(TextOffset(5));
        editor.move_left();
        assert_eq!(editor.carets(), vec![TextOffset(0), TextOffset(4)]);
    }

    #[test]
    fn holding_shift_extends_the_selection() {
        let mut editor = Editor::new("hello");
        editor.extend_right();
        assert_eq!(
            editor.selection().range(editor.buffer()),
            TextOffset(0)..TextOffset(1)
        );
        editor.extend_right();
        assert_eq!(
            editor.selection().range(editor.buffer()),
            TextOffset(0)..TextOffset(2)
        );
    }

    #[test]
    fn extending_to_the_left_shrinks_the_selection() {
        let mut editor = Editor::new("hello");
        editor.select_range(TextOffset(0), TextOffset(3));
        editor.extend_left();
        assert_eq!(
            editor.selection().range(editor.buffer()),
            TextOffset(0)..TextOffset(2)
        );
    }

    #[test]
    fn extending_moves_every_head() {
        let mut editor = Editor::new("one two");
        editor.move_right();
        editor.add_cursor(TextOffset(5));
        editor.extend_right();
        let ranges: Vec<_> = editor
            .selections()
            .iter()
            .map(|selection| selection.range(editor.buffer()))
            .collect();
        assert_eq!(
            ranges,
            vec![TextOffset(1)..TextOffset(2), TextOffset(5)..TextOffset(6)]
        );
    }

    #[test]
    fn delete_removes_the_character_after_the_caret() {
        let mut editor = Editor::new("hello");
        editor.delete_forward().unwrap();
        assert_eq!(editor.text(), "ello");
        assert_eq!(editor.caret(), TextOffset(0));
    }

    #[test]
    fn deleting_at_the_end_of_the_text_does_nothing() {
        let mut editor = Editor::new("x");
        editor.move_right();
        editor.delete_forward().unwrap();
        assert_eq!(editor.text(), "x");
    }
}
