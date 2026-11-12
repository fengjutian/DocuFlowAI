//! The editor view: what part of the document is on screen.
//!
//! The view owns the scroll position and knows how to turn a mouse position
//! into a text offset and back. The caret and the selection live in the
//! editor's core, because they are part of the document's state and have to
//! survive an edit; the view only knows where they are on screen.

use crate::layout::TextLayout;

/// The scroll position of the editor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewState {
    /// The first logical line that is visible.
    pub first_line: usize,
    /// The horizontal scroll, in pixels.
    pub scroll_x: f32,
}

impl ViewState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scrolls by a number of logical lines.
    ///
    /// The position is clamped so the document can never be scrolled past its
    /// end.
    pub fn scroll_lines(&mut self, delta: isize, total_lines: usize) {
        let last = total_lines.saturating_sub(1);
        let next = self.first_line as isize + delta;
        self.first_line = next.clamp(0, last as isize) as usize;
    }

    /// Scrolls by a number of pixels.
    pub fn scroll_pixels(&mut self, delta: f32, line_height: f32, total_lines: usize) {
        if line_height <= 0.0 {
            return;
        }
        let delta_lines = (delta / line_height).round() as isize;
        self.scroll_lines(delta_lines, total_lines);
    }

    /// Makes sure a logical line is visible.
    ///
    /// `visible_lines` is how many logical lines fit on screen. The line is
    /// scrolled into view only when it is outside it.
    pub fn ensure_line_visible(&mut self, line: usize, visible_lines: usize, total_lines: usize) {
        let visible_lines = visible_lines.max(1);
        if line < self.first_line {
            self.first_line = line;
        } else if line >= self.first_line + visible_lines {
            self.first_line = line + 1 - visible_lines;
        }
        let last = total_lines.saturating_sub(1);
        self.first_line = self.first_line.min(last);
    }
}

/// The caret's rectangle on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretShape {
    /// The caret's left edge.
    pub x: f32,
    /// The caret's top edge.
    pub y: f32,
    /// The caret's height.
    pub height: f32,
}

/// Finds where the caret sits in the layout.
///
/// Returns `None` when the offset is outside the layout, which happens when
/// the caret is on a line that is not currently visible.
pub fn caret_shape(layout: &TextLayout, offset: usize) -> Option<CaretShape> {
    let (x, y) = layout.offset_point(offset)?;
    Some(CaretShape {
        x,
        y,
        height: layout.line_height,
    })
}

/// The selection's rectangle on screen, when the selection is on one line.
///
/// A selection that spans several lines has no single rectangle; the caller
/// draws it line by line.
pub fn selection_line(
    layout: &TextLayout,
    range: std::ops::Range<usize>,
) -> Option<(f32, f32, f32)> {
    let line = layout.line_of(range.start)?;
    let start = line.offset_at(range.start);
    let end = line.offset_at(range.end.min(line.range.end));
    if end < start {
        return None;
    }
    Some((start, line.y, end - start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_is_clamped_to_the_document() {
        let mut view = ViewState::new();
        view.scroll_lines(5, 3);
        assert_eq!(view.first_line, 2);
        view.scroll_lines(-10, 3);
        assert_eq!(view.first_line, 0);
    }

    #[test]
    fn scrolling_by_pixels_moves_by_lines() {
        let mut view = ViewState::new();
        view.scroll_pixels(45.0, 20.0, 100);
        assert_eq!(view.first_line, 2);
        view.scroll_pixels(-25.0, 20.0, 100);
        assert_eq!(view.first_line, 1);
    }

    #[test]
    fn a_line_outside_the_view_is_scrolled_into_it() {
        let mut view = ViewState::new();
        view.ensure_line_visible(10, 5, 100);
        assert_eq!(view.first_line, 6);
        view.ensure_line_visible(2, 5, 100);
        assert_eq!(view.first_line, 2);
    }

    #[test]
    fn a_line_inside_the_view_is_left_alone() {
        let mut view = ViewState::new();
        view.ensure_line_visible(3, 5, 100);
        assert_eq!(view.first_line, 0);
    }
}
