//! Where the parts of the window are.
//!
//! The window is drawn and it is clicked, so the place of every part has to be
//! worked out the same way twice — once to draw it, once to hit test it.
//! That is why the layout lives here and not in the drawing code: a status
//! bar that is drawn at the bottom and clicked at the top is a status bar
//! nobody can click.
//!
//! The layout is pure: it takes the size of the window and says where
//! everything is. It knows nothing about the editor.
//!
//! A window with one column shows one file. A window that is split shows two
//! (or more) columns side by side, and **each column has its own parts**: its
//! own strip of tabs, its own row that says where the file is, its own line
//! numbers, its own text and its own map. That is why the parts of a column
//! live in [`ColumnLayout`] and not in [`WindowLayout`].

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

/// The line between two columns.
pub const SEPARATOR: f32 = 1.0;

/// How wide a column has to be before it gets a map of the document.
///
/// The map is 90 pixels wide and the line numbers 56: in a narrow column the
/// map would take the room the text needs. The text comes first.
pub const MINIMAP_MIN_WIDTH: f32 = 360.0;

/// How wide the panel of the agent is, as a part of the window.
///
/// The panel is wide: the transcript of the agent is worth reading.
const AGENT_SHARE: f32 = 0.34;
/// The narrowest the panel of the agent may be.
const AGENT_MIN: f32 = 280.0;
/// The widest the panel of the agent may be.
const AGENT_MAX: f32 = 520.0;

/// One column of the window: the parts that hold the text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnLayout {
    /// The whole column: from the top of the window to the status bar.
    ///
    /// It is what a click in the column lands in, whether the click is on the
    /// text, on the line numbers or on the strip of tabs.
    pub column: Rect,
    /// The strip the tabs are drawn in.
    pub tab_bar: Rect,
    /// The row that says where the file that is open is.
    pub breadcrumbs: Rect,
    /// The strip the line numbers are drawn in.
    pub gutter: Rect,
    /// The text of the document.
    pub body: Rect,
    /// The strip with the map of the document.
    ///
    /// It is `None` in a column that is too narrow for it: the text of the
    /// document is worth more than the map of it.
    pub minimap: Option<Rect>,
}

impl ColumnLayout {
    /// The column in one place.
    ///
    /// The strip of tabs starts where the side bar ends; the text of the
    /// document starts a gap further to the right, after the line numbers.
    fn of(x: f32, width: f32, top: f32, height: f32, window_height: f32) -> ColumnLayout {
        let gutter = Rect::new(x + GAP, top, GUTTER_WIDTH, height);
        let (body, minimap) = if width >= MINIMAP_MIN_WIDTH {
            let minimap = Rect::new(
                x + width - minimap::WIDTH,
                top,
                minimap::WIDTH,
                height,
            );
            let body = Rect::new(
                gutter.max_x(),
                top,
                (minimap.origin.x - minimap::GAP - gutter.max_x()).max(1.0),
                height,
            );
            (body, Some(minimap))
        } else {
            (
                Rect::new(
                    gutter.max_x(),
                    top,
                    (x + width - gutter.max_x()).max(1.0),
                    height,
                ),
                None,
            )
        };
        ColumnLayout {
            column: Rect::new(x, 0.0, width, window_height.max(height + top)),
            tab_bar: Rect::new(x, 0.0, width, TAB_BAR_HEIGHT),
            breadcrumbs: Rect::new(x, TAB_BAR_HEIGHT, width, breadcrumbs::HEIGHT),
            gutter,
            body,
            minimap,
        }
    }
}

/// Where the parts of the window are.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowLayout {
    /// The size of the window.
    pub size: Size,
    /// The bar with the activities on the left.
    pub activity_bar: Rect,
    /// The side bar with the section that was picked.
    pub sidebar: Rect,
    /// The status bar at the bottom of the window.
    pub status_bar: Rect,
    /// The panel of the agent on the right.
    pub agent_panel: Rect,
    /// The columns of the window, from left to right.
    pub columns: Vec<ColumnLayout>,
}

impl WindowLayout {
    /// Works out where everything is.
    ///
    /// `bottom` is the room that is kept at the bottom of the window for the
    /// output panel of a task or the panel of the debugger: the text of the
    /// document does not go under them, and they do not go under the status
    /// bar either.
    ///
    /// `columns` says how many files are on screen at the same time. One
    /// column is the whole width of the window; two columns are half of it
    /// each, with a line between them.
    pub fn of(size: Size, bottom: f32, sidebar_open: bool, columns: usize) -> Self {
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
        let edge = ACTIVITY_BAR_WIDTH + sidebar_width;
        let left = edge + GAP;
        let right = (agent_panel.origin.x - GAP).max(left + 1.0);
        // The row that says where the file is is between the tabs and the
        // text: it is part of the window around the text, not part of it.
        let top = TAB_BAR_HEIGHT + breadcrumbs::HEIGHT;
        let height = (above - top - bottom).max(1.0);

        // The columns share the room between the side bar and the panel of
        // the agent, with a line between each two of them.
        let count = columns.max(1);
        let available = (right - edge).max(1.0);
        let separators = SEPARATOR * (count - 1) as f32;
        let width = ((available - separators) / count as f32).max(1.0);
        let mut laid_out = Vec::with_capacity(count);
        for index in 0..count {
            let x = edge + index as f32 * (width + SEPARATOR);
            laid_out.push(ColumnLayout::of(
                x,
                width,
                top,
                height,
                (size.height - STATUS_BAR_HEIGHT).max(1.0),
            ));
        }

        WindowLayout {
            size,
            activity_bar,
            sidebar,
            status_bar,
            agent_panel,
            columns: laid_out,
        }
    }

    /// The column that is active.
    ///
    /// The window with one column has only that one; a window that is split
    /// says which of its columns has the keyboard.
    pub fn column(&self, active: usize) -> Option<&ColumnLayout> {
        self.columns.get(active)
    }

    /// The column the point is in.
    pub fn column_at(&self, point: codeweaver_render::Point) -> Option<usize> {
        self.columns
            .iter()
            .position(|column| column.column.contains(point))
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
        let layout = WindowLayout::of(size(), 0.0, true, 1);
        let column = layout.column(0).expect("there is a column");
        assert!(layout.activity_bar.max_x() <= layout.sidebar.origin.x);
        assert!(layout.sidebar.max_x() <= column.gutter.origin.x);
        assert!(column.gutter.max_x() <= column.body.origin.x);
        assert!(column.body.max_x() <= layout.agent_panel.origin.x);
        assert!(column.tab_bar.max_y() <= column.body.origin.y);
        assert!(column.body.max_y() <= layout.status_bar.origin.y);
    }

    #[test]
    fn the_status_bar_is_at_the_bottom_of_the_window() {
        let layout = WindowLayout::of(size(), 0.0, true, 1);
        assert_eq!(layout.status_bar.max_y(), 800.0);
        assert_eq!(layout.status_bar.origin.x, 0.0);
        assert_eq!(layout.status_bar.size.width, 1280.0);
    }

    #[test]
    fn the_text_of_the_document_starts_under_the_path() {
        let layout = WindowLayout::of(size(), 0.0, true, 1);
        let column = layout.column(0).expect("there is a column");
        assert_eq!(column.body.origin.y, TAB_BAR_HEIGHT + breadcrumbs::HEIGHT);
        assert_eq!(column.gutter.origin.y, TAB_BAR_HEIGHT + breadcrumbs::HEIGHT);
    }

    #[test]
    fn the_path_is_between_the_tabs_and_the_text() {
        let layout = WindowLayout::of(size(), 0.0, true, 1);
        let column = layout.column(0).expect("there is a column");
        assert_eq!(column.breadcrumbs.origin.y, TAB_BAR_HEIGHT);
        assert_eq!(column.breadcrumbs.size.height, breadcrumbs::HEIGHT);
        assert!(column.tab_bar.max_y() <= column.breadcrumbs.origin.y);
        assert!(column.breadcrumbs.max_y() <= column.body.origin.y);
    }

    #[test]
    fn the_path_is_as_wide_as_the_strip_of_tabs() {
        let layout = WindowLayout::of(size(), 0.0, true, 1);
        let column = layout.column(0).expect("there is a column");
        assert_eq!(column.breadcrumbs.origin.x, column.tab_bar.origin.x);
        assert_eq!(column.breadcrumbs.size.width, column.tab_bar.size.width);
    }

    #[test]
    fn the_line_numbers_are_on_the_left_of_the_text() {
        let layout = WindowLayout::of(size(), 0.0, true, 1);
        let column = layout.column(0).expect("there is a column");
        assert_eq!(column.gutter.size.width, GUTTER_WIDTH);
        assert_eq!(column.body.origin.x, column.gutter.max_x());
    }

    #[test]
    fn the_map_of_the_document_is_on_the_right_of_the_text() {
        let layout = WindowLayout::of(size(), 0.0, true, 1);
        let column = layout.column(0).expect("there is a column");
        let minimap = column.minimap.expect("a wide column has a map");
        assert_eq!(minimap.size.width, minimap::WIDTH);
        assert!(column.body.max_x() <= minimap.origin.x);
        assert!(minimap.max_x() <= layout.agent_panel.origin.x);
    }

    #[test]
    fn a_closed_side_bar_gives_its_room_to_the_text() {
        let open = WindowLayout::of(size(), 0.0, true, 1);
        let closed = WindowLayout::of(size(), 0.0, false, 1);
        assert!(closed.column(0).expect("there is a column").body.size.width > open.column(0).expect("there is a column").body.size.width);
        assert!(closed.column(0).expect("there is a column").gutter.origin.x < open.column(0).expect("there is a column").gutter.origin.x);
    }

    #[test]
    fn the_panel_at_the_bottom_takes_the_room_of_the_text() {
        let without = WindowLayout::of(size(), 0.0, true, 1);
        let with = WindowLayout::of(size(), 200.0, true, 1);
        assert_eq!(
            with.column(0).expect("there is a column").body.size.height,
            without.column(0).expect("there is a column").body.size.height - 200.0
        );
    }

    #[test]
    fn the_panel_of_the_agent_stops_above_the_status_bar() {
        let layout = WindowLayout::of(size(), 0.0, true, 1);
        assert_eq!(
            layout.agent_panel.max_y(),
            layout.status_bar.origin.y
        );
    }

    #[test]
    fn a_window_that_is_too_small_does_not_blow_up() {
        let tiny = Size {
            width: 10.0,
            height: 10.0,
        };
        let layout = WindowLayout::of(tiny, 200.0, true, 1);
        let column = layout.column(0).expect("there is a column");
        assert!(column.body.size.width >= 1.0);
        assert!(column.body.size.height >= 1.0);
        assert!(layout.sidebar_content().size.height >= 1.0);
    }

    #[test]
    fn two_columns_share_the_room_between_them() {
        let layout = WindowLayout::of(size(), 0.0, true, 2);
        assert_eq!(layout.columns.len(), 2);
        let left = layout.column(0).expect("there is a column");
        let right = layout.column(1).expect("there is a column");
        // The right column starts where the left one ends, with a line
        // between them.
        assert_eq!(
            right.tab_bar.origin.x - left.tab_bar.max_x(),
            SEPARATOR
        );
        assert_eq!(left.gutter.size.width, right.gutter.size.width);
        assert_eq!(left.body.size.height, right.body.size.height);
        assert_eq!(left.body.size.width, right.body.size.width);
    }

    #[test]
    fn the_text_of_the_two_columns_does_not_lie_on_top_of_each_other() {
        let layout = WindowLayout::of(size(), 0.0, true, 2);
        let left = layout.column(0).expect("there is a column");
        let right = layout.column(1).expect("there is a column");
        assert!(left.body.max_x() <= right.body.origin.x);
        assert!(left.tab_bar.max_x() <= right.tab_bar.origin.x);
    }

    #[test]
    fn a_narrow_column_has_no_map_of_the_document() {
        let layout = WindowLayout::of(size(), 0.0, true, 2);
        let right = layout.column(1).expect("there is a column");
        // The room of the map goes to the text instead.
        assert!(right.minimap.is_none());
    }

    #[test]
    fn the_column_the_point_is_in_is_the_one_that_is_under_it() {
        let layout = WindowLayout::of(size(), 0.0, true, 2);
        let right = layout.column(1).expect("there is a column");
        let point = codeweaver_render::Point {
            x: right.body.origin.x + 10.0,
            y: right.body.origin.y + 10.0,
        };
        assert_eq!(layout.column_at(point), Some(1));
    }
}
