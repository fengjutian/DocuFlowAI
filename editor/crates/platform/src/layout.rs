//! Where the parts of the window are.
//!
//! The window is drawn and it is clicked, so the place of every part has to
//! be worked out the same way twice — once to draw it, once to hit test it.
//! That is why the layout lives here and not in the drawing code: a status
//! bar that is drawn at the bottom and clicked at the top is a status bar
//! nobody can click.
//!
//! The layout is pure: it takes the size of the window and says where
//! everything is. It knows nothing about the editor.

use codeweaver_render::{Rect, Size};

use crate::breadcrumbs;
use crate::minimap;

/// The width of the bar with the activities on the left.
pub const ACTIVITY_BAR_WIDTH: f32 = 48.0;
/// The width of the side bar.
pub const SIDEBAR_WIDTH: f32 = 220.0;
/// The height of the strip the tabs are drawn in.
pub const TAB_BAR_HEIGHT: f32 = 34.0;
/// The height of the status bar at the bottom of the window.
pub const STATUS_BAR_HEIGHT: f32 = 24.0;
/// The width of the strip the line numbers are drawn in.
pub const GUTTER_WIDTH: f32 = 56.0;

/// The space between the side bar and the text of the document.
const GAP: f32 = 16.0;

/// How wide the panel of the agent is, as a part of the window.
///
/// The panel is wide: the transcript of the agent is worth reading.
const AGENT_SHARE: f32 = 0.34;
/// The narrowest the panel of the agent may be.
const AGENT_MIN: f32 = 280.0;
/// The widest the panel of the agent may be.
const AGENT_MAX: f32 = 520.0;

/// Where the parts of the window are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowLayout {
    /// The size of the window.
    pub size: Size,
    /// The bar with the activities on the left.
    pub activity_bar: Rect,
    /// The side bar with the section that was picked.
    pub sidebar: Rect,
    /// The strip the tabs are drawn in.
    pub tab_bar: Rect,
    /// The row that says where the file that is open is.
    pub breadcrumbs: Rect,
    /// The strip the line numbers are drawn in.
    pub gutter: Rect,
    /// The text of the document.
    pub body: Rect,
    /// The strip with the map of the document.
    pub minimap: Rect,
    /// The status bar at the bottom of the window.
    pub status_bar: Rect,
    /// The panel of the agent on the right.
    pub agent_panel: Rect,
}

impl WindowLayout {
    /// Works out where everything is.
    ///
    /// `bottom` is the room that is kept at the bottom of the window for the
    /// output panel of a task or the panel of the debugger: the text of the
    /// document does not go under them, and they do not go under the status
    /// bar either.
    pub fn of(size: Size, bottom: f32, sidebar_open: bool) -> Self {
        let status_bar = Rect::new(
            0.0,
            (size.height - STATUS_BAR_HEIGHT).max(0.0),
            size.width,
            STATUS_BAR_HEIGHT,
        );
        // The agent panel stops at the status bar: the status bar is the
        // bottom of the window, not the bottom of the text.
        let above = (size.height - STATUS_BAR_HEIGHT).max(1.0);
        let agent_width = (size.width * AGENT_SHARE).clamp(AGENT_MIN, AGENT_MAX);
        let agent_panel = Rect::new(
            (size.width - agent_width).max(0.0),
            0.0,
            agent_width.min(size.width),
            above,
        );

        let activity_bar = Rect::new(0.0, 0.0, ACTIVITY_BAR_WIDTH, size.height);
        let sidebar_width = if sidebar_open { SIDEBAR_WIDTH } else { 0.0 };
        let sidebar = Rect::new(ACTIVITY_BAR_WIDTH, 0.0, sidebar_width, above);

        // The text of the document starts after the side bar and stops
        // before the panel of the agent.
        let left = ACTIVITY_BAR_WIDTH + sidebar_width + GAP;
        let right = (agent_panel.origin.x - GAP).max(left + 1.0);
        // The row that says where the file is is between the tabs and the
        // text: it is part of the window around the text, not part of it.
        let top = TAB_BAR_HEIGHT + breadcrumbs::HEIGHT;
        let height = (above - top - bottom).max(1.0);
        let gutter = Rect::new(left, top, GUTTER_WIDTH, height);
        // The map of the document is on the right of the text.
        let minimap = Rect::new(
            (right - minimap::WIDTH).max(left + GUTTER_WIDTH),
            top,
            minimap::WIDTH,
            height,
        );
        let body = Rect::new(
            left + GUTTER_WIDTH,
            top,
            (minimap.origin.x - minimap::GAP - left - GUTTER_WIDTH).max(1.0),
            height,
        );
        let tab_bar = Rect::new(
            ACTIVITY_BAR_WIDTH + sidebar_width,
            0.0,
            (right - ACTIVITY_BAR_WIDTH - sidebar_width).max(1.0),
            TAB_BAR_HEIGHT,
        );
        // The row of the path is as wide as the strip of tabs: both are the
        // head of the place the text is in.
        let breadcrumbs = Rect::new(
            ACTIVITY_BAR_WIDTH + sidebar_width,
            TAB_BAR_HEIGHT,
            (right - ACTIVITY_BAR_WIDTH - sidebar_width).max(1.0),
            breadcrumbs::HEIGHT,
        );

        WindowLayout {
            size,
            activity_bar,
            sidebar,
            tab_bar,
            breadcrumbs,
            gutter,
            body,
            minimap,
            status_bar,
            agent_panel,
        }
    }

    /// The area the file tree is drawn in: under the title of the section.
    pub fn sidebar_content(&self) -> Rect {
        Rect::new(
            self.sidebar.origin.x,
            self.sidebar.origin.y + TITLE_HEIGHT,
            self.sidebar.size.width,
            (self.sidebar.size.height - TITLE_HEIGHT).max(1.0),
        )
    }
}

/// The height of the title of a section of the side bar.
pub const TITLE_HEIGHT: f32 = 28.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// A window of the size a window usually has.
    fn size() -> Size {
        Size {
            width: 1280.0,
            height: 800.0,
        }
    }

    #[test]
    fn the_parts_of_the_window_do_not_lie_on_top_of_each_other() {
        let layout = WindowLayout::of(size(), 0.0, true);
        assert!(layout.activity_bar.max_x() <= layout.sidebar.origin.x);
        assert!(layout.sidebar.max_x() <= layout.gutter.origin.x);
        assert!(layout.gutter.max_x() <= layout.body.origin.x);
        assert!(layout.body.max_x() <= layout.agent_panel.origin.x);
        assert!(layout.tab_bar.max_y() <= layout.body.origin.y);
        assert!(layout.body.max_y() <= layout.status_bar.origin.y);
    }

    #[test]
    fn the_status_bar_is_at_the_bottom_of_the_window() {
        let layout = WindowLayout::of(size(), 0.0, true);
        assert_eq!(layout.status_bar.max_y(), 800.0);
        assert_eq!(layout.status_bar.origin.x, 0.0);
        assert_eq!(layout.status_bar.size.width, 1280.0);
    }

    #[test]
    fn the_text_of_the_document_starts_under_the_path() {
        let layout = WindowLayout::of(size(), 0.0, true);
        assert_eq!(layout.body.origin.y, TAB_BAR_HEIGHT + breadcrumbs::HEIGHT);
        assert_eq!(layout.gutter.origin.y, TAB_BAR_HEIGHT + breadcrumbs::HEIGHT);
    }

    #[test]
    fn the_path_is_between_the_tabs_and_the_text() {
        let layout = WindowLayout::of(size(), 0.0, true);
        assert_eq!(layout.breadcrumbs.origin.y, TAB_BAR_HEIGHT);
        assert_eq!(layout.breadcrumbs.size.height, breadcrumbs::HEIGHT);
        assert!(layout.tab_bar.max_y() <= layout.breadcrumbs.origin.y);
        assert!(layout.breadcrumbs.max_y() <= layout.body.origin.y);
    }

    #[test]
    fn the_path_is_as_wide_as_the_strip_of_tabs() {
        let layout = WindowLayout::of(size(), 0.0, true);
        assert_eq!(layout.breadcrumbs.origin.x, layout.tab_bar.origin.x);
        assert_eq!(layout.breadcrumbs.size.width, layout.tab_bar.size.width);
    }

    #[test]
    fn the_line_numbers_are_on_the_left_of_the_text() {
        let layout = WindowLayout::of(size(), 0.0, true);
        assert_eq!(layout.gutter.size.width, GUTTER_WIDTH);
        assert_eq!(layout.body.origin.x, layout.gutter.max_x());
    }

    #[test]
    fn the_map_of_the_document_is_on_the_right_of_the_text() {
        let layout = WindowLayout::of(size(), 0.0, true);
        assert_eq!(layout.minimap.size.width, minimap::WIDTH);
        assert!(layout.body.max_x() <= layout.minimap.origin.x);
        assert!(layout.minimap.max_x() <= layout.agent_panel.origin.x);
    }

    #[test]
    fn a_closed_side_bar_gives_its_room_to_the_text() {
        let open = WindowLayout::of(size(), 0.0, true);
        let closed = WindowLayout::of(size(), 0.0, false);
        assert_eq!(closed.sidebar.size.width, 0.0);
        assert!(closed.body.size.width > open.body.size.width);
        assert!(closed.gutter.origin.x < open.gutter.origin.x);
    }

    #[test]
    fn the_panel_at_the_bottom_takes_the_room_of_the_text() {
        let without = WindowLayout::of(size(), 0.0, true);
        let with = WindowLayout::of(size(), 200.0, true);
        assert_eq!(with.body.size.height, without.body.size.height - 200.0);
    }

    #[test]
    fn the_panel_of_the_agent_stops_above_the_status_bar() {
        let layout = WindowLayout::of(size(), 0.0, true);
        assert_eq!(layout.agent_panel.max_y(), layout.status_bar.origin.y);
    }

    #[test]
    fn a_window_that_is_too_small_does_not_blow_up() {
        let tiny = Size {
            width: 10.0,
            height: 10.0,
        };
        let layout = WindowLayout::of(tiny, 200.0, true);
        assert!(layout.body.size.width >= 1.0);
        assert!(layout.body.size.height >= 1.0);
        assert!(layout.sidebar_content().size.height >= 1.0);
    }
}
