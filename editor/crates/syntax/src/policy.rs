//! When the highlighter gives up.
//!
//! Highlighting is the one thing in the editor that costs more as the file
//! grows: the parse is linear in the size of the document, and a file that
//! somebody generated — a lock file, a bundle, a dump — can be big enough that
//! parsing it costs more than a frame is allowed to take.
//!
//! The policy is the line between "highlight this" and "do not highlight
//! this". It is deliberately a *policy* and not a constant: the threshold is a
//! product decision (how big a file may be before the editor stops colouring
//! it), and the editor states that decision out loud instead of silently
//! showing plain text.

/// The sizes at which the highlighter gives up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HighlightPolicy {
    /// Documents bigger than this many bytes are not highlighted.
    pub max_bytes: usize,
    /// Documents with more lines than this are not highlighted.
    pub max_lines: usize,
}

impl Default for HighlightPolicy {
    /// The default limits.
    ///
    /// A megabyte of source is already far past what a person reads in one
    /// sitting, and fifty thousand lines is about a very large source file.
    /// Both limits exist to keep the *frame* budget, not to keep the disk
    /// small.
    fn default() -> Self {
        HighlightPolicy {
            max_bytes: 1_000_000,
            max_lines: 50_000,
        }
    }
}

impl HighlightPolicy {
    /// A policy that never gives up.
    ///
    /// Useful for a benchmark that wants to measure the cost of highlighting a
    /// large file rather than the cost of refusing to.
    pub fn unlimited() -> Self {
        HighlightPolicy {
            max_bytes: usize::MAX,
            max_lines: usize::MAX,
        }
    }

    /// Whether a document of this size is highlighted.
    ///
    /// Both limits have to hold: a file that is short but wide (a single line
    /// of a hundred megabytes) is as expensive as a long one.
    pub fn allows(&self, bytes: usize, lines: usize) -> bool {
        bytes <= self.max_bytes && lines <= self.max_lines
    }
}
