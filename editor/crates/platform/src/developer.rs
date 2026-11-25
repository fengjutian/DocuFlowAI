//! The panel that shows what the editor is doing inside.
//!
//! The editor says a lot about itself in the status line, and the status
//! line has room for one line. The things a person working on the editor
//! wants to see are the ones that do not fit there: every command of the
//! editor with its key, whether two commands want the same key, whether the
//! language server is there, and what the log says.
//!
//! The panel is a modal: it covers the window, the way the settings panel
//! does, and it takes the keys while it is open.

use codeweaver_render::{Color, FrameScene, Rect, Size, TextFamily, TextStyle};
use winit::keyboard::{Key, NamedKey};

/// The height of one line of the panel.
const ROW_HEIGHT: f32 = 22.0;
/// The room the title takes.
const HEADER: f32 = 46.0;
/// The room the hint takes.
const FOOTER: f32 = 32.0;
/// The room left over between the panel and the edges of the window.
const MARGIN: f32 = 40.0;

/// What a line of the panel says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// The name of a part of the panel.
    Section,
    /// A line of facts.
    Text,
    /// A line that says something is wrong.
    Warning,
    /// A line that says something is right.
    Good,
}

/// One line of the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// What the line says.
    pub text: String,
    /// How the line is drawn.
    pub kind: LineKind,
}

impl Line {
    /// A line of facts.
    pub fn text(text: impl Into<String>) -> Self {
        Line {
            text: text.into(),
            kind: LineKind::Text,
        }
    }

    /// The name of a part of the panel.
    pub fn section(text: impl Into<String>) -> Self {
        Line {
            text: text.into(),
            kind: LineKind::Section,
        }
    }

    /// A line that says something is wrong.
    pub fn warning(text: impl Into<String>) -> Self {
        Line {
            text: text.into(),
            kind: LineKind::Warning,
        }
    }

    /// A line that says something is right.
    pub fn good(text: impl Into<String>) -> Self {
        Line {
            text: text.into(),
            kind: LineKind::Good,
        }
    }
}

/// What the panel wants the editor to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeveloperCommand {
    /// Nothing.
    None,
    /// The panel is closed.
    Close,
}

/// The panel that shows what the editor is doing inside.
#[derive(Clone, Debug, Default)]
pub struct DeveloperPanel {
    /// The line that is at the top of the panel.
    scroll: usize,
}

impl DeveloperPanel {
    /// Opens the panel at the top.
    pub fn new() -> Self {
        DeveloperPanel { scroll: 0 }
    }

    /// The line that is at the top of the panel.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Moves the panel up or down.
    ///
    /// The panel does not wrap around: the text below the last line is
    /// nowhere, and a panel that jumps back to the top is a panel the person
    /// loses track of.
    pub fn scroll_by(&mut self, delta: isize, total: usize, visible: usize) {
        if total <= visible {
            self.scroll = 0;
            return;
        }
        let last = total - visible;
        let next = self.scroll as isize + delta;
        self.scroll = next.clamp(0, last as isize) as usize;
    }

    /// Gives a key to the panel.
    ///
    /// The panel takes every key while it is open: a key that is typed
    /// behind a panel is a key nobody sees.
    pub fn handle_key<T: AsRef<str>>(
        &mut self,
        key: &Key<T>,
        total: usize,
        visible: usize,
    ) -> DeveloperCommand {
        match key {
            Key::Named(NamedKey::Escape) => DeveloperCommand::Close,
            Key::Named(NamedKey::ArrowDown) => {
                self.scroll_by(1, total, visible);
                DeveloperCommand::None
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.scroll_by(-1, total, visible);
                DeveloperCommand::None
            }
            Key::Named(NamedKey::PageDown) => {
                self.scroll_by(visible as isize, total, visible);
                DeveloperCommand::None
            }
            Key::Named(NamedKey::PageUp) => {
                self.scroll_by(-(visible as isize), total, visible);
                DeveloperCommand::None
            }
            _ => DeveloperCommand::None,
        }
    }
}

/// How many lines are on screen at once.
///
/// The panel is as tall as the window allows: a panel that is taller than
/// the window has lines that cannot be read.
pub fn visible_rows(size: Size) -> usize {
    ((size.height - HEADER - FOOTER - MARGIN) / ROW_HEIGHT)
        .floor()
        .max(1.0) as usize
}

/// Draws the panel over the window.
///
/// The panel covers the window: it is what the person is looking at, and the
/// text behind it is not.
pub fn draw(
    scene: &mut FrameScene,
    viewport: Rect,
    size: Size,
    panel: &DeveloperPanel,
    lines: &[Line],
) {
    scene.push_quad(viewport, Color::rgb(0.02, 0.03, 0.05), viewport);

    let visible = visible_rows(size);
    let width = (size.width * 0.72).clamp(520.0, 960.0);
    let height = HEADER + visible as f32 * ROW_HEIGHT + FOOTER;
    let box_rect = Rect::new(
        (size.width - width) / 2.0,
        (size.height - height) / 2.0,
        width,
        height,
    );
    scene.push_quad(box_rect, Color::rgb(0.13, 0.15, 0.20), viewport);
    scene.push_quad(
        Rect::new(
            box_rect.origin.x,
            box_rect.origin.y,
            box_rect.size.width,
            3.0,
        ),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );
    scene.push_text(
        "开发者面板".to_owned(),
        Rect::new(
            box_rect.origin.x + 20.0,
            box_rect.origin.y + 14.0,
            box_rect.size.width - 40.0,
            24.0,
        ),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.90, 0.92, 0.96),
            font_size: 16.0,
            line_height: 22.0,
            family: TextFamily::SansSerif,
        },
    );

    // The lines that are on screen.
    let start = panel.scroll().min(lines.len());
    for (index, line) in lines.iter().skip(start).take(visible).enumerate() {
        let y = box_rect.origin.y + HEADER + index as f32 * ROW_HEIGHT;
        let color = match line.kind {
            LineKind::Section => Color::rgb(0.60, 0.65, 0.74),
            LineKind::Text => Color::rgb(0.82, 0.86, 0.92),
            LineKind::Warning => Color::rgb(0.95, 0.68, 0.25),
            LineKind::Good => Color::rgb(0.42, 0.78, 0.56),
        };
        let font_size = if line.kind == LineKind::Section {
            14.0
        } else {
            13.0
        };
        scene.push_text(
            line.text.clone(),
            Rect::new(
                box_rect.origin.x + 20.0,
                y + 2.0,
                box_rect.size.width - 40.0,
                ROW_HEIGHT,
            ),
            TextStyle {
                clip: viewport,
                color,
                font_size,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    }

    // The keys the panel takes are said at the bottom of it.
    scene.push_text(
        "↑↓ 翻页 · PgUp/PgDn 翻大页 · Esc 关掉".to_owned(),
        Rect::new(
            box_rect.origin.x + 20.0,
            box_rect.origin.y + box_rect.size.height - FOOTER + 4.0,
            box_rect.size.width - 40.0,
            22.0,
        ),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.52, 0.57, 0.66),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> DeveloperPanel {
        DeveloperPanel::new()
    }

    #[test]
    fn the_panel_starts_at_the_top() {
        assert_eq!(panel().scroll(), 0);
    }

    #[test]
    fn the_escape_key_closes_the_panel() {
        assert_eq!(
            panel().handle_key(&Key::<&str>::Named(NamedKey::Escape), 40, 20),
            DeveloperCommand::Close
        );
    }

    #[test]
    fn the_panel_scrolls_down_and_up() {
        let mut panel = panel();
        panel.scroll_by(1, 40, 20);
        assert_eq!(panel.scroll(), 1);
        panel.scroll_by(-1, 40, 20);
        assert_eq!(panel.scroll(), 0);
    }

    #[test]
    fn the_panel_does_not_scroll_past_the_end() {
        let mut panel = panel();
        panel.scroll_by(1000, 40, 20);
        assert_eq!(panel.scroll(), 20, "the last page is at the end");
        panel.scroll_by(-1000, 40, 20);
        assert_eq!(panel.scroll(), 0);
    }

    #[test]
    fn a_list_that_fits_on_one_screen_does_not_scroll() {
        let mut panel = panel();
        panel.scroll_by(5, 10, 20);
        assert_eq!(panel.scroll(), 0);
    }

    #[test]
    fn the_arrow_keys_move_the_panel() {
        let mut panel = panel();
        panel.handle_key(&Key::<&str>::Named(NamedKey::ArrowDown), 40, 20);
        assert_eq!(panel.scroll(), 1);
        panel.handle_key(&Key::<&str>::Named(NamedKey::PageDown), 40, 20);
        assert_eq!(panel.scroll(), 20);
        panel.handle_key(&Key::<&str>::Named(NamedKey::PageUp), 40, 20);
        assert_eq!(panel.scroll(), 0);
    }

    #[test]
    fn a_line_knows_how_it_is_drawn() {
        assert_eq!(Line::text("一件事").kind, LineKind::Text);
        assert_eq!(Line::section("一部分").kind, LineKind::Section);
        assert_eq!(Line::warning("有问题").kind, LineKind::Warning);
        assert_eq!(Line::good("没问题").kind, LineKind::Good);
    }

    #[test]
    fn the_panel_fits_in_the_window() {
        let size = Size::new(1200.0, 750.0);
        let visible = visible_rows(size);
        assert!(visible > 5, "the window shows more than five lines");
        // The panel is not taller than the window.
        let height = HEADER + visible as f32 * ROW_HEIGHT + FOOTER;
        assert!(height <= size.height, "the panel is {height} tall");
    }
}
