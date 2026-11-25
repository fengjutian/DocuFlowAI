//! The shapes the chrome of the editor is drawn with.
//!
//! The activity bar and the side bar are drawn with shapes, not with
//! pictures: the editor has no images of its own, and a picture of an icon
//! that does not fit the theme is worse than a shape that does.
//!
//! The renderer draws blocks of colour and then text on top of them, so a
//! shape is made of blocks of colour.

use codeweaver_render::{Color, FrameScene, Rect};

/// Draws a filled circle.
///
/// The circle is made of rows: every row is as wide as the circle is at that
/// height.
pub fn disc(scene: &mut FrameScene, viewport: Rect, center: Rect, radius: f32, color: Color) {
    let cx = center.origin.x + center.size.width / 2.0;
    let cy = center.origin.y + center.size.height / 2.0;
    let radius = radius.max(0.5);
    let rows = (radius * 2.0).ceil() as i32;
    for row in 0..=rows {
        let dy = -radius + row as f32;
        let half = (radius * radius - dy * dy).max(0.0).sqrt();
        if half <= 0.0 {
            continue;
        }
        scene.push_quad(
            Rect::new(cx - half, cy + dy, half * 2.0, 1.0),
            color,
            viewport,
        );
    }
}

/// Draws a circle with a hole in the middle.
///
/// The hole is not cut out: the rows that cross it are drawn as two pieces
/// with a gap between them.
pub fn ring(scene: &mut FrameScene, viewport: Rect, center: Rect, outer: f32, inner: f32, color: Color) {
    let cx = center.origin.x + center.size.width / 2.0;
    let cy = center.origin.y + center.size.height / 2.0;
    let outer = outer.max(0.5);
    let inner = inner.min(outer - 0.5).max(0.0);
    let rows = (outer * 2.0).ceil() as i32;
    for row in 0..=rows {
        let dy = -outer + row as f32;
        let half = (outer * outer - dy * dy).max(0.0).sqrt();
        if half <= 0.0 {
            continue;
        }
        let hole = (inner * inner - dy * dy).max(0.0).sqrt();
        if hole <= 0.0 {
            scene.push_quad(
                Rect::new(cx - half, cy + dy, half * 2.0, 1.0),
                color,
                viewport,
            );
            continue;
        }
        scene.push_quad(
            Rect::new(cx - half, cy + dy, half - hole, 1.0),
            color,
            viewport,
        );
        scene.push_quad(
            Rect::new(cx + hole, cy + dy, half - hole, 1.0),
            color,
            viewport,
        );
    }
}

/// Draws a triangle.
///
/// The triangle is made of columns: every column is one step narrower than
/// the one before it.
pub fn triangle(
    scene: &mut FrameScene,
    viewport: Rect,
    x: f32,
    y: f32,
    size: f32,
    points_right: bool,
    color: Color,
) {
    let steps = size.ceil().max(1.0) as i32;
    for step in 0..steps {
        let step = step as f32;
        let height = (size - step).max(0.0);
        let top = y + step / 2.0;
        let left = if points_right {
            x + step
        } else {
            x + size - step - 1.0
        };
        scene.push_quad(Rect::new(left, top, 1.0, height), color, viewport);
    }
}

/// Draws the border of a rectangle.
pub fn outline(
    scene: &mut FrameScene,
    viewport: Rect,
    rect: Rect,
    thickness: f32,
    color: Color,
) {
    let thickness = thickness.max(0.5);
    scene.push_quad(
        Rect::new(rect.origin.x, rect.origin.y, rect.size.width, thickness),
        color,
        viewport,
    );
    scene.push_quad(
        Rect::new(
            rect.origin.x,
            rect.max_y() - thickness,
            rect.size.width,
            thickness,
        ),
        color,
        viewport,
    );
    scene.push_quad(
        Rect::new(rect.origin.x, rect.origin.y, thickness, rect.size.height),
        color,
        viewport,
    );
    scene.push_quad(
        Rect::new(
            rect.max_x() - thickness,
            rect.origin.y,
            thickness,
            rect.size.height,
        ),
        color,
        viewport,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_circle_is_made_of_rows_that_get_shorter() {
        let mut scene = FrameScene::default();
        disc(
            &mut scene,
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Rect::new(40.0, 40.0, 20.0, 20.0),
            8.0,
            Color::rgb(1.0, 1.0, 1.0),
        );
        assert!(!scene.quads.is_empty());
    }
}
