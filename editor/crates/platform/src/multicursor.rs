//! The arithmetic of more than one caret.
//!
//! Two things in the editor need more than one caret, and both of them are
//! arithmetic over the text:
//!
//! - a **box**: the same columns of several lines (the column selection),
//! - **the next place the same word appears** (the word cursor).
//!
//! Both are here, away from the window, so that they can be worked out and
//! tested without a window. The window only says which way the box moves and
//! which word is looked for.

use std::ops::Range;

use codeweaver_text::{Buffer, TextOffset};

/// One corner of a box: a line and a column.
///
/// The column is counted in **characters**, not in pixels: the text is
/// monospaced, so a column is a place in the line that does not move when the
/// window is resized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Corner {
    /// The line of the corner.
    pub line: usize,
    /// How many characters are before the corner in that line.
    pub column: usize,
}

impl Corner {
    /// The corner the caret is at.
    pub fn of(buffer: &Buffer, caret: usize) -> Corner {
        let caret = caret.min(buffer.len().0);
        let line = buffer.line_of_offset(caret);
        let start = buffer.line_range(line).start.0;
        let text = buffer.line_text(line);
        let column = text
            .get(..(caret - start).min(text.len()))
            .map(|before| before.chars().count())
            .unwrap_or(0);
        Corner { line, column }
    }
}

/// Which way the corner of a box moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// One line down.
    Down,
    /// One line up.
    Up,
    /// One column to the right.
    Right,
    /// One column to the left.
    Left,
}

/// A box of text: two corners, and the lines between them.
///
/// The box starts at one corner and the other corner moves. Every line the box
/// covers gets the same columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnSelection {
    /// The corner where the box began.
    pub anchor: Corner,
    /// The corner that moves.
    pub current: Corner,
}

impl ColumnSelection {
    /// A box that is not open yet: both corners are in the same place.
    pub fn new(anchor: Corner) -> Self {
        ColumnSelection {
            anchor,
            current: anchor,
        }
    }

    /// The corner of the caret.
    pub fn of(buffer: &Buffer, caret: usize) -> Self {
        ColumnSelection::new(Corner::of(buffer, caret))
    }

    /// Moves the corner that moves one step in that direction.
    ///
    /// Returns `false` when the corner cannot go that way: the edge of the
    /// text is the end of the road.
    pub fn move_current(&mut self, direction: Direction, buffer: &Buffer) -> bool {
        match direction {
            Direction::Down => self.move_down(buffer),
            Direction::Up => self.move_up(),
            Direction::Right => self.move_right(buffer),
            Direction::Left => self.move_left(),
        }
    }

    /// Moves the corner that moves one line down.
    ///
    /// Returns `false` when the corner is already on the last line.
    pub fn move_down(&mut self, buffer: &Buffer) -> bool {
        let last = buffer.line_count().saturating_sub(1);
        if self.current.line >= last {
            return false;
        }
        self.current.line += 1;
        true
    }

    /// Moves the corner that moves one line up.
    ///
    /// Returns `false` when the corner is already on the first line.
    pub fn move_up(&mut self) -> bool {
        if self.current.line == 0 {
            return false;
        }
        self.current.line -= 1;
        true
    }

    /// Moves the corner that moves one column to the right.
    ///
    /// Returns `false` when the corner is already at the end of its line.
    pub fn move_right(&mut self, buffer: &Buffer) -> bool {
        let width = width_of_the_line(buffer, self.current.line);
        if self.current.column >= width {
            return false;
        }
        self.current.column += 1;
        true
    }

    /// Moves the corner that moves one column to the left.
    ///
    /// Returns `false` when the corner is already at the start of its line.
    pub fn move_left(&mut self) -> bool {
        if self.current.column == 0 {
            return false;
        }
        self.current.column -= 1;
        true
    }
}

/// The ranges of the lines the box covers.
///
/// Every line of the box gets the same columns: the columns between the two
/// corners. A line that is too short is cut at its end — the box does not
/// spill into the next line. A long line is cut at the column of the corner.
pub fn ranges(buffer: &Buffer, box_: &ColumnSelection) -> Vec<Range<TextOffset>> {
    let first = box_.anchor.line.min(box_.current.line);
    let last = box_.anchor.line.max(box_.current.line);
    let from = box_.anchor.column.min(box_.current.column);
    let to = box_.anchor.column.max(box_.current.column);
    let mut ranges = Vec::new();
    for line in first..=last {
        let width = width_of_the_line(buffer, line);
        let start = offset_of_the_column(buffer, line, from.min(width));
        let end = offset_of_the_column(buffer, line, to.min(width));
        ranges.push(TextOffset(start)..TextOffset(end));
    }
    ranges
}

/// How many characters the line has.
fn width_of_the_line(buffer: &Buffer, line: usize) -> usize {
    buffer.line_text(line).chars().count()
}

/// The byte offset of the column in the line.
///
/// A column that is past the end of the line is the end of the line.
fn offset_of_the_column(buffer: &Buffer, line: usize, column: usize) -> usize {
    let text = buffer.line_text(line);
    let start = buffer.line_range(line).start.0;
    for (seen, (index, _)) in text.char_indices().enumerate() {
        if seen == column {
            return start + index;
        }
    }
    start + text.len()
}

/// The next place the text appears after the given one.
///
/// The places that are already taken are skipped: the same occurrence is not
/// picked twice. The search goes round to the beginning of the document when
/// there is nothing left after the given place.
pub fn next_occurrence(
    buffer: &Buffer,
    text: &str,
    from: usize,
    taken: &[Range<usize>],
) -> Option<Range<usize>> {
    let needle = text.as_bytes();
    if needle.is_empty() {
        return None;
    }
    let len = buffer.len().0;
    let from = from.min(len);
    // First the text after the place we are at, then the text before it: the
    // search goes round the document.
    scan(buffer, needle, from, len, taken).or_else(|| scan(buffer, needle, 0, from, taken))
}

/// The first occurrence of the needle between the two places that is not taken.
fn scan(
    buffer: &Buffer,
    needle: &[u8],
    from: usize,
    to: usize,
    taken: &[Range<usize>],
) -> Option<Range<usize>> {
    let mut start = from;
    while start < to {
        let rest = buffer.slice(TextOffset(start)..TextOffset(to));
        let at = find(rest.as_bytes(), needle)?;
        let range = start + at..start + at + needle.len();
        if !taken.contains(&range) {
            return Some(range);
        }
        // This one is taken: the next one may not be.
        start = range.end;
    }
    None
}

/// The place of the needle in the haystack.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer with that text in it.
    fn buffer(text: &str) -> Buffer {
        Buffer::new(text)
    }

    #[test]
    fn the_corner_of_the_caret_is_the_line_and_the_column() {
        let buffer = buffer("one\ntwo\n");
        assert_eq!(Corner::of(&buffer, 0), Corner { line: 0, column: 0 });
        assert_eq!(Corner::of(&buffer, 2), Corner { line: 0, column: 2 });
        assert_eq!(Corner::of(&buffer, 4), Corner { line: 1, column: 0 });
    }

    #[test]
    fn a_chinese_character_is_one_column() {
        let buffer = buffer("你好世界");
        // The second character is three bytes long, but it is one column.
        assert_eq!(Corner::of(&buffer, 3), Corner { line: 0, column: 1 });
    }

    #[test]
    fn the_box_takes_the_same_columns_on_every_line() {
        let buffer = buffer("abcd\nefgh\nijkl\n");
        let box_ = ColumnSelection {
            anchor: Corner { line: 0, column: 1 },
            current: Corner { line: 2, column: 3 },
        };
        let ranges = ranges(&buffer, &box_);
        assert_eq!(ranges.len(), 3, "the box covers three lines");
        // Every line starts after the line break of the one before it.
        assert_eq!(ranges[0], TextOffset(1)..TextOffset(3));
        assert_eq!(ranges[1], TextOffset(6)..TextOffset(8));
        assert_eq!(ranges[2], TextOffset(11)..TextOffset(13));
    }

    #[test]
    fn the_box_can_go_up_as_well_as_down() {
        let buffer = buffer("abcd\nefgh\n");
        let box_ = ColumnSelection {
            anchor: Corner { line: 2, column: 0 },
            current: Corner { line: 0, column: 0 },
        };
        let ranges = ranges(&buffer, &box_);
        assert_eq!(ranges.len(), 3, "the box covers three lines");
        assert_eq!(ranges[0], TextOffset(0)..TextOffset(0));
        assert_eq!(ranges[2], TextOffset(10)..TextOffset(10));
    }

    #[test]
    fn a_short_line_is_cut_at_its_end() {
        let buffer = buffer("abcd\nef\nijkl\n");
        let box_ = ColumnSelection {
            anchor: Corner { line: 0, column: 1 },
            current: Corner { line: 2, column: 3 },
        };
        let ranges = ranges(&buffer, &box_);
        // The second line is too short: the box stops at its end and does not
        // spill into the next line.
        assert_eq!(ranges[1], TextOffset(6)..TextOffset(7));
        assert_eq!(ranges[2], TextOffset(9)..TextOffset(11));
    }

    #[test]
    fn a_long_line_is_cut_at_the_column() {
        let buffer = buffer("abcdefgh\n");
        let box_ = ColumnSelection {
            anchor: Corner { line: 0, column: 1 },
            current: Corner { line: 0, column: 3 },
        };
        assert_eq!(ranges(&buffer, &box_), vec![TextOffset(1)..TextOffset(3)]);
    }

    #[test]
    fn the_corner_moves_down_until_the_last_line() {
        let buffer = buffer("one\ntwo\nthree");
        let mut box_ = ColumnSelection::new(Corner { line: 0, column: 0 });
        assert!(box_.move_down(&buffer));
        assert_eq!(box_.current.line, 1);
        assert!(box_.move_down(&buffer));
        assert_eq!(box_.current.line, 2);
        assert!(!box_.move_down(&buffer), "the last line is the end");
        assert_eq!(box_.current.line, 2);
    }

    #[test]
    fn the_line_after_the_last_break_is_a_line_of_its_own() {
        // A document that ends with a line break has an empty last line: the
        // caret can stand there, so the box can reach it.
        let buffer = buffer("one\ntwo\n");
        let mut box_ = ColumnSelection::new(Corner { line: 0, column: 0 });
        assert!(box_.move_down(&buffer));
        assert!(box_.move_down(&buffer));
        assert_eq!(box_.current.line, 2);
        assert!(!box_.move_down(&buffer));
    }

    #[test]
    fn the_corner_moves_up_until_the_first_line() {
        let mut box_ = ColumnSelection::new(Corner { line: 2, column: 0 });
        assert!(box_.move_up());
        assert_eq!(box_.current.line, 1);
        assert!(box_.move_up());
        assert_eq!(box_.current.line, 0);
        assert!(!box_.move_up(), "the first line is the end");
        assert_eq!(box_.current.line, 0);
    }

    #[test]
    fn the_corner_moves_sideways_until_the_edge_of_the_line() {
        let buffer = buffer("abc\n");
        let mut box_ = ColumnSelection::new(Corner { line: 0, column: 0 });
        assert!(box_.move_right(&buffer));
        assert_eq!(box_.current.column, 1);
        assert!(box_.move_right(&buffer));
        assert_eq!(box_.current.column, 2);
        // The caret may stand at the end of the line.
        assert!(box_.move_right(&buffer));
        assert_eq!(box_.current.column, 3);
        assert!(!box_.move_right(&buffer), "the end of the line is the end");
        assert_eq!(box_.current.column, 3);
        assert!(box_.move_left());
        assert_eq!(box_.current.column, 2);
        assert!(box_.move_left());
        assert_eq!(box_.current.column, 1);
        assert!(box_.move_left());
        assert_eq!(box_.current.column, 0);
        assert!(!box_.move_left(), "the start of the line is the end");
    }

    #[test]
    fn the_next_occurrence_is_found_after_the_place_we_are_at() {
        let buffer = buffer("one two one two one");
        let found = next_occurrence(&buffer, "one", 0, &[]);
        assert_eq!(found, Some(0..3));
        let found = next_occurrence(&buffer, "one", 1, &[]);
        assert_eq!(found, Some(8..11));
    }

    #[test]
    fn the_next_occurrence_goes_round_the_document() {
        let buffer = buffer("one two one");
        // The last one is behind us: the search goes round to the first.
        let taken = [Range { start: 0, end: 3 }];
        let found = next_occurrence(&buffer, "one", 11, &taken);
        assert_eq!(found, Some(8..11), "the one that is left");
    }

    #[test]
    fn a_place_that_is_already_taken_is_not_picked_again() {
        let buffer = buffer("one two one");
        let taken = [Range { start: 0, end: 3 }];
        let found = next_occurrence(&buffer, "one", 0, &taken);
        assert_eq!(found, Some(8..11));
    }

    #[test]
    fn there_is_no_next_occurrence_when_the_word_is_not_there() {
        let buffer = buffer("one two");
        assert_eq!(next_occurrence(&buffer, "three", 0, &[]), None);
        assert_eq!(next_occurrence(&buffer, "", 0, &[]), None);
    }
}
