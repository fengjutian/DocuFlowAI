use std::{ops::Range, sync::Arc};

use thiserror::Error;

pub type BufferVersion = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextOffset(pub usize);

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
}

#[derive(Clone, Debug, Default)]
pub struct Buffer {
    text: String,
    version: BufferVersion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedTransaction {
    pub inverse: Transaction,
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

impl Buffer {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            version: 0,
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn len(&self) -> TextOffset {
        TextOffset(self.text.len())
    }
    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            text: Arc::from(self.text.as_str()),
            version: self.version,
        }
    }

    pub fn apply(&mut self, transaction: Transaction) -> Result<AppliedTransaction, ApplyError> {
        if transaction.base_version != self.version {
            return Err(ApplyError::StaleVersion {
                expected: self.version,
                actual: transaction.base_version,
            });
        }
        self.validate_edits(&transaction.edits)?;

        let mut inverse_edits = Vec::with_capacity(transaction.edits.len());
        let mut shift: isize = 0;
        for edit in &transaction.edits {
            let old = self.text[edit.range.start.0..edit.range.end.0].to_owned();
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
            self.text
                .replace_range(edit.range.start.0..edit.range.end.0, &edit.replacement);
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
            if start > end || end > self.text.len() {
                return Err(ApplyError::OutOfBounds {
                    start,
                    end,
                    len: self.text.len(),
                });
            }
            if !self.text.is_char_boundary(start) || !self.text.is_char_boundary(end) {
                return Err(ApplyError::InvalidBoundary { start, end });
            }
            if index > 0 && start < previous_end {
                return Err(ApplyError::OverlappingEdits);
            }
            previous_end = end;
        }
        Ok(())
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
}
