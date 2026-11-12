//! Where a place in the document is, in the protocol's own words.
//!
//! The protocol counts lines from zero and columns in **UTF-16 code units**:
//! a character outside the basic plane counts as two. The editor counts
//! bytes, so every place that comes from or goes to the server has to be
//! translated.

use codeweaver_text::{Buffer, TextOffset};

use crate::protocol::Position;

/// The place of a byte offset in the document.
pub fn position_at(buffer: &Buffer, offset: usize) -> Position {
    let line = buffer.line_of_offset(offset);
    let start = buffer.line_range(line).start.0;
    let text = buffer.slice(TextOffset(start)..TextOffset(offset));
    let character = text.chars().map(|c| c.len_utf16() as u32).sum();
    Position::new(line as u32, character)
}

/// The byte offset of a place in the document.
///
/// A column past the end of the line lands at the end of the line: the
/// server sometimes asks about a place that is not there.
pub fn offset_at(buffer: &Buffer, position: Position) -> Option<usize> {
    let line = position.line as usize;
    let text = buffer.line_text(line);
    let mut units = 0u32;
    for (index, character) in text.char_indices() {
        if units >= position.character {
            return Some(buffer.line_range(line).start.0 + index);
        }
        units += character.len_utf16() as u32;
    }
    Some(buffer.line_range(line).start.0 + text.len())
}

/// The place that comes after `text`, when the text starts at `place`.
pub fn advance(place: Position, text: &str) -> Position {
    let mut line = place.line;
    let mut character = place.character;
    for c in text.chars() {
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += c.len_utf16() as u32;
        }
    }
    Position::new(line, character)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_byte_offset_becomes_a_place() {
        let buffer = Buffer::new("fn main() {\n    let x = 1;\n}\n");
        assert_eq!(position_at(&buffer, 0), Position::new(0, 0));
        assert_eq!(position_at(&buffer, 3), Position::new(0, 3));
        // The start of the second line.
        assert_eq!(position_at(&buffer, 12), Position::new(1, 0));
        assert_eq!(position_at(&buffer, 16), Position::new(1, 4));
    }

    #[test]
    fn a_place_becomes_a_byte_offset() {
        let buffer = Buffer::new("fn main() {\n    let x = 1;\n}\n");
        assert_eq!(offset_at(&buffer, Position::new(0, 0)), Some(0));
        assert_eq!(offset_at(&buffer, Position::new(0, 3)), Some(3));
        assert_eq!(offset_at(&buffer, Position::new(1, 4)), Some(16));
    }

    #[test]
    fn a_character_outside_the_basic_plane_counts_as_two() {
        let buffer = Buffer::new("let x = \"😀\";\n");
        // The emoji is four bytes and two UTF-16 code units.
        assert_eq!(position_at(&buffer, 12), Position::new(0, 12));
        assert_eq!(offset_at(&buffer, Position::new(0, 12)), Some(12));
    }

    #[test]
    fn a_column_past_the_end_of_the_line_lands_at_the_end() {
        let buffer = Buffer::new("abc\ndef\n");
        assert_eq!(offset_at(&buffer, Position::new(0, 99)), Some(3));
    }

    #[test]
    fn a_place_advances_over_the_text() {
        assert_eq!(advance(Position::new(2, 4), "abc"), Position::new(2, 7));
        assert_eq!(advance(Position::new(2, 4), "a\nb"), Position::new(3, 1));
    }
}
