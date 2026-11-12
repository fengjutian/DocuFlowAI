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

/// The selection's rectangle on one visual line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionRect {
    /// The left edge of the selection.
    pub x: f32,
    /// The top edge of the selection.
    pub y: f32,
    /// The width of the selection.
    pub width: f32,
    /// The height of the selection.
    pub height: f32,
}

/// The selection's rectangles on screen, one per visual line it covers.
///
/// A selection that spans several lines has no single rectangle; the caller
/// draws it line by line.
pub fn selection_rects(layout: &TextLayout, range: std::ops::Range<usize>) -> Vec<SelectionRect> {
    let mut rects = Vec::new();
    for line in &layout.lines {
        let start = range.start.max(line.range.start);
        let end = range.end.min(line.range.end);
        if end <= start {
            continue;
        }
        let x = line.offset_at(start);
        let width = line.offset_at(end) - x;
        if width <= 0.0 {
            continue;
        }
        rects.push(SelectionRect {
            x,
            y: line.y,
            width,
            height: layout.line_height,
        });
    }
    rects
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

    fn layout_of(text: &str) -> TextLayout {
        use crate::measure::MonospaceMeasurer;
        use codeweaver_text::Buffer;
        let buffer = Buffer::new(text);
        let mut measurer = MonospaceMeasurer::new(10.0);
        TextLayout::build(&buffer, 0, 10, 1000.0, 16.0, 20.0, &mut measurer)
    }

    #[test]
    fn a_selection_on_one_line_is_one_rectangle() {
        let layout = layout_of("abc\ndef\n");
        let rects = selection_rects(&layout, 1..3);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].x, 10.0);
        assert_eq!(rects[0].width, 20.0);
        assert_eq!(rects[0].y, 0.0);
        assert_eq!(rects[0].height, 20.0);
    }

    #[test]
    fn a_selection_over_several_lines_is_one_rectangle_per_line() {
        let layout = layout_of("abc\ndef\n");
        let rects = selection_rects(&layout, 1..5);
        assert_eq!(rects.len(), 2);
        assert_eq!((rects[0].x, rects[0].width), (10.0, 20.0));
        assert_eq!((rects[1].x, rects[1].width), (0.0, 10.0));
        assert_eq!(rects[1].y, 20.0);
    }

    #[test]
    fn an_empty_selection_has_no_rectangle() {
        let layout = layout_of("abc\ndef\n");
        assert!(selection_rects(&layout, 1..1).is_empty());
    }

    #[test]
    fn the_caret_sits_on_the_line_of_its_offset() {
        let layout = layout_of("abc\ndef\n");
        let caret = caret_shape(&layout, 5).expect("the caret is on screen");
        assert_eq!(caret.x, 10.0);
        assert_eq!(caret.y, 20.0);
        assert_eq!(caret.height, 20.0);
    }
}
