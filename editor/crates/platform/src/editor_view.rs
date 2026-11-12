//! The editor body: the text on screen, the caret and the selection.
//!
//! The body is the part of the window the user types in. It owns the scroll
//! position and the layout of the text that is visible. The text itself and
//! the selection stay in the editor's core: the body only knows where they
//! are on screen.

use codeweaver_editor_core::Editor;
use codeweaver_render::{Color, FrameScene, Rect, Renderer, TextFamily, TextStyle};
use codeweaver_text::{Buffer, TextOffset};
use codeweaver_ui::{
    LineMetrics, TextLayout, TextMeasurer, ViewState, VisualLine, caret_shape, selection_rects,
};

/// The size the body text is drawn at.
pub const FONT_SIZE: f32 = 16.0;
/// The height of one line of the body.
pub const LINE_HEIGHT: f32 = 22.0;
/// The width of the caret.
const CARET_WIDTH: f32 = 2.0;

/// The color of the text.
const TEXT_COLOR: Color = Color::rgb(0.88, 0.90, 0.94);
/// The color of the selected text.
const SELECTION_COLOR: Color = Color::rgb(0.20, 0.33, 0.55);
/// The color of the caret.
const CARET_COLOR: Color = Color::rgb(0.30, 0.58, 0.96);

/// The presentation state of the editor body.
pub struct EditorView {
    /// Where the view is scrolled to.
    pub view: ViewState,
    /// The layout of the text that is on screen.
    layout: TextLayout,
    /// The offset the current mouse drag started at.
    drag_anchor: Option<usize>,
    /// What the layout was built from.
    built: Built,
}

/// The state the layout was built from.
///
/// Measuring a line is not free, so the layout is kept until the text, the
/// scroll position or the size of the body changes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Built {
    version: u64,
    width: f32,
    height: f32,
    first_line: usize,
}

impl Default for EditorView {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorView {
    pub fn new() -> Self {
        EditorView {
            view: ViewState::new(),
            layout: TextLayout::default(),
            drag_anchor: None,
            built: Built {
                version: u64::MAX,
                width: 0.0,
                height: 0.0,
                first_line: 0,
            },
        }
    }

    /// The layout of the text that is on screen.
    pub fn current_layout(&self) -> &TextLayout {
        &self.layout
    }

    /// Rebuilds the layout when the text, the scroll position or the size of
    /// the body changed.
    pub fn layout(
        &mut self,
        buffer: &Buffer,
        width: f32,
        height: f32,
        measurer: &mut dyn TextMeasurer,
    ) {
        let built = Built {
            version: buffer.version(),
            width,
            height,
            first_line: self.view.first_line,
        };
        if built == self.built {
            return;
        }
        let lines = ((height / LINE_HEIGHT).floor() as usize).max(1);
        self.layout = TextLayout::build(
            buffer,
            self.view.first_line,
            lines,
            width,
            FONT_SIZE,
            LINE_HEIGHT,
            measurer,
        );
        self.built = built;
    }

    /// The offset at a point in the body, relative to the body's top left.
    pub fn offset_at(&self, x: f32, y: f32) -> Option<usize> {
        self.layout.hit_test(x, y)
    }

    /// Places the caret at a point and starts a drag from there.
    pub fn press(&mut self, editor: &mut Editor, x: f32, y: f32) {
        let Some(offset) = self.layout.hit_test(x, y) else {
            return;
        };
        self.drag_anchor = Some(offset);
        editor.select_range(TextOffset(offset), TextOffset(offset));
    }

    /// Extends the selection to a point.
    pub fn drag(&mut self, editor: &mut Editor, x: f32, y: f32) {
        let Some(anchor) = self.drag_anchor else {
            return;
        };
        let Some(offset) = self.layout.hit_test(x, y) else {
            return;
        };
        editor.select_range(TextOffset(anchor), TextOffset(offset));
    }

    /// Ends a mouse drag.
    pub fn release(&mut self) {
        self.drag_anchor = None;
    }

    /// Whether a mouse drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag_anchor.is_some()
    }

    /// Scrolls the body by a number of logical lines.
    pub fn scroll(&mut self, lines: isize, total_lines: usize) {
        self.view.scroll_lines(lines, total_lines);
    }

    /// Moves the caret one visual line up or down.
    ///
    /// Returns `false` when the caret would leave the text that is on screen;
    /// the caller scrolls and tries again.
    pub fn move_vertical(&mut self, editor: &mut Editor, delta: isize) -> bool {
        let caret = editor.caret().0;
        let Some(index) = self.layout.line_index_of(caret) else {
            return false;
        };
        let target = index as isize + delta;
        if target < 0 || target >= self.layout.lines.len() as isize {
            return false;
        }
        // The caret keeps its column: the new offset is the character
        // closest to the same x on the line above or below.
        let x = self.layout.lines[index].offset_at(caret);
        let y = self.layout.lines[target as usize].y;
        let Some(offset) = self.layout.hit_test(x, y) else {
            return false;
        };
        editor.select_range(TextOffset(offset), TextOffset(offset));
        true
    }

    /// Moves the caret to the start or the end of its line.
    pub fn move_to_line_edge(&mut self, editor: &mut Editor, to_end: bool) {
        let caret = editor.caret().0;
        let line = editor.buffer().line_of_offset(caret);
        let range = editor.buffer().line_range(line);
        let target = if to_end { range.end } else { range.start };
        editor.select_range(target, target);
    }

    /// Scrolls the body so that the caret is on screen.
    pub fn ensure_caret_visible(
        &mut self,
        buffer: &Buffer,
        caret: usize,
        total_lines: usize,
        visible_lines: usize,
    ) {
        if self.layout.line_index_of(caret).is_some() {
            return;
        }
        let line = buffer.line_of_offset(caret);
        self.view.ensure_line_visible(line, visible_lines, total_lines);
    }

    /// Draws the body: the selection, the text and the caret.
    pub fn draw(
        &self,
        scene: &mut FrameScene,
        editor: &Editor,
        preedit: &str,
        body: Rect,
        measurer: &mut dyn TextMeasurer,
    ) {
        let clip = body;

        let selection = editor.selection().range(editor.buffer());
        for rect in selection_rects(&self.layout, selection.start.0..selection.end.0) {
            scene.push_quad(
                Rect::new(
                    body.origin.x + rect.x,
                    body.origin.y + rect.y,
                    rect.width,
                    rect.height,
                ),
                SELECTION_COLOR,
                clip,
            );
        }

        let caret = editor.caret().0;
        for line in &self.layout.lines {
            let y = body.origin.y + line.y;
            match self.preedit_split(line, caret, preedit) {
                Some(at) => {
                    // The composition belongs between the text before the
                    // caret and the text after it, so the line is drawn in
                    // two parts.
                    let before = &line.text[..at];
                    let after = &line.text[at..];
                    let combined = format!("{before}{preedit}");
                    let width = measurer.measure(&combined, FONT_SIZE).width;
                    self.push_line(scene, &combined, body.origin.x, y, body, clip);
                    self.push_line(scene, after, body.origin.x + width, y, body, clip);
                }
                None => self.push_line(scene, &line.text, body.origin.x, y, body, clip),
            }
        }

        if let Some(caret) = caret_shape(&self.layout, caret) {
            scene.push_quad(
                Rect::new(
                    body.origin.x + caret.x,
                    body.origin.y + caret.y,
                    CARET_WIDTH,
                    caret.height,
                ),
                CARET_COLOR,
                clip,
            );
        }
    }

    /// Draws one line of text.
    fn push_line(
        &self,
        scene: &mut FrameScene,
        text: &str,
        x: f32,
        y: f32,
        body: Rect,
        clip: Rect,
    ) {
        // The run is wider than the body so the renderer does not wrap the
        // text a second time: the body already wrapped it.
        scene.push_text(
            text,
            Rect::new(x, y, body.size.width * 2.0, LINE_HEIGHT),
            TextStyle {
                clip,
                color: TEXT_COLOR,
                font_size: FONT_SIZE,
                line_height: LINE_HEIGHT,
                family: TextFamily::Monospace,
            },
        );
    }

    /// Where the composition starts in a line, when the caret is on it.
    fn preedit_split(&self, line: &VisualLine, caret: usize, preedit: &str) -> Option<usize> {
        if preedit.is_empty() || caret < line.range.start || caret > line.range.end {
            return None;
        }
        let at = caret - line.range.start;
        // The offset has to be a character boundary of the line's own text.
        line.text.is_char_boundary(at).then_some(at)
    }
}

/// Measures text the way the renderer draws it.
///
/// The layout asks for measurements through this trait instead of guessing
/// from a font size, so the caret lands on the glyph and not next to it.
pub struct RendererMeasurer<'a> {
    renderer: &'a mut Renderer,
    family: TextFamily,
}

impl<'a> RendererMeasurer<'a> {
    pub fn new(renderer: &'a mut Renderer, family: TextFamily) -> Self {
        RendererMeasurer { renderer, family }
    }
}

impl TextMeasurer for RendererMeasurer<'_> {
    fn measure(&mut self, text: &str, font_size: f32) -> LineMetrics {
        let measured = self.renderer.measure_text(text, font_size, self.family);
        LineMetrics {
            boundaries: measured.boundaries,
            width: measured.width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_ui::MonospaceMeasurer;

    fn view_of(text: &str) -> (EditorView, Editor) {
        let mut editor = Editor::new(text);
        let mut view = EditorView::new();
        let mut measurer = MonospaceMeasurer::new(10.0);
        view.layout(editor.buffer(), 1000.0, 200.0, &mut measurer);
        (view, editor)
    }

    #[test]
    fn a_click_places_the_caret_at_the_closest_character() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        view.press(&mut editor, 25.0, 0.0);
        assert_eq!(editor.caret().0, 3);
        assert!(editor.selection().range(editor.buffer()).is_empty());
    }

    #[test]
    fn a_drag_selects_the_text_between_the_two_points() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        view.press(&mut editor, 0.0, 0.0);
        view.drag(&mut editor, 25.0, 0.0);
        assert_eq!(editor.selection().range(editor.buffer()).start.0, 0);
        assert_eq!(editor.selection().range(editor.buffer()).end.0, 3);
        view.release();
        assert!(!view.is_dragging());
    }

    #[test]
    fn a_drag_without_a_press_changes_nothing() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        view.drag(&mut editor, 25.0, 0.0);
        assert_eq!(editor.caret().0, 0);
    }

    #[test]
    fn moving_down_keeps_the_column() {
        let (mut view, mut editor) = view_of("abcdef\nabcdef\n");
        editor.select_range(TextOffset(2), TextOffset(2));
        assert!(view.move_vertical(&mut editor, 1));
        assert_eq!(editor.caret().0, 9);
        assert!(view.move_vertical(&mut editor, -1));
        assert_eq!(editor.caret().0, 2);
    }

    #[test]
    fn moving_past_the_edge_of_the_screen_is_refused() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        assert!(!view.move_vertical(&mut editor, -1));
        editor.select_range(TextOffset(8), TextOffset(8));
        assert!(!view.move_vertical(&mut editor, 1));
    }

    #[test]
    fn the_line_edges_are_the_start_and_the_end_of_the_line() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        editor.select_range(TextOffset(5), TextOffset(5));
        view.move_to_line_edge(&mut editor, false);
        assert_eq!(editor.caret().0, 4);
        view.move_to_line_edge(&mut editor, true);
        assert_eq!(editor.caret().0, 7);
    }

    #[test]
    fn a_caret_outside_the_screen_scrolls_into_it() {
        let mut editor = Editor::new("");
        for _ in 0..100 {
            editor.insert("line\n").unwrap();
        }
        let mut view = EditorView::new();
        let mut measurer = MonospaceMeasurer::new(10.0);
        view.layout(editor.buffer(), 1000.0, 200.0, &mut measurer);

        let caret = editor.caret().0;
        view.ensure_caret_visible(editor.buffer(), caret, editor.buffer().line_count(), 9);
        assert!(view.view.first_line > 0);
    }

    #[test]
    fn a_caret_that_is_already_on_screen_does_not_scroll() {
        let (mut view, editor) = view_of("abc\ndef\n");
        let before = view.view.first_line;
        view.ensure_caret_visible(editor.buffer(), 1, editor.buffer().line_count(), 9);
        assert_eq!(view.view.first_line, before);
    }
}
