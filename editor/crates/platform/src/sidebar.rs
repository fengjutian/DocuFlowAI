//! The side bar: the section that the activity bar picked.
//!
//! Every activity has its own section, and the section shows what belongs to
//! it: the file tree, the hits of the last search, the files that git knows
//! have changed, the breakpoints of the file that is open.
//!
//! The component knows nothing about the editor: the rows are given to it,
//! and it says where they are and draws them. What a row does when it is
//! clicked is said in the row itself, so that the drawing and the clicking
//! cannot disagree.

use std::path::PathBuf;

use codeweaver_render::{Color, FrameScene, Point, Rect, TextFamily, TextStyle};
use codeweaver_ui::TextMeasurer;
use codeweaver_workspace::{NodeKind, TreeNode};

/// The height of the title of a section.
pub const TITLE_HEIGHT: f32 = 28.0;
/// The height of one row.
pub const ROW_HEIGHT: f32 = 22.0;
/// How much a level of the tree is indented.
pub const INDENT: f32 = 14.0;
/// The space on the left of the rows.
const PADDING: f32 = 8.0;

/// The color of the panel.
const PANEL_COLOR: Color = Color::rgb(0.10, 0.12, 0.16);
/// The color of the border on the right of the panel.
const BORDER_COLOR: Color = Color::rgb(0.20, 0.23, 0.29);
/// The color of the title of the section.
const TITLE_COLOR: Color = Color::rgb(0.72, 0.76, 0.84);
/// The color of the name of a directory.
const DIRECTORY_COLOR: Color = Color::rgb(0.76, 0.80, 0.88);
/// The color of the name of a file.
const FILE_COLOR: Color = Color::rgb(0.60, 0.64, 0.72);
/// The color of the text on the right of a row.
const DETAIL_COLOR: Color = Color::rgb(0.45, 0.49, 0.57);

/// What happens when a row is clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing happens: the row is there to be read.
    None,
    /// The file is opened.
    OpenFile(PathBuf),
    /// The file is opened at a line.
    OpenFileAt { path: PathBuf, line: usize },
    /// The breakpoint of a line is put in, or taken out.
    ToggleBreakpoint(u32),
    /// A directory of the tree is opened, or closed.
    ToggleDirectory(PathBuf),
}

/// One row of the side bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// What the row says.
    pub title: String,
    /// What is written on the right of it.
    pub detail: String,
    /// How deep the row is below the top of the section.
    pub depth: usize,
    /// Whether the row is a directory that can be opened.
    pub directory: bool,
    /// Whether the directory is open.
    pub expanded: bool,
    /// What happens when the row is clicked.
    pub action: Action,
    /// Where the row is drawn.
    pub rect: Rect,
}

impl Row {
    /// A row that says one thing.
    pub fn text(title: impl Into<String>, detail: impl Into<String>) -> Row {
        Row {
            title: title.into(),
            detail: detail.into(),
            depth: 0,
            directory: false,
            expanded: false,
            action: Action::None,
            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
        }
    }
}

/// The rows of the file tree, in the order they are drawn.
///
/// The rows that do not fit in the panel are left out: a row that is drawn
/// under the edge of the panel cannot be clicked.
pub fn tree_rows(nodes: &[TreeNode], panel: Rect) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut y = panel.origin.y;
    for node in nodes {
        if y + ROW_HEIGHT > panel.origin.y + panel.size.height {
            break;
        }
        rows.push(Row {
            title: node.name.clone(),
            detail: String::new(),
            depth: node.depth,
            directory: node.kind == NodeKind::Directory,
            expanded: node.expanded,
            action: if node.kind == NodeKind::Directory {
                Action::ToggleDirectory(node.path.clone())
            } else {
                Action::OpenFile(node.path.clone())
            },
            rect: Rect::new(panel.origin.x, y, panel.size.width, ROW_HEIGHT),
        });
        y += ROW_HEIGHT;
    }
    rows
}

/// The rows of a list of things, one under the other.
pub fn list_rows(panel: Rect, things: Vec<Row>) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut y = panel.origin.y;
    for mut thing in things {
        if y + ROW_HEIGHT > panel.origin.y + panel.size.height {
            break;
        }
        thing.rect = Rect::new(panel.origin.x, y, panel.size.width, ROW_HEIGHT);
        rows.push(thing);
        y += ROW_HEIGHT;
    }
    rows
}

/// The row that was clicked, if one was.
pub fn hit(rows: &[Row], point: Point) -> Option<&Row> {
    rows.iter().find(|row| row.rect.contains(point))
}

/// Draws the section: its title and its rows.
pub fn draw(
    scene: &mut FrameScene,
    viewport: Rect,
    panel: Rect,
    title: &str,
    rows: &[Row],
    measurer: &mut dyn TextMeasurer,
) {
    scene.push_quad(panel, PANEL_COLOR, viewport);
    scene.push_quad(
        Rect::new(
            panel.max_x() - 1.0,
            panel.origin.y,
            1.0,
            panel.size.height,
        ),
        BORDER_COLOR,
        viewport,
    );
    scene.push_quad(
        Rect::new(panel.origin.x, panel.origin.y, panel.size.width, TITLE_HEIGHT),
        Color::rgb(0.12, 0.14, 0.18),
        viewport,
    );
    scene.push_text(
        title,
        Rect::new(
            panel.origin.x + PADDING,
            panel.origin.y + 6.0,
            (panel.size.width - PADDING * 2.0).max(1.0),
            18.0,
        ),
        TextStyle {
            clip: viewport,
            color: TITLE_COLOR,
            font_size: 12.0,
            line_height: 16.0,
            family: TextFamily::SansSerif,
        },
    );

    for row in rows {
        let indent = panel.origin.x + PADDING + row.depth as f32 * INDENT;
        let marker = if row.directory {
            if row.expanded { "▾ " } else { "▸ " }
        } else {
            ""
        };
        let label = if marker.is_empty() {
            row.title.clone()
        } else {
            format!("{marker}{}", row.title)
        };
        scene.push_text(
            label,
            Rect::new(
                indent,
                row.rect.origin.y + 3.0,
                (panel.max_x() - indent - PADDING).max(1.0),
                18.0,
            ),
            TextStyle {
                clip: viewport,
                color: if row.directory {
                    DIRECTORY_COLOR
                } else {
                    FILE_COLOR
                },
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
        if !row.detail.is_empty() {
            let width = measurer.measure(&row.detail, 12.0).width;
            scene.push_text(
                row.detail.clone(),
                Rect::new(
                    panel.max_x() - PADDING - width,
                    row.rect.origin.y + 4.0,
                    width + 2.0,
                    16.0,
                ),
                TextStyle {
                    clip: viewport,
                    color: DETAIL_COLOR,
                    font_size: 12.0,
                    line_height: 16.0,
                    family: TextFamily::SansSerif,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel of a side bar.
    fn panel() -> Rect {
        Rect::new(48.0, 0.0, 220.0, 400.0)
    }

    /// A tree with one directory and one file.
    fn nodes() -> Vec<TreeNode> {
        vec![
            TreeNode {
                name: "src".to_owned(),
                path: PathBuf::from("src"),
                kind: NodeKind::Directory,
                children: Vec::new(),
                expanded: true,
                depth: 0,
            },
            TreeNode {
                name: "main.rs".to_owned(),
                path: PathBuf::from("src/main.rs"),
                kind: NodeKind::File,
                children: Vec::new(),
                expanded: false,
                depth: 1,
            },
        ]
    }

    #[test]
    fn the_rows_of_the_tree_are_under_each_other() {
        let rows = tree_rows(&nodes(), panel());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].rect.origin.y, panel().origin.y);
        assert_eq!(rows[1].rect.origin.y, panel().origin.y + ROW_HEIGHT);
    }

    #[test]
    fn a_directory_is_told_from_a_file() {
        let rows = tree_rows(&nodes(), panel());
        assert!(rows[0].directory);
        assert!(rows[0].expanded);
        assert!(!rows[1].directory);
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn the_rows_that_do_not_fit_are_left_out() {
        let small = Rect::new(48.0, 0.0, 220.0, 30.0);
        let rows = tree_rows(&nodes(), small);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn the_row_that_was_clicked_is_the_one_under_the_point() {
        let rows = tree_rows(&nodes(), panel());
        let first = rows[0].rect;
        assert_eq!(
            hit(&rows, Point { x: 60.0, y: first.origin.y + 4.0 }),
            Some(&rows[0])
        );
        assert_eq!(hit(&rows, Point { x: 60.0, y: 380.0 }), None);
    }

    #[test]
    fn clicking_a_row_of_the_tree_opens_the_file() {
        let rows = tree_rows(&nodes(), panel());
        assert_eq!(rows[1].action, Action::OpenFile(PathBuf::from("src/main.rs")));
    }
}
