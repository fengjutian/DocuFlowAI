//! The editor body: the text on screen, the caret and the selection.
//!
//! The body is the part of the window the user types in. It owns the scroll
//! position and the layout of the text that is visible. The text itself and
//! the selection stay in the editor's core: the body only knows where they
//! are on screen.

use std::ops::Range;

use codeweaver_editor_core::Editor;
use codeweaver_render::{Color, FrameScene, Rect, Renderer, TextFamily, TextShaper, TextStyle};
use codeweaver_syntax::{Highlight, HighlightKind};
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
/// The color of a match that the find bar found.
const MATCH_COLOR: Color = Color::rgb(0.34, 0.29, 0.10);
/// The color of the match that the find bar is on.
const CURRENT_MATCH_COLOR: Color = Color::rgb(0.58, 0.46, 0.11);

/// The color that a kind of text is drawn in.
///
/// The colors are the ones of a dark theme: the text stays readable, and the
/// kinds that are worth noticing are the ones that stand out.
pub fn highlight_color(kind: HighlightKind) -> Color {
    match kind {
        HighlightKind::Keyword => Color::rgb(0.76, 0.36, 0.90),
        HighlightKind::Function => Color::rgb(0.40, 0.72, 0.98),
        HighlightKind::Type => Color::rgb(0.32, 0.80, 0.72),
        HighlightKind::String => Color::rgb(0.68, 0.82, 0.42),
        HighlightKind::Number => Color::rgb(0.92, 0.70, 0.38),
        HighlightKind::Comment => Color::rgb(0.42, 0.47, 0.56),
        HighlightKind::Operator => Color::rgb(0.74, 0.78, 0.86),
        HighlightKind::Punctuation => Color::rgb(0.60, 0.65, 0.74),
        HighlightKind::Constant => Color::rgb(0.92, 0.70, 0.38),
        HighlightKind::Variable => TEXT_COLOR,
        HighlightKind::Property => Color::rgb(0.52, 0.78, 0.86),
        HighlightKind::Constructor => Color::rgb(0.40, 0.72, 0.98),
        HighlightKind::Label => Color::rgb(0.52, 0.78, 0.86),
        HighlightKind::Escape => Color::rgb(0.92, 0.70, 0.38),
        HighlightKind::Attribute => Color::rgb(0.72, 0.62, 0.95),
        HighlightKind::Module => Color::rgb(0.32, 0.80, 0.72),
    }
}

/// A piece of a line, with the kind of text in it.
struct Piece {
    /// The byte range of the piece in the document.
    start: usize,
    end: usize,
    /// What the highlighter said about the text, if it said anything.
    kind: Option<HighlightKind>,
}

/// How bad a thing the language server found is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// A thing the language server found wrong in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// The byte range of the text the mark is under.
    pub range: Range<usize>,
    /// How bad it is.
    pub severity: MarkSeverity,
}

/// The color of the line under a thing the server found wrong.
fn mark_color(severity: MarkSeverity) -> Color {
    match severity {
        MarkSeverity::Error => Color::rgb(0.75, 0.10, 0.10),
        MarkSeverity::Warning => Color::rgb(0.85, 0.55, 0.10),
        MarkSeverity::Information => Color::rgb(0.20, 0.60, 0.85),
        MarkSeverity::Hint => Color::rgb(0.45, 0.70, 0.45),
    }
}

/// What the find bar leaves on the body of the editor.
pub struct FindOverlay<'a> {
    /// The place of the find bar.
    ///
    /// The find bar floats over the body, and the renderer draws all the
    /// text after all the blocks of colour, so the text of the body is left
    /// out where the bar is.
    pub bar: Option<Rect>,
    /// The matches, in the order of the text.
    pub matches: &'a [Range<TextOffset>],
    /// Which match is current, counted from the first one.
    pub current: Option<usize>,
}

/// Everything that is drawn on top of the text of the body.
pub struct BodyOverlay<'a> {
    /// What the find bar leaves on the body.
    pub find: FindOverlay<'a>,
    /// The highlights of the syntax, in the order of the text.
    pub highlights: &'a [Highlight],
    /// The things the language server found wrong.
    pub marks: &'a [Mark],
}

/// The presentation state of the editor body.
pub struct EditorView {
    /// Where the view is scrolled to.
    pub view: ViewState,
    /// The layout of the text that is on screen.
    layout: TextLayout,
    /// The offset the current mouse drag started at.
    drag_anchor: Option<usize>,
    /// The selection the current mouse drag is moving.
    drag_index: Option<usize>,
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
            drag_index: None,
            built: Built {
                version: u64::MAX,
                width: 0.0,
                height: 0.0,
                first_line: 0,
            },
        }
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

    /// Places the caret at a point and starts a drag from there.
    ///
    /// With `add` the caret is added to the ones that are already there
    /// instead of replacing them, which is what Ctrl+click does.
    pub fn press(&mut self, editor: &mut Editor, x: f32, y: f32, add: bool) {
        let Some(offset) = self.layout.hit_test(x, y) else {
            return;
        };
        let offset = TextOffset(offset);
        let index = if add {
            editor.add_cursor(offset)
        } else {
            editor.select_range(offset, offset);
            0
        };
        // The drag moves the head of the selection it started on.
        self.drag_index = Some(index);
        self.drag_anchor = Some(offset.0);
    }

    /// Extends the selection to a point.
    pub fn drag(&mut self, editor: &mut Editor, x: f32, y: f32) {
        let (Some(anchor), Some(index)) = (self.drag_anchor, self.drag_index) else {
            return;
        };
        let Some(offset) = self.layout.hit_test(x, y) else {
            return;
        };
        editor.select_range_at(index, TextOffset(anchor), TextOffset(offset));
    }

    /// Ends a mouse drag.
    pub fn release(&mut self) {
        self.drag_anchor = None;
        self.drag_index = None;
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
    /// Every caret moves. Returns `false` when a caret would leave the text
    /// that is on screen; the caller scrolls and tries again.
    pub fn move_vertical(&mut self, editor: &mut Editor, delta: isize) -> bool {
        let mut targets = Vec::new();
        for selection in editor.selections() {
            let caret = selection.head(editor.buffer()).0;
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
            targets.push(TextOffset(offset));
        }
        editor.set_carets(&targets);
        true
    }

    /// Adds a caret on the line above or below every caret.
    ///
    /// Returns `false` when no caret could move, which means there is nothing
    /// to add.
    pub fn add_cursor_vertically(&mut self, editor: &mut Editor, delta: isize) -> bool {
        let mut offsets = Vec::new();
        for selection in editor.selections() {
            let caret = selection.head(editor.buffer()).0;
            let Some(index) = self.layout.line_index_of(caret) else {
                continue;
            };
            let target = index as isize + delta;
            if target < 0 || target >= self.layout.lines.len() as isize {
                continue;
            }
            let x = self.layout.lines[index].offset_at(caret);
            let y = self.layout.lines[target as usize].y;
            let Some(offset) = self.layout.hit_test(x, y) else {
                continue;
            };
            offsets.push(TextOffset(offset));
        }
        if offsets.is_empty() {
            return false;
        }
        editor.add_cursors(&offsets);
        true
    }

    /// Extends every selection to the line above or below.
    pub fn extend_vertical(&mut self, editor: &mut Editor, delta: isize) -> bool {
        let mut heads = Vec::new();
        for selection in editor.selections() {
            let head = selection.head(editor.buffer()).0;
            let Some(index) = self.layout.line_index_of(head) else {
                return false;
            };
            let target = index as isize + delta;
            if target < 0 || target >= self.layout.lines.len() as isize {
                return false;
            }
            let x = self.layout.lines[index].offset_at(head);
            let y = self.layout.lines[target as usize].y;
            let Some(offset) = self.layout.hit_test(x, y) else {
                return false;
            };
            heads.push(TextOffset(offset));
        }
        editor.set_selection_heads(&heads);
        true
    }

    /// Extends every selection to the start or the end of its line.
    pub fn extend_to_line_edge(&mut self, editor: &mut Editor, to_end: bool) {
        let heads: Vec<TextOffset> = editor
            .selections()
            .iter()
            .map(|selection| {
                let head = selection.head(editor.buffer());
                let line = editor.buffer().line_of_offset(head.0);
                let range = editor.buffer().line_range(line);
                if to_end { range.end } else { range.start }
            })
            .collect();
        editor.set_selection_heads(&heads);
    }

    /// Moves the caret to the start or the end of its line.
    pub fn move_to_line_edge(&mut self, editor: &mut Editor, to_end: bool) {
        let carets: Vec<TextOffset> = editor
            .selections()
            .iter()
            .map(|selection| {
                let caret = selection.head(editor.buffer());
                let line = editor.buffer().line_of_offset(caret.0);
                let range = editor.buffer().line_range(line);
                if to_end { range.end } else { range.start }
            })
            .collect();
        editor.set_carets(&carets);
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
        self.view
            .ensure_line_visible(line, visible_lines, total_lines);
    }

    /// The byte range of the text that is on screen.
    ///
    /// The highlighter only looks at the text that is on screen, so this is
    /// the range that it is asked about.
    pub fn visible_range(&self) -> Range<usize> {
        match (self.layout.lines.first(), self.layout.lines.last()) {
            (Some(first), Some(last)) => first.range.start..last.range.end,
            _ => 0..0,
        }
    }

    /// The place of a byte offset on screen, in the coordinates of the body.
    ///
    /// Returns nothing when the offset is not on screen.
    pub fn point_of(&self, offset: usize) -> Option<(f32, f32)> {
        self.layout.offset_point(offset)
    }

    /// Draws the body: the selection, the text and the caret.
    ///
    /// The matches that the find bar found are drawn under the selection, so
    /// the selection of the user still shows on top of them.
    pub fn draw(
        &self,
        scene: &mut FrameScene,
        editor: &Editor,
        preedit: &str,
        body: Rect,
        overlay: BodyOverlay<'_>,
        measurer: &mut dyn TextMeasurer,
    ) {
        let clip = body;
        let find = overlay.find;

        for (index, range) in find.matches.iter().enumerate() {
            let color = if Some(index) == find.current {
                CURRENT_MATCH_COLOR
            } else {
                MATCH_COLOR
            };
            for rect in selection_rects(&self.layout, range.start.0..range.end.0) {
                scene.push_quad(
                    Rect::new(
                        body.origin.x + rect.x,
                        body.origin.y + rect.y,
                        rect.width,
                        rect.height,
                    ),
                    color,
                    clip,
                );
            }
        }

        for selection in editor.selections() {
            let range = selection.range(editor.buffer());
            for rect in selection_rects(&self.layout, range.start.0..range.end.0) {
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
        }

        // The things the server found wrong are drawn under the text, so the
        // text stays readable.
        for mark in overlay.marks {
            let rects = selection_rects(&self.layout, mark.range.start..mark.range.end);
            if rects.is_empty() {
                // A mark with no text under it (a missing semicolon, say)
                // still has to be seen: it gets a bar of its own.
                if let Some(point) = self.layout.offset_point(mark.range.start) {
                    scene.push_quad(
                        Rect::new(
                            body.origin.x + point.0,
                            body.origin.y + point.1 + LINE_HEIGHT - 2.0,
                            4.0,
                            2.0,
                        ),
                        mark_color(mark.severity),
                        clip,
                    );
                }
                continue;
            }
            for rect in rects {
                scene.push_quad(
                    Rect::new(
                        body.origin.x + rect.x,
                        body.origin.y + rect.y + rect.height - 2.0,
                        rect.width,
                        2.0,
                    ),
                    mark_color(mark.severity),
                    clip,
                );
            }
        }

        let caret = editor.caret().0;
        for line in &self.layout.lines {
            let y = body.origin.y + line.y;
            // A line that runs under the floating part stops at its edge.
            let line_clip = match find.bar {
                Some(bar) if y + LINE_HEIGHT > bar.origin.y && y < bar.max_y() => Rect::new(
                    body.origin.x,
                    body.origin.y,
                    (bar.origin.x - body.origin.x).max(1.0),
                    body.size.height,
                ),
                _ => clip,
            };
            self.push_line(
                scene,
                line,
                overlay.highlights,
                body,
                y,
                line_clip,
                caret,
                preedit,
                measurer,
            );
        }

        for selection in editor.selections() {
            let caret = selection.head(editor.buffer()).0;
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
    }

    /// Draws one line of text, in the pieces that the highlighter found.
    #[allow(clippy::too_many_arguments)]
    fn push_line(
        &self,
        scene: &mut FrameScene,
        line: &VisualLine,
        highlights: &[Highlight],
        body: Rect,
        y: f32,
        clip: Rect,
        caret: usize,
        preedit: &str,
        measurer: &mut dyn TextMeasurer,
    ) {
        // The composition belongs between the text before the caret and the
        // text after it, so the line is split there.
        let split = self
            .preedit_split(line, caret, preedit)
            .map(|at| line.range.start + at);
        let pieces = pieces_of(line, highlights, split);
        let width = match split {
            Some(_) => measurer.measure(preedit, FONT_SIZE).width,
            None => 0.0,
        };
        for piece in pieces {
            let mut x = body.origin.x + line.offset_at(piece.start);
            // The text after the composition is pushed to the right by it.
            if let Some(split) = split
                && piece.start >= split
            {
                x += width;
            }
            push_piece(scene, line, &piece, x, y, body, clip);
        }
        if let Some(split) = split {
            push_text(
                scene,
                preedit,
                body.origin.x + line.offset_at(split),
                y,
                body,
                clip,
                TEXT_COLOR,
            );
        }
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

/// The pieces of a line, with the kind of text in each.
///
/// The pieces cover the whole line: the text that the highlighter said
/// nothing about is in pieces of its own. The line is also split at the
/// position of the composition, so that the text after it can be pushed to
/// the right.
fn pieces_of(line: &VisualLine, highlights: &[Highlight], split: Option<usize>) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut cursor = line.range.start;
    for highlight in highlights {
        if highlight.range.end <= cursor {
            continue;
        }
        if highlight.range.start >= line.range.end {
            break;
        }
        let start = highlight.range.start.max(line.range.start);
        let end = highlight.range.end.min(line.range.end);
        if start > cursor {
            pieces.push(Piece {
                start: cursor,
                end: start,
                kind: None,
            });
        }
        pieces.push(Piece {
            start,
            end,
            kind: Some(highlight.kind),
        });
        cursor = end;
    }
    if cursor < line.range.end {
        pieces.push(Piece {
            start: cursor,
            end: line.range.end,
            kind: None,
        });
    }

    let Some(split) = split else {
        return pieces;
    };
    let mut split_pieces: Vec<Piece> = Vec::with_capacity(pieces.len() + 1);
    for piece in pieces {
        if piece.start < split && split < piece.end {
            split_pieces.push(Piece {
                end: split,
                ..piece
            });
            split_pieces.push(Piece {
                start: split,
                ..piece
            });
        } else {
            split_pieces.push(piece);
        }
    }
    split_pieces
}

/// Draws one piece of a line.
fn push_piece(
    scene: &mut FrameScene,
    line: &VisualLine,
    piece: &Piece,
    x: f32,
    y: f32,
    body: Rect,
    clip: Rect,
) {
    let text = &line.text[piece.start - line.range.start..piece.end - line.range.start];
    let color = piece.kind.map(highlight_color).unwrap_or(TEXT_COLOR);
    push_text(scene, text, x, y, body, clip, color);
}

/// Draws a run of text.
fn push_text(
    scene: &mut FrameScene,
    text: &str,
    x: f32,
    y: f32,
    body: Rect,
    clip: Rect,
    color: Color,
) {
    // The run is wider than the body so the renderer does not wrap the
    // text a second time: the body already wrapped it.
    scene.push_text(
        text,
        Rect::new(x, y, body.size.width * 2.0, LINE_HEIGHT),
        TextStyle {
            clip,
            color,
            font_size: FONT_SIZE,
            line_height: LINE_HEIGHT,
            family: TextFamily::Monospace,
        },
    );
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

/// Measures text with a shaper, without a renderer.
///
/// The real font stack does not need a GPU, so the measurements can be taken
/// without a window — the benchmarks need exactly those numbers.
pub struct ShaperMeasurer<'a> {
    shaper: &'a mut TextShaper,
    family: TextFamily,
}

impl<'a> ShaperMeasurer<'a> {
    pub fn new(shaper: &'a mut TextShaper, family: TextFamily) -> Self {
        ShaperMeasurer { shaper, family }
    }
}

impl TextMeasurer for ShaperMeasurer<'_> {
    fn measure(&mut self, text: &str, font_size: f32) -> LineMetrics {
        let measured = self.shaper.measure(text, font_size, self.family);
        LineMetrics {
            boundaries: measured.boundaries,
            width: measured.width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_syntax::{Language, Syntax};
    use codeweaver_ui::MonospaceMeasurer;

    fn view_of(text: &str) -> (EditorView, Editor) {
        let editor = Editor::new(text);
        let mut view = EditorView::new();
        let mut measurer = MonospaceMeasurer::new(10.0);
        view.layout(editor.buffer(), 1000.0, 200.0, &mut measurer);
        (view, editor)
    }

    /// The overlay of a body that has nothing on it.
    fn no_overlay(highlights: &[Highlight]) -> BodyOverlay<'_> {
        BodyOverlay {
            find: FindOverlay {
                bar: None,
                matches: &[],
                current: None,
            },
            highlights,
            marks: &[],
        }
    }

    #[test]
    fn a_click_places_the_caret_at_the_closest_character() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        view.press(&mut editor, 25.0, 0.0, false);
        assert_eq!(editor.caret().0, 3);
        assert!(editor.selection().range(editor.buffer()).is_empty());
    }

    #[test]
    fn a_control_click_adds_a_caret() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        view.press(&mut editor, 0.0, 0.0, false);
        // The second line starts at y = 22, the height of a line.
        view.press(&mut editor, 25.0, 25.0, true);
        assert_eq!(editor.cursor_count(), 2);
        assert_eq!(editor.carets(), vec![TextOffset(0), TextOffset(7)]);
    }

    #[test]
    fn a_click_without_control_replaces_the_carets() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        view.press(&mut editor, 0.0, 0.0, false);
        view.press(&mut editor, 25.0, 25.0, true);
        view.press(&mut editor, 0.0, 0.0, false);
        assert_eq!(editor.cursor_count(), 1);
    }

    #[test]
    fn every_caret_is_drawn() {
        let (view, mut editor) = view_of("abc\ndef\n");
        editor.add_cursor(TextOffset(5));

        let mut scene = FrameScene::default();
        let mut measurer = MonospaceMeasurer::new(10.0);
        view.draw(
            &mut scene,
            &editor,
            "",
            Rect::new(0.0, 0.0, 1000.0, 200.0),
            no_overlay(&[]),
            &mut measurer,
        );

        let carets = scene
            .quads
            .iter()
            .filter(|quad| quad.color == CARET_COLOR)
            .count();
        assert_eq!(carets, 2, "both carets must be drawn");
    }

    #[test]
    fn every_selection_is_drawn() {
        let (view, mut editor) = view_of("abc\ndef\n");
        editor.select_range(TextOffset(0), TextOffset(2));
        editor.add_cursor(TextOffset(4));
        editor.select_range_at(1, TextOffset(4), TextOffset(7));

        let mut scene = FrameScene::default();
        let mut measurer = MonospaceMeasurer::new(10.0);
        view.draw(
            &mut scene,
            &editor,
            "",
            Rect::new(0.0, 0.0, 1000.0, 200.0),
            no_overlay(&[]),
            &mut measurer,
        );

        let selections = scene
            .quads
            .iter()
            .filter(|quad| quad.color == SELECTION_COLOR)
            .count();
        assert_eq!(selections, 2, "both selections must be drawn");
    }

    #[test]
    fn the_highlights_are_drawn_in_their_own_color() {
        let text = "fn main() {}\n";
        let (view, editor) = view_of(text);
        let mut syntax = Syntax::new();
        syntax.set_language(Some(Language::Rust));
        syntax.update(editor.buffer(), None);
        let highlights = syntax.highlights(editor.buffer(), 0..editor.buffer().len().0);
        assert!(!highlights.is_empty(), "the text should be highlighted");

        let mut scene = FrameScene::default();
        let mut measurer = MonospaceMeasurer::new(10.0);
        view.draw(
            &mut scene,
            &editor,
            "",
            Rect::new(0.0, 0.0, 1000.0, 200.0),
            no_overlay(highlights),
            &mut measurer,
        );

        let keyword = highlight_color(HighlightKind::Keyword);
        let keyword_runs = scene
            .glyph_runs
            .iter()
            .filter(|run| run.color == keyword)
            .count();
        assert_eq!(keyword_runs, 1, "the keyword should be drawn on its own");
        assert_eq!(
            scene.glyph_runs[0].text, "fn",
            "the keyword should be the first thing on the line"
        );
    }

    #[test]
    fn the_text_of_a_line_is_drawn_in_one_piece_when_there_is_no_highlight() {
        let (view, editor) = view_of("abc\ndef\n");
        let mut scene = FrameScene::default();
        let mut measurer = MonospaceMeasurer::new(10.0);
        view.draw(
            &mut scene,
            &editor,
            "",
            Rect::new(0.0, 0.0, 1000.0, 200.0),
            no_overlay(&[]),
            &mut measurer,
        );

        // The document ends with a line break, so the last line is empty and
        // nothing is drawn on it.
        assert_eq!(scene.glyph_runs.len(), 2, "one run per line with text");
        assert_eq!(scene.glyph_runs[0].text, "abc");
    }

    #[test]
    fn a_drag_selects_the_text_between_the_two_points() {
        let (mut view, mut editor) = view_of("abc\ndef\n");
        view.press(&mut editor, 0.0, 0.0, false);
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
