//! The strip on the right of the text: a map of the whole document.
//!
//! The map says where the long lines are and which part of the file is on
//! screen. VS Code draws the letters of the file there; this draws the lines
//! — the shape of the file is what is needed to find a place in a big one.
//!
//! The map is built from the buffer, not from the text on screen: the point
//! of it is to show the whole file, including the part that is not on screen.

use codeweaver_render::{Color, FrameScene, Rect};
use codeweaver_text::Buffer;

/// The width of the strip.
pub const WIDTH: f32 = 90.0;
/// The space between the text and the map.
pub const GAP: f32 = 12.0;
/// The width of one character on the map.
const CHARACTER: f32 = 0.6;
/// The most characters a line can be wide on the map.
const MAX_CHARACTERS: f32 = 120.0;
/// The most pixels one line of the file is high on the map.
const MAX_LINE_HEIGHT: f32 = 2.0;

/// The colour of the ground of the map.
const GROUND: Color = Color::rgb(0.062, 0.071, 0.094);
/// The colour of a line of the file that is not on screen.
const LINE: Color = Color::rgb(0.28, 0.31, 0.38);
/// The colour of a line of the file that is on screen.
const VISIBLE_LINE: Color = Color::rgb(0.52, 0.56, 0.64);
/// The colour of the edge of the part of the file that is on screen.
const SLIDER: Color = Color::rgb(0.34, 0.40, 0.50);

/// The map of the document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Minimap {
    /// How wide every line of the document is, in characters.
    pub widths: Vec<f32>,
}

/// How the map is laid out in its strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// The height of one line of the file on the map.
    pub line_height: f32,
    /// The height of the whole map.
    pub height: f32,
}

impl Minimap {
    /// The map of the document.
    ///
    /// The width of a line is how many characters it has. The text is not
    /// copied: the characters are counted where the bytes lie.
    pub fn of(buffer: &Buffer) -> Minimap {
        let count = buffer.line_count();
        let mut widths = Vec::with_capacity(count);
        for line in 0..count {
            let start = buffer.line_range(line).start.0;
            let bytes = buffer.slice_from(start);
            let end = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap_or(bytes.len());
            // The bytes that continue a character are not characters of
            // their own: a Chinese character is three bytes and one
            // character.
            let characters = bytes[..end]
                .iter()
                .filter(|byte| (**byte & 0xC0) != 0x80)
                .count();
            widths.push(characters.min(MAX_CHARACTERS as usize) as f32);
        }
        Minimap { widths }
    }

    /// How many lines the document has.
    pub fn len(&self) -> usize {
        self.widths.len()
    }

    /// Whether the document is empty.
    pub fn is_empty(&self) -> bool {
        self.widths.is_empty()
    }

    /// How the map is laid out in its strip.
    ///
    /// A short file is not stretched: its map ends where the file ends. A
    /// long file is squeezed: one line of it is less than a pixel high, and
    /// the whole file fits in the strip.
    pub fn geometry(&self, strip: Rect) -> Geometry {
        let total = self.len().max(1) as f32;
        let line_height = (strip.size.height.max(1.0) / total).min(MAX_LINE_HEIGHT);
        Geometry {
            line_height,
            height: total * line_height,
        }
    }

    /// The line of the document that a point of the strip is about.
    ///
    /// Returns nothing when the point is not on the map: the map of a short
    /// file does not fill the whole strip, and the rest of the strip is not
    /// the file.
    pub fn line_at(&self, y: f32, strip: Rect) -> Option<usize> {
        if self.is_empty() {
            return None;
        }
        let geometry = self.geometry(strip);
        if y < strip.origin.y || y > strip.origin.y + geometry.height {
            return None;
        }
        let line = ((y - strip.origin.y) / geometry.line_height).floor().max(0.0) as usize;
        Some(line.min(self.len() - 1))
    }
}

/// Draws the map.
///
/// The whole document is squeezed into the strip: every pixel row of the map
/// is the widest line that falls into it. The part of the file that is on
/// screen is brighter, and it has an edge around it.
pub fn draw(
    scene: &mut FrameScene,
    viewport: Rect,
    strip: Rect,
    map: &Minimap,
    first_line: usize,
    visible_lines: usize,
) {
    if map.is_empty() {
        return;
    }
    scene.push_quad(strip, GROUND, viewport);
    let total = map.len();
    let geometry = map.geometry(strip);
    let rows = strip.size.height.floor().max(1.0) as usize;

    for row in 0..rows {
        // The lines that fall into this row of the map.
        let from = ((row as f32) / geometry.line_height).floor() as usize;
        let to = (((row + 1) as f32) / geometry.line_height)
            .ceil()
            .max((from + 1) as f32) as usize;
        let from = from.min(total - 1);
        let to = to.min(total).max(from + 1);
        let width = map.widths[from..to]
            .iter()
            .copied()
            .fold(0.0f32, f32::max);
        if width <= 0.0 {
            continue;
        }
        let on_screen = from >= first_line && from < first_line + visible_lines;
        let length = (width * CHARACTER).min(strip.size.width - 4.0).max(1.0);
        scene.push_quad(
            Rect::new(strip.origin.x + 2.0, strip.origin.y + row as f32, length, 1.0),
            if on_screen { VISIBLE_LINE } else { LINE },
            viewport,
        );
    }

    // The edge of the part of the file that is on screen.
    let top = strip.origin.y + first_line as f32 * geometry.line_height;
    let bottom = strip.origin.y + (first_line + visible_lines) as f32 * geometry.line_height;
    let bottom = bottom.min(strip.max_y()).max(top + 1.0);
    let width = strip.size.width - 2.0;
    scene.push_quad(
        Rect::new(strip.origin.x + 1.0, top, width, 1.0),
        SLIDER,
        viewport,
    );
    scene.push_quad(
        Rect::new(strip.origin.x + 1.0, bottom - 1.0, width, 1.0),
        SLIDER,
        viewport,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strip of the map in a window that is 800 pixels wide.
    fn strip() -> Rect {
        Rect::new(700.0, 34.0, WIDTH, 700.0)
    }

    #[test]
    fn every_line_of_the_document_is_on_the_map() {
        let buffer = Buffer::new("one\ntwo\nthree");
        let map = Minimap::of(&buffer);
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn the_width_of_a_line_is_the_number_of_its_characters() {
        let buffer = Buffer::new("a\nabcd\n");
        let map = Minimap::of(&buffer);
        assert_eq!(map.widths[0], 1.0);
        assert_eq!(map.widths[1], 4.0);
    }

    #[test]
    fn a_chinese_character_is_one_character_and_not_three_bytes() {
        let buffer = Buffer::new("中文");
        let map = Minimap::of(&buffer);
        assert_eq!(map.widths[0], 2.0);
    }

    #[test]
    fn a_short_file_is_not_stretched_over_the_whole_strip() {
        let buffer = Buffer::new("one\ntwo\nthree");
        let map = Minimap::of(&buffer);
        let geometry = map.geometry(strip());
        assert_eq!(geometry.line_height, MAX_LINE_HEIGHT);
        assert!(geometry.height < strip().size.height);
    }

    #[test]
    fn a_long_file_is_squeezed_into_the_strip() {
        let text = (0..1000).map(|_| "line").collect::<Vec<_>>().join("\n");
        let buffer = Buffer::new(text);
        let map = Minimap::of(&buffer);
        let geometry = map.geometry(strip());
        assert!(geometry.line_height < 1.0);
        assert!(geometry.height <= strip().size.height);
    }

    #[test]
    fn the_map_says_which_line_a_point_is_about() {
        let text = (0..100).map(|_| "line").collect::<Vec<_>>().join("\n");
        let buffer = Buffer::new(text);
        let map = Minimap::of(&buffer);
        let geometry = map.geometry(strip());
        let line = map
            .line_at(strip().origin.y + geometry.line_height * 10.0, strip())
            .expect("the point is on the map");
        assert_eq!(line, 10);
    }

    #[test]
    fn a_point_below_the_map_of_a_short_file_is_not_on_the_map() {
        let buffer = Buffer::new("one\ntwo\nthree");
        let map = Minimap::of(&buffer);
        let geometry = map.geometry(strip());
        let below = strip().origin.y + geometry.height + 10.0;
        assert_eq!(map.line_at(below, strip()), None);
    }

    #[test]
    fn the_map_draws_a_bar_for_every_row_of_the_strip() {
        let mut scene = FrameScene::default();
        let buffer = Buffer::new("one\ntwo\nthree");
        let map = Minimap::of(&buffer);
        draw(
            &mut scene,
            Rect::new(0.0, 0.0, 1280.0, 800.0),
            strip(),
            &map,
            0,
            10,
        );
        // The ground of the map, one bar per row of the strip, and the two
        // edges of the part of the file that is on screen.
        assert_eq!(scene.quads.len(), 3 + strip().size.height as usize);
    }
}
