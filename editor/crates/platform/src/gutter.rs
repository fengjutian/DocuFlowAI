//! The strip with the line numbers, on the left of the text.
//!
//! The numbers are the numbers of the **logical** lines of the document: a
//! line that is too long and wraps into two rows has one number, and the
//! second row has the same number. A number that counts the rows on screen
//! would say something different from what the status bar says about the
//! caret, and the person would not know which of the two is right.

use codeweaver_render::{Color, FrameScene, Rect, TextFamily, TextStyle};
use codeweaver_text::Buffer;
use codeweaver_ui::{TextMeasurer, VisualLine};

/// The space between the numbers and the text of the document.
const PADDING: f32 = 10.0;
/// How far the number is moved down, so that its bottom line is the bottom
/// line of the text.
const BASELINE_OFFSET: f32 = 3.0;

/// The color of a line number.
const NUMBER_COLOR: Color = Color::rgb(0.36, 0.40, 0.48);
/// The color of the number of the line the caret is on.
const CURRENT_COLOR: Color = Color::rgb(0.82, 0.86, 0.93);
/// The color behind the line the caret is on.
const CURRENT_BACKGROUND: Color = Color::rgb(0.098, 0.110, 0.138);

/// One line number, and where it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct GutterLine {
    /// The number of the line, counting from one.
    pub number: usize,
    /// Whether the caret is on this line.
    pub current: bool,
    /// Where the number is drawn.
    pub rect: Rect,
}

/// The numbers of the lines that are on screen.
///
/// `visual` are the rows of the text that are on screen: a row of the text
/// is not always a line of the document. `current` is the line of the
/// document the caret is on, counting from zero. `line_height` is the height
/// of one line of the text: a number and the line it belongs to have to be
/// on the same row, and the text can be drawn bigger or smaller.
pub fn lines(
    buffer: &Buffer,
    visual: &[VisualLine],
    gutter: Rect,
    current: usize,
    line_height: f32,
) -> Vec<GutterLine> {
    visual
        .iter()
        .map(|row| {
            let number = buffer.line_of_offset(row.range.start) + 1;
            GutterLine {
                number,
                current: number == current + 1,
                rect: Rect::new(
                    gutter.origin.x,
                    gutter.origin.y + row.y,
                    gutter.size.width,
                    line_height,
                ),
            }
        })
        .collect()
}

/// Draws the numbers.
///
/// The numbers are right-aligned: the ones of a long line end where the ones
/// of a short line end, so that the eye can follow the column.
pub fn draw(
    scene: &mut FrameScene,
    viewport: Rect,
    gutter: Rect,
    numbers: &[GutterLine],
    font_size: f32,
    line_height: f32,
    measurer: &mut dyn TextMeasurer,
) {
    for number in numbers {
        if number.current {
            // The line the caret is on is a little lighter: the person has to
            // find it without counting.
            scene.push_quad(number.rect, CURRENT_BACKGROUND, viewport);
        }
        let text = number.number.to_string();
        let width = measurer.measure(&text, font_size).width;
        scene.push_text(
            text,
            Rect::new(
                gutter.max_x() - PADDING - width,
                number.rect.origin.y + BASELINE_OFFSET,
                width + 2.0,
                line_height,
            ),
            TextStyle {
                clip: viewport,
                color: if number.current {
                    CURRENT_COLOR
                } else {
                    NUMBER_COLOR
                },
                font_size,
                line_height,
                family: TextFamily::SansSerif,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_ui::MonospaceMeasurer;

    /// The height of one line of the text.
    const LINE_HEIGHT: f32 = 22.0;
    /// The size the numbers are drawn at.
    const FONT_SIZE: f32 = 13.0;

    /// The gutter of a window that is 800 pixels wide.
    fn gutter() -> Rect {
        Rect::new(280.0, 34.0, 56.0, 400.0)
    }

    /// A buffer with three lines.
    fn buffer() -> Buffer {
        Buffer::new("one\ntwo\nthree")
    }

    /// The rows of the text that are on screen.
    fn rows(buffer: &Buffer) -> Vec<VisualLine> {
        let mut measurer = MonospaceMeasurer::new(8.0);
        codeweaver_ui::TextLayout::build(buffer, 0, 10, 400.0, 16.0, LINE_HEIGHT, &mut measurer).lines
    }
    #[test]
    fn every_line_of_the_document_has_a_number() {
        let buffer = buffer();
        let numbers = lines(&buffer, &rows(&buffer), gutter(), 0, LINE_HEIGHT);
        assert_eq!(
            numbers.iter().map(|line| line.number).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn the_number_is_the_number_of_the_line_of_the_document() {
        let buffer = buffer();
        let rows = rows(&buffer);
        // The second row of the text is the second line of the document.
        assert_eq!(rows[1].range.start, 4);
        let numbers = lines(&buffer, &rows, gutter(), 0, LINE_HEIGHT);
        assert_eq!(numbers[1].number, 2);
    }

    #[test]
    fn the_line_of_the_caret_is_the_one_that_is_marked() {
        let buffer = buffer();
        let numbers = lines(&buffer, &rows(&buffer), gutter(), 1, LINE_HEIGHT);
        assert!(numbers[1].current);
        assert!(!numbers[0].current);
        assert!(!numbers[2].current);
    }

    #[test]
    fn the_numbers_are_on_the_rows_of_the_text() {
        let buffer = buffer();
        let rows = rows(&buffer);
        let numbers = lines(&buffer, &rows, gutter(), 0, LINE_HEIGHT);
        assert_eq!(numbers[0].rect.origin.y, gutter().origin.y);
        assert_eq!(numbers[1].rect.origin.y, gutter().origin.y + rows[1].y);
    }

    #[test]
    fn the_numbers_are_right_aligned() {
        let mut scene = FrameScene::default();
        let mut measurer = MonospaceMeasurer::new(8.0);
        let numbers = vec![
            GutterLine {
                number: 9,
                current: false,
                rect: Rect::new(0.0, 0.0, 56.0, 22.0),
            },
            GutterLine {
                number: 100,
                current: false,
                rect: Rect::new(0.0, 22.0, 56.0, 22.0),
            },
        ];
        draw(
            &mut scene,
            Rect::new(0.0, 0.0, 800.0, 600.0),
            Rect::new(0.0, 0.0, 56.0, 400.0),
            &numbers,
            FONT_SIZE,
            LINE_HEIGHT,
            &mut measurer,
        );
        assert_eq!(scene.glyph_runs.len(), 2);
        let one = scene.glyph_runs[0].bounds;
        let two = scene.glyph_runs[1].bounds;
        assert_eq!(one.max_x(), two.max_x(), "the numbers end in the same place");
    }
}
