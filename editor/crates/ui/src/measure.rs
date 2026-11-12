//! Text measurement, injected into the layout.
//!
//! The layout has to know where every character ends up on screen, and only
//! the renderer knows: it owns the font system and the shaping. So the layout
//! asks for measurements through this trait instead of guessing from a font
//! size — guessing is how a caret ends up between two characters.

/// The measured shape of a run of text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineMetrics {
    /// The x offset of every byte boundary, in logical pixels.
    ///
    /// One more entry than the text has bytes: the last entry is the width of
    /// the run. The bytes inside one character share the character's start
    /// offset.
    pub boundaries: Vec<f32>,
    /// The width of the run.
    pub width: f32,
}

impl LineMetrics {
    /// The x offset of a byte offset within the measured text.
    pub fn offset_at(&self, byte_offset: usize) -> f32 {
        self.boundaries
            .get(byte_offset)
            .copied()
            .unwrap_or(self.width)
    }
}

/// Measures a run of text.
pub trait TextMeasurer {
    /// Measures the text at `font_size`.
    fn measure(&mut self, text: &str, font_size: f32) -> LineMetrics;
}

/// A measurer for text that has no glyphs of its own.
///
/// It assumes every character is one unit wide. It exists so the layout can
/// be tested without a font system, and so a caller without a renderer still
/// gets a usable — if approximate — layout.
#[derive(Clone, Copy, Debug)]
pub struct MonospaceMeasurer {
    /// The width of one character.
    pub advance: f32,
}

impl MonospaceMeasurer {
    pub fn new(advance: f32) -> Self {
        MonospaceMeasurer { advance }
    }
}

impl TextMeasurer for MonospaceMeasurer {
    fn measure(&mut self, text: &str, _font_size: f32) -> LineMetrics {
        let mut boundaries = Vec::with_capacity(text.len() + 1);
        let mut width = 0.0;
        for character in text.chars() {
            // The bytes inside one character have no glyph of their own, so
            // they share the character's start offset.
            for _ in 0..character.len_utf8() {
                boundaries.push(width);
            }
            width += self.advance;
        }
        boundaries.push(width);
        LineMetrics { width, boundaries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_monospace_measurer_reports_one_advance_per_character() {
        let mut measurer = MonospaceMeasurer::new(10.0);
        let metrics = measurer.measure("abc", 16.0);
        assert_eq!(metrics.width, 30.0);
        assert_eq!(metrics.boundaries, vec![0.0, 10.0, 20.0, 30.0]);
        assert_eq!(metrics.offset_at(2), 20.0);
    }

    #[test]
    fn the_monospace_measurer_counts_characters_not_bytes() {
        let mut measurer = MonospaceMeasurer::new(10.0);
        let metrics = measurer.measure("中文", 16.0);
        assert_eq!(metrics.width, 20.0);
        // The bytes inside one character share its start offset.
        assert_eq!(metrics.boundaries, vec![0.0, 0.0, 0.0, 10.0, 10.0, 10.0, 20.0]);
    }
}
