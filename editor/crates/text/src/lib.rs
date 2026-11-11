//! Text model for the CodeWeaver editor.
//!
//! [`Buffer`] is the single source of truth for a document's text. It is
//! backed by a [`ropey::Rope`] so that large files stay cheap to edit, and it
//! exposes:
//!
//! - an immutable [`BufferSnapshot`] for readers that need a stable view;
//! - versioned [`Transaction`]s so stale writes are rejected;
//! - [`Anchor`]s, which are re-resolved after every transaction so that
//!   long-lived references survive edits.

mod anchor;

pub use anchor::{Anchor, AnchorId, AnchorRange, Bias};

use std::{collections::BTreeMap, ops::Range, sync::Arc};

use ropey::Rope;
use thiserror::Error;

pub type BufferVersion = u64;

/// A byte offset into the buffer text.
///
/// Offsets are always UTF-8 boundary aligned; the buffer rejects any edit that
/// would split a scalar value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextOffset(pub usize);

impl TextOffset {
    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// A single replacement inside a transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<TextOffset>,
    pub replacement: String,
}

impl TextEdit {
    pub fn new(range: Range<TextOffset>, replacement: impl Into<String>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
        }
    }
}

/// A versioned, atomic set of edits.
///
/// The base version is checked against the buffer when the transaction is
/// applied, so a transaction computed against an outdated view of the text is
/// rejected instead of silently corrupting the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub base_version: BufferVersion,
    pub edits: Vec<TextEdit>,
}

impl Transaction {
    pub fn new(base_version: BufferVersion, edits: Vec<TextEdit>) -> Self {
        Self {
            base_version,
            edits,
        }
    }
}

/// An immutable view of the buffer at a specific version.
///
/// The snapshot owns its text, so it stays valid no matter how the live buffer
/// changes afterwards.
#[derive(Clone, Debug)]
pub struct BufferSnapshot {
    text: Arc<str>,
    version: BufferVersion,
}

impl BufferSnapshot {
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn version(&self) -> BufferVersion {
        self.version
    }
    pub fn len(&self) -> TextOffset {
        TextOffset(self.text.len())
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// The outcome of applying a transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedTransaction {
    /// The transaction that undoes the applied one.
    pub inverse: Transaction,
    /// The buffer version after the transaction was applied.
    pub version: BufferVersion,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ApplyError {
    #[error("transaction is based on version {actual}, but the buffer is at {expected}")]
    StaleVersion {
        expected: BufferVersion,
        actual: BufferVersion,
    },
    #[error("edit range {start}..{end} is outside the {len}-byte buffer")]
    OutOfBounds {
        start: usize,
        end: usize,
        len: usize,
    },
    #[error("edit range {start}..{end} does not fall on UTF-8 boundaries")]
    InvalidBoundary { start: usize, end: usize },
    #[error("edits overlap or are not sorted")]
    OverlappingEdits,
}

/// The text buffer.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    rope: Rope,
    version: BufferVersion,
    /// Anchor table: anchor id -> current state.
    anchors: BTreeMap<AnchorId, anchor::AnchorState>,
    next_anchor_id: u64,
}

impl Buffer {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            rope: Rope::from_str(&text.into()),
            version: 0,
            anchors: BTreeMap::new(),
            next_anchor_id: 0,
        }
    }

    /// The full text of the buffer.
    ///
    /// This materialises the whole document; prefer [`Buffer::slice`] when only
    /// a region is needed.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// The text in a byte range.
    pub fn slice(&self, range: Range<TextOffset>) -> String {
        self.rope
            .slice(self.char_index(range.start.0)..self.char_index(range.end.0))
            .to_string()
    }

    /// Total length in bytes.
    pub fn len(&self) -> TextOffset {
        TextOffset(self.rope.len_bytes())
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    /// Number of lines in the buffer.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// An immutable view of the current text.
    pub fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            text: Arc::from(self.rope.to_string().as_str()),
            version: self.version,
        }
    }

    /// Creates an anchor that tracks `offset` across later edits.
    ///
    /// `bias` decides which side of an insertion at the same position the
    /// anchor ends up on.
    pub fn anchor(&mut self, offset: TextOffset, bias: Bias) -> Anchor {
        let id = AnchorId(self.next_anchor_id);
        self.next_anchor_id += 1;
        let offset = TextOffset(offset.0.min(self.rope.len_bytes()));
        self.anchors
            .insert(id, anchor::AnchorState { offset, bias });
        Anchor { id, bias }
    }

    /// Resolves an anchor to its current byte offset.
    ///
    /// Returns `None` if the anchor does not belong to this buffer.
    pub fn resolve_anchor(&self, anchor: &Anchor) -> Option<TextOffset> {
        self.anchors.get(&anchor.id).map(|state| state.offset)
    }

    /// Removes an anchor from the table.
    pub fn release_anchor(&mut self, anchor: &Anchor) {
        self.anchors.remove(&anchor.id);
    }

    /// Number of live anchors.
    pub fn anchor_count(&self) -> usize {
        self.anchors.len()
    }

    /// Applies a transaction to the buffer.
    ///
    /// The transaction's base version must match the buffer's current version.
    /// This is the path external writers take — LSP edits, Agent patches,
    /// formatting — so that a write computed against an outdated view of the
    /// text is rejected instead of corrupting the document.
    pub fn apply(&mut self, transaction: Transaction) -> Result<AppliedTransaction, ApplyError> {
        if transaction.base_version != self.version {
            return Err(ApplyError::StaleVersion {
                expected: self.version,
                actual: transaction.base_version,
            });
        }
        self.apply_edits(transaction)
    }

    /// Replays a transaction that this buffer recorded in its own history.
    ///
    /// History replay skips the base-version check, because the inverse of an
    /// edit is expressed in the coordinate space of the state that edit
    /// produced, while the buffer's version counter keeps advancing as the
    /// history is walked backwards and forwards.
    ///
    /// This is safe only while the recorded edits still describe the current
    /// text, which holds when the history is walked in strict LIFO order and
    /// every write to the buffer is recorded in that same history. Callers
    /// must not use this path for writes that come from outside the editor.
    pub fn replay(&mut self, transaction: Transaction) -> Result<AppliedTransaction, ApplyError> {
        self.apply_edits(transaction)
    }

    fn apply_edits(&mut self, transaction: Transaction) -> Result<AppliedTransaction, ApplyError> {
        self.validate_edits(&transaction.edits)?;

        // Compute the inverse against the pre-edit text, then apply the edits
        // back to front so earlier offsets stay valid.
        let mut inverse_edits = Vec::with_capacity(transaction.edits.len());
        let mut shift: isize = 0;
        for edit in &transaction.edits {
            let old = self
                .rope
                .slice(self.char_index(edit.range.start.0)..self.char_index(edit.range.end.0))
                .to_string();
            let new_start = edit
                .range
                .start
                .0
                .checked_add_signed(shift)
                .expect("validated offset");
            inverse_edits.push(TextEdit::new(
                TextOffset(new_start)..TextOffset(new_start + edit.replacement.len()),
                old,
            ));
            shift +=
                edit.replacement.len() as isize - (edit.range.end.0 - edit.range.start.0) as isize;
        }

        for edit in transaction.edits.iter().rev() {
            let start_char = self.char_index(edit.range.start.0);
            let end_char = self.char_index(edit.range.end.0);
            self.rope.remove(start_char..end_char);
            if !edit.replacement.is_empty() {
                self.rope.insert(start_char, &edit.replacement);
            }
        }

        for state in self.anchors.values_mut() {
            for edit in transaction.edits.iter().rev() {
                state.apply_edit(edit.range.start.0..edit.range.end.0, edit.replacement.len());
            }
        }

        self.version += 1;
        Ok(AppliedTransaction {
            inverse: Transaction::new(self.version, inverse_edits),
            version: self.version,
        })
    }

    fn validate_edits(&self, edits: &[TextEdit]) -> Result<(), ApplyError> {
        let mut previous_end = 0;
        for (index, edit) in edits.iter().enumerate() {
            let start = edit.range.start.0;
            let end = edit.range.end.0;
            if start > end || end > self.rope.len_bytes() {
                return Err(ApplyError::OutOfBounds {
                    start,
                    end,
                    len: self.rope.len_bytes(),
                });
            }
            if !self.is_char_boundary(start) || !self.is_char_boundary(end) {
                return Err(ApplyError::InvalidBoundary { start, end });
            }
            if index > 0 && start < previous_end {
                return Err(ApplyError::OverlappingEdits);
            }
            previous_end = end;
        }
        Ok(())
    }

    /// Whether `offset` sits on a UTF-8 scalar boundary.
    pub fn is_char_boundary(&self, offset: usize) -> bool {
        if offset == 0 || offset == self.rope.len_bytes() {
            return true;
        }
        // `ropey` maps a byte inside a multi-byte scalar to the scalar that
        // contains it, so a round trip through the char index tells us
        // whether the byte was a boundary.
        self.char_to_byte(self.char_index(offset)) == offset
    }

    /// Converts a byte offset into a char index.
    fn char_index(&self, byte_offset: usize) -> usize {
        self.rope.byte_to_char(byte_offset)
    }

    /// Converts a char index into a byte offset.
    fn char_to_byte(&self, char_index: usize) -> usize {
        self.rope.char_to_byte(char_index)
    }

    /// The byte offset of the character boundary before `offset`.
    ///
    /// Returns `offset` unchanged when it is already at the start of the text.
    pub fn prev_char_boundary(&self, offset: TextOffset) -> TextOffset {
        if offset.0 == 0 {
            return offset;
        }
        let char_index = self.char_index(offset.0);
        if char_index == 0 {
            return TextOffset(0);
        }
        TextOffset(self.char_to_byte(char_index - 1))
    }

    /// The byte offset of the character boundary after `offset`.
    ///
    /// Returns the end of the text when `offset` is already at the end.
    pub fn next_char_boundary(&self, offset: TextOffset) -> TextOffset {
        if offset.0 >= self.rope.len_bytes() {
            return TextOffset(self.rope.len_bytes());
        }
        let char_index = self.char_index(offset.0);
        TextOffset(self.char_to_byte(char_index + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_edits_generate_a_working_inverse() {
        let mut buffer = Buffer::new("A中文Z");
        let applied = buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(TextOffset(1)..TextOffset(7), "Rust")],
            ))
            .unwrap();
        assert_eq!(buffer.text(), "ARustZ");
        buffer.apply(applied.inverse).unwrap();
        assert_eq!(buffer.text(), "A中文Z");
    }

    #[test]
    fn rejects_offsets_inside_a_unicode_scalar() {
        let mut buffer = Buffer::new("中文");
        let error = buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(TextOffset(1)..TextOffset(2), "x")],
            ))
            .unwrap_err();
        assert!(matches!(error, ApplyError::InvalidBoundary { .. }));
    }

    #[test]
    fn rejects_a_stale_transaction() {
        let mut buffer = Buffer::new("abc");
        buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(TextOffset(3)..TextOffset(3), "d")],
            ))
            .unwrap();
        assert!(matches!(
            buffer.apply(Transaction::new(0, vec![])),
            Err(ApplyError::StaleVersion { .. })
        ));
    }

    #[test]
    fn anchor_follows_insertions_before_it() {
        let mut buffer = Buffer::new("world");
        let anchor = buffer.anchor(TextOffset(0), Bias::Right);
        buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(TextOffset(0)..TextOffset(0), "hello ")],
            ))
            .unwrap();
        assert_eq!(buffer.resolve_anchor(&anchor), Some(TextOffset(6)));
    }

    #[test]
    fn bias_decides_which_side_of_an_insertion_an_anchor_lands_on() {
        let mut buffer = Buffer::new("world");
        let left = buffer.anchor(TextOffset(0), Bias::Left);
        let right = buffer.anchor(TextOffset(0), Bias::Right);
        buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(TextOffset(0)..TextOffset(0), "hello ")],
            ))
            .unwrap();
        assert_eq!(buffer.resolve_anchor(&left), Some(TextOffset(0)));
        assert_eq!(buffer.resolve_anchor(&right), Some(TextOffset(6)));
    }

    #[test]
    fn anchor_collapses_to_the_start_of_a_deleted_range() {
        let mut buffer = Buffer::new("hello world");
        let anchor = buffer.anchor(TextOffset(8), Bias::Right);
        buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(TextOffset(6)..TextOffset(11), "there")],
            ))
            .unwrap();
        assert_eq!(buffer.resolve_anchor(&anchor), Some(TextOffset(6)));
    }

    #[test]
    fn anchor_survives_an_edit_behind_it() {
        let mut buffer = Buffer::new("0123456789");
        let anchor = buffer.anchor(TextOffset(8), Bias::Right);
        buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(TextOffset(0)..TextOffset(2), "xx")],
            ))
            .unwrap();
        assert_eq!(buffer.resolve_anchor(&anchor), Some(TextOffset(8)));
    }

    #[test]
    fn snapshot_stays_valid_after_further_edits() {
        let mut buffer = Buffer::new("first");
        let snapshot = buffer.snapshot();
        buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(TextOffset(0)..TextOffset(0), "second ")],
            ))
            .unwrap();
        assert_eq!(snapshot.text(), "first");
        assert_eq!(snapshot.version(), 0);
        assert_eq!(buffer.text(), "second first");
    }

    #[test]
    fn rope_preserves_large_text() {
        let mut buffer = Buffer::new("a".repeat(100_000));
        assert_eq!(buffer.len(), TextOffset(100_000));
        assert_eq!(buffer.line_count(), 1);
        buffer
            .apply(Transaction::new(
                0,
                vec![TextEdit::new(
                    TextOffset(50_000)..TextOffset(50_000),
                    "middle",
                )],
            ))
            .unwrap();
        assert_eq!(buffer.len(), TextOffset(100_006));
        assert_eq!(
            &buffer.slice(TextOffset(49_995)..TextOffset(50_012)),
            "aaaaamiddleaaaaaa"
        );
    }
}
