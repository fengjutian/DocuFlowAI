#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    pub fn max_x(self) -> f32 {
        self.origin.x + self.size.width
    }
    pub fn max_y(self) -> f32 {
        self.origin.y + self.size.height
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let x = self.origin.x.max(other.origin.x);
        let y = self.origin.y.max(other.origin.y);
        let max_x = self.max_x().min(other.max_x());
        let max_y = self.max_y().min(other.max_y());
        (max_x > x && max_y > y).then(|| Self::new(x, y, max_x - x, max_y - y))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }
}

impl From<Color> for [f32; 4] {
    fn from(color: Color) -> Self {
        [color.r, color.g, color.b, color.a]
    }
}

impl From<Color> for wgpu::Color {
    fn from(color: Color) -> Self {
        Self {
            r: color.r as f64,
            g: color.g as f64,
            b: color.b as f64,
            a: color.a as f64,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Quad {
    pub bounds: Rect,
    pub color: Color,
    pub clip: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    pub text: String,
    pub bounds: Rect,
    pub clip: Rect,
    pub color: Color,
    pub font_size: f32,
    pub line_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub clip: Rect,
    pub color: Color,
    pub font_size: f32,
    pub line_height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameScene {
    pub clear: Color,
    pub quads: Vec<Quad>,
    pub glyph_runs: Vec<GlyphRun>,
}

impl Default for FrameScene {
    fn default() -> Self {
        Self {
            clear: Color::rgb(0.047, 0.055, 0.071),
            quads: Vec::new(),
            glyph_runs: Vec::new(),
        }
    }
}

impl FrameScene {
    pub fn push_quad(&mut self, bounds: Rect, color: Color, clip: Rect) {
        if bounds.intersection(clip).is_some() {
            self.quads.push(Quad {
                bounds,
                color,
                clip,
            });
        }
    }

    pub fn push_text(&mut self, text: impl Into<String>, bounds: Rect, style: TextStyle) {
        let text = text.into();
        if !text.is_empty() && bounds.intersection(style.clip).is_some() {
            self.glyph_runs.push(GlyphRun {
                text,
                bounds,
                clip: style.clip,
                color: style.color,
                font_size: style.font_size,
                line_height: style.line_height,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_intersection_clips_all_edges() {
        assert_eq!(
            Rect::new(0.0, 0.0, 10.0, 10.0).intersection(Rect::new(5.0, -2.0, 10.0, 6.0)),
            Some(Rect::new(5.0, 0.0, 5.0, 4.0))
        );
    }

    #[test]
    fn scene_discards_fully_clipped_quads() {
        let mut scene = FrameScene::default();
        scene.push_quad(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Color::rgb(1.0, 0.0, 0.0),
            Rect::new(20.0, 20.0, 10.0, 10.0),
        );
        assert!(scene.quads.is_empty());
    }
}
