//! Stable positions that survive edits.
//!
//! A [`TextOffset`] is only valid for the exact text it was measured against.
//! Long-lived references — LSP selections, code actions, Agent patches, folded
//! ranges — must instead use an [`Anchor`], which the buffer re-resolves after
//! every transaction.
//!
//! The semantics follow the bias convention used by Zed and VS Code:
//!
//! - `Bias::Left` keeps the anchor *before* text inserted at the same position.
//! - `Bias::Right` keeps the anchor *after* text inserted at the same position.
//!
//! When the text an anchor points at is deleted, the anchor collapses to the
//! start of the deleted range. An anchor that points at the character *after*
//! a replaced range follows that character to its new position.

use std::fmt;

use crate::TextOffset;

/// Which side of an insertion an anchor attaches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bias {
    /// The anchor stays before text inserted at its position.
    Left,
    /// The anchor stays after text inserted at its position.
    Right,
}

impl Bias {
    pub fn opposite(self) -> Self {
        match self {
            Bias::Left => Bias::Right,
            Bias::Right => Bias::Left,
        }
    }
}

/// A stable reference to a position inside a buffer.
///
/// Anchors are cheap to copy and are resolved against the live buffer with
/// [`Buffer::resolve_anchor`](crate::Buffer::resolve_anchor). The buffer owns
/// the anchor table; anchors are created with
/// [`Buffer::anchor`](crate::Buffer::anchor).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub(crate) id: AnchorId,
    pub(crate) bias: Bias,
}

/// Identifier of an anchor inside a buffer's anchor table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnchorId(pub u64);

impl fmt::Display for AnchorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "anchor#{}", self.0)
    }
}

/// A range delimited by two anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorRange {
    pub start: Anchor,
    pub end: Anchor,
}

/// Internal per-anchor bookkeeping.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AnchorState {
    /// Current byte offset of the anchor.
    pub offset: TextOffset,
    /// The bias the anchor was created with.
    pub bias: Bias,
}

impl AnchorState {
    /// Applies a replacement of `range` with `replacement_len` bytes.
    ///
    /// `range` is the byte range that was replaced, expressed in coordinates
    /// *before* the replacement.
    pub(crate) fn apply_edit(&mut self, range: std::ops::Range<usize>, replacement_len: usize) {
        let start = range.start;
        let end = range.end;
        let removed = end - start;
        let delta = replacement_len as isize - removed as isize;

        if self.offset.0 < start {
            // Entirely before the replaced range.
            return;
        }
        if self.offset.0 > end {
            // Entirely after the replaced range: shift by the size delta.
            self.offset = TextOffset((self.offset.0 as isize + delta) as usize);
            return;
        }
        if removed > 0 && self.offset.0 == end {
            // Points at the character right after the replaced range. That
            // character survives, so the anchor follows it.
            self.offset = TextOffset(start + replacement_len);
            return;
        }
        if removed > 0 {
            // Points into the replaced range: collapse to its start.
            self.offset = TextOffset(start);
            return;
        }
        // Pure insertion at the anchor's position: the bias decides which side
        // of the inserted text the anchor lands on.
        self.offset = TextOffset(match self.bias {
            Bias::Left => start,
            Bias::Right => start + replacement_len,
        });
    }
}
