//! The text layout: which characters are where on screen.
//!
//! The layout turns a buffer into the visual lines that fit in a viewport.
//! A logical line (one line of the file) becomes one or more *visual* lines
//! when it is longer than the viewport is wide.
//!
//! The layout is built for a viewport, not for the whole document: measuring
//! a hundred megabytes to draw forty lines would be absurd.

use std::ops::Range;

use codeweaver_text::Buffer;

use crate::measure::{LineMetrics, TextMeasurer};

/// One visual line: a piece of a logical line that fits the viewport width.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualLine {
    /// The byte range of this visual line in the document.
    pub range: Range<usize>,
    /// The text of the visual line.
    pub text: String,
    /// The x offset of every byte boundary, relative to the line's start.
    pub boundaries: Vec<f32>,
    /// The y offset of the line, in pixels from the top of the layout.
    pub y: f32,
}

impl VisualLine {
    /// The x offset of a byte offset within this line.
    pub fn offset_at(&self, byte_offset: usize) -> f32 {
        let index = byte_offset.saturating_sub(self.range.start);
        self.boundaries
            .get(index)
            .copied()
            .unwrap_or_else(|| self.boundaries.last().copied().unwrap_or(0.0))
    }

    /// The byte offsets of the line's characters, in order.
    fn char_boundaries(&self) -> Vec<usize> {
        let mut offsets = vec![self.range.start];
        offsets.extend(
            self.text
                .char_indices()
                .skip(1)
                .map(|(index, _)| self.range.start + index),
        );
        offsets.push(self.range.end);
        offsets
    }
}

/// The visual lines of a viewport.
#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    /// The visual lines, top to bottom.
    pub lines: Vec<VisualLine>,
    /// The height of one line.
    pub line_height: f32,
    /// The font size the lines were measured at.
    pub font_size: f32,
}

impl TextLayout {
    /// Builds the layout of the viewport.
    ///
    /// `first_line` is the first logical line to show, `max_lines` how many
    /// visual lines fit, and `width` how wide they may be.
    pub fn build(
        buffer: &Buffer,
        first_line: usize,
        max_lines: usize,
        width: f32,
        font_size: f32,
        line_height: f32,
        measurer: &mut dyn TextMeasurer,
    ) -> Self {
        let mut layout = TextLayout {
            lines: Vec::new(),
            line_height,
            font_size,
        };
        if max_lines == 0 || width <= 0.0 {
            return layout;
        }

        let total_lines = buffer.line_count();
        let mut logical_line = first_line.min(total_lines.saturating_sub(1));

        while layout.lines.len() < max_lines && logical_line < total_lines {
            let text = buffer.line_text(logical_line);
            let line_range = buffer.line_range(logical_line);
            let metrics = measurer.measure(&text, font_size);

            for (segment, bounds) in wrap(&text, &metrics, width) {
                let start = line_range.start.0 + segment.start;
                let end = line_range.start.0 + segment.end;
                layout.lines.push(VisualLine {
                    range: start..end,
                    text: text[segment.clone()].to_owned(),
                    boundaries: bounds,
                    y: layout.lines.len() as f32 * line_height,
                });
                if layout.lines.len() >= max_lines {
                    break;
                }
            }
            logical_line += 1;
        }

        layout
    }

    /// The total height of the layout.
    pub fn height(&self) -> f32 {
        self.lines.len() as f32 * self.line_height
    }

    /// The visual line that contains a byte offset.
    pub fn line_of(&self, offset: usize) -> Option<&VisualLine> {
        self.lines
            .iter()
            .find(|line| offset >= line.range.start && offset <= line.range.end)
    }

    /// The index of the visual line that contains a byte offset.
    pub fn line_index_of(&self, offset: usize) -> Option<usize> {
        self.lines
            .iter()
            .position(|line| offset >= line.range.start && offset <= line.range.end)
    }

    /// The point of a byte offset, relative to the layout's top left corner.
    ///
    /// Returns `None` when the offset is outside the layout — for example
    /// when the document was edited and the offset no longer exists.
    pub fn offset_point(&self, offset: usize) -> Option<(f32, f32)> {
        let line = self.line_of(offset)?;
        Some((line.offset_at(offset), line.y))
    }

    /// The byte offset at a point, relative to the layout's top left corner.
    ///
    /// The offset snaps to the nearest character boundary, the way a click
    /// between two characters picks the closer one.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }

        let line_index = ((y / self.line_height).floor() as isize)
            .clamp(0, self.lines.len() as isize - 1) as usize;
        let line = &self.lines[line_index];

        // The character boundaries of the line, with their x offsets.
        let boundaries = line.char_boundaries();
        let mut best = boundaries[0];
        let mut best_distance = f32::INFINITY;
        for offset in boundaries {
            let distance = (line.offset_at(offset) - x).abs();
            // A click exactly between two characters goes to the later one,
            // so the caret never ends up on the wrong side of a glyph.
            if distance <= best_distance {
                best_distance = distance;
                best = offset;
            }
        }
        Some(best)
    }
}

/// Splits a logical line into the pieces that fit the width.
///
/// Returns the byte ranges of the pieces, together with the x offsets of
/// their byte boundaries relative to the piece's own start.
fn wrap(text: &str, metrics: &LineMetrics, width: f32) -> Vec<(Range<usize>, Vec<f32>)> {
    if text.is_empty() {
        return vec![(0..0, vec![0.0])];
    }
    if metrics.width <= width {
        return vec![(0..text.len(), metrics.boundaries.clone())];
    }

    let mut pieces = Vec::new();
    let mut segment_start = 0usize;

    // Walk the characters, cutting whenever the next one would not fit.
    for (index, character) in text.char_indices() {
        let end = index + character.len_utf8();
        // The width is measured from the start of the piece, not from the
        // start of the line: a piece always begins at the left edge.
        let used = metrics.offset_at(end) - metrics.offset_at(segment_start);
        if used > width && index > segment_start {
            pieces.push((
                segment_start..index,
                relative(&metrics.boundaries, segment_start, index),
            ));
            segment_start = index;
        }
    }
    pieces.push((
        segment_start..text.len(),
        relative(&metrics.boundaries, segment_start, text.len()),
    ));

    pieces
}

/// The x offsets of the byte boundaries `start..=end`, relative to `start`.
fn relative(boundaries: &[f32], start: usize, end: usize) -> Vec<f32> {
    let base = boundaries.get(start).copied().unwrap_or(0.0);
    boundaries[start..=end]
        .iter()
        .map(|offset| offset - base)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::MonospaceMeasurer;

    fn layout_of(text: &str, width: f32, max_lines: usize) -> TextLayout {
        let buffer = Buffer::new(text);
        let mut measurer = MonospaceMeasurer::new(10.0);
        TextLayout::build(&buffer, 0, max_lines, width, 16.0, 20.0, &mut measurer)
    }

    #[test]
    fn a_short_document_is_one_line_per_logical_line() {
        let layout = layout_of("first\nsecond\n", 1000.0, 10);
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(layout.lines[0].text, "first");
        assert_eq!(layout.lines[1].text, "second");
        assert_eq!(layout.lines[2].text, "");
        assert_eq!(layout.lines[1].y, 20.0);
    }

    #[test]
    fn a_long_line_wraps_at_the_width() {
        // Ten characters at ten pixels each: only five fit in a 50 pixel row.
        let layout = layout_of("abcdefghij", 50.0, 10);
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(layout.lines[0].text, "abcde");
        assert_eq!(layout.lines[1].text, "fghij");
        assert_eq!(layout.lines[1].y, 20.0);
    }

    #[test]
    fn the_viewport_limits_how_much_is_built() {
        let layout = layout_of("one\ntwo\nthree\nfour\n", 1000.0, 2);
        assert_eq!(layout.lines.len(), 2);
        assert_eq!(layout.lines[0].text, "one");
        assert_eq!(layout.lines[1].text, "two");
    }

    #[test]
    fn a_hit_test_finds_the_closest_character() {
        let layout = layout_of("abc", 1000.0, 10);
        assert_eq!(layout.hit_test(0.0, 0.0), Some(0));
        assert_eq!(layout.hit_test(4.0, 0.0), Some(0));
        assert_eq!(layout.hit_test(6.0, 0.0), Some(1));
        assert_eq!(layout.hit_test(25.0, 0.0), Some(3));
    }

    #[test]
    fn a_hit_test_picks_the_line_by_height() {
        let layout = layout_of("abc\ndef\n", 1000.0, 10);
        assert_eq!(layout.hit_test(0.0, 0.0), Some(0));
        assert_eq!(layout.hit_test(0.0, 25.0), Some(4));
    }

    #[test]
    fn an_offset_maps_back_to_a_point() {
        let layout = layout_of("abc\ndef\n", 1000.0, 10);
        assert_eq!(layout.offset_point(0), Some((0.0, 0.0)));
        assert_eq!(layout.offset_point(1), Some((10.0, 0.0)));
        assert_eq!(layout.offset_point(4), Some((0.0, 20.0)));
    }

    #[test]
    fn a_wrapped_line_keeps_the_document_offsets() {
        let layout = layout_of("abcdefghij", 50.0, 10);
        assert_eq!(layout.lines[0].range, 0..5);
        assert_eq!(layout.lines[1].range, 5..10);
        assert_eq!(layout.offset_point(7), Some((20.0, 20.0)));
    }

    #[test]
    fn a_wide_character_is_not_split() {
        // "中文" is two characters of three bytes each.
        let layout = layout_of("中文", 1000.0, 10);
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].text, "中文");
        assert_eq!(layout.lines[0].range, 0..6);
    }
}
