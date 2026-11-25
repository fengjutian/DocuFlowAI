//! The status bar at the bottom of the window.
//!
//! The bar says three things: on the left what is open and what is wrong
//! with it, in the middle what the editor is doing inside, on the right
//! where the caret is.
//!
//! It is at the bottom of the window because that is where it belongs: the
//! text of the document is the thing the person looks at, and a bar above it
//! pushes the text down.

use codeweaver_render::{Color, FrameScene, Rect, TextFamily, TextStyle};
use codeweaver_ui::TextMeasurer;

/// The color of the bar.
const BAR_COLOR: Color = Color::rgb(0.075, 0.086, 0.11);
/// The color of the line above the bar.
const BORDER_COLOR: Color = Color::rgb(0.16, 0.18, 0.22);
/// The color of the text on the left.
const LEFT_COLOR: Color = Color::rgb(0.78, 0.82, 0.90);
/// The color of the text in the middle.
const MIDDLE_COLOR: Color = Color::rgb(0.45, 0.49, 0.57);
/// The color of the text on the right.
const RIGHT_COLOR: Color = Color::rgb(0.62, 0.66, 0.74);
/// The space on the left of the text.
const PADDING: f32 = 12.0;

/// What the status bar says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusBar {
    /// The file that is open, and what is wrong with it.
    pub left: String,
    /// What the editor is doing inside.
    pub middle: String,
    /// Where the caret is, and what the file is.
    pub right: String,
}

/// Draws the bar.
pub fn draw(
    scene: &mut FrameScene,
    viewport: Rect,
    rect: Rect,
    bar: &StatusBar,
    measurer: &mut dyn TextMeasurer,
) {
    scene.push_quad(rect, BAR_COLOR, viewport);
    scene.push_quad(
        Rect::new(
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            1.0,
        ),
        BORDER_COLOR,
        viewport,
    );

    let height = rect.size.height;
    let mut x = rect.origin.x + PADDING;
    if !bar.left.is_empty() {
        let width = measurer.measure(&bar.left, 12.0).width;
        scene.push_text(
            bar.left.clone(),
            Rect::new(x, rect.origin.y + 4.0, width + 2.0, height - 4.0),
            TextStyle {
                clip: viewport,
                color: LEFT_COLOR,
                font_size: 12.0,
                line_height: height - 6.0,
                family: TextFamily::SansSerif,
            },
        );
        x += width + 24.0;
    }
    if !bar.middle.is_empty() {
        let width = measurer.measure(&bar.middle, 12.0).width;
        scene.push_text(
            bar.middle.clone(),
            Rect::new(x, rect.origin.y + 4.0, width + 2.0, height - 4.0),
            TextStyle {
                clip: viewport,
                color: MIDDLE_COLOR,
                font_size: 12.0,
                line_height: height - 6.0,
                family: TextFamily::SansSerif,
            },
        );
    }
    if !bar.right.is_empty() {
        let width = measurer.measure(&bar.right, 12.0).width;
        scene.push_text(
            bar.right.clone(),
            Rect::new(
                rect.max_x() - PADDING - width,
                rect.origin.y + 4.0,
                width + 2.0,
                height - 4.0,
            ),
            TextStyle {
                clip: viewport,
                color: RIGHT_COLOR,
                font_size: 12.0,
                line_height: height - 6.0,
                family: TextFamily::SansSerif,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_ui::MonospaceMeasurer;

    #[test]
    fn the_bar_is_drawn_at_the_bottom_of_the_window() {
        let mut scene = FrameScene::default();
        let mut measurer = MonospaceMeasurer::new(6.0);
        let rect = Rect::new(0.0, 776.0, 1280.0, 24.0);
        let bar = StatusBar {
            left: "main.rs".to_owned(),
            middle: "Buffer v0".to_owned(),
            right: "行 1，列 1".to_owned(),
        };
        draw(
            &mut scene,
            Rect::new(0.0, 0.0, 1280.0, 800.0),
            rect,
            &bar,
            &mut measurer,
        );
        assert_eq!(scene.quads[0].bounds, rect);
        assert_eq!(scene.glyph_runs.len(), 3);
    }

    #[test]
    fn an_empty_bar_says_nothing() {
        let mut scene = FrameScene::default();
        let mut measurer = MonospaceMeasurer::new(6.0);
        let bar = StatusBar::default();
        draw(
            &mut scene,
            Rect::new(0.0, 0.0, 1280.0, 800.0),
            Rect::new(0.0, 776.0, 1280.0, 24.0),
            &bar,
            &mut measurer,
        );
        assert!(scene.glyph_runs.is_empty());
    }
}
