//! The chrome around the text: the tab strip and the file tree.
//!
//! Both are drawn and clicked, so the place of a tab and the place of a file
//! have to be worked out the same way twice — once to draw, once to hit test.
//! That is why the layout lives here and not in the drawing code: a tab that
//! is drawn in one place and clicked in another is a tab that cannot be
//! clicked.

use std::path::{Path, PathBuf};

use codeweaver_render::{Point, Rect};
use codeweaver_ui::TextMeasurer;
use codeweaver_workbench::{Tab, TabId};

/// The height of one row of the file tree.
pub const ROW_HEIGHT: f32 = 22.0;
/// How much a level of the tree is indented.
pub const INDENT: f32 = 14.0;
/// The padding on both sides of the text of a tab.
pub const TAB_PADDING: f32 = 12.0;
/// The space a tab needs for the dot that says it has unsaved changes.
pub const DIRTY_SPACE: f32 = 12.0;

/// One tab of the strip, and where it is drawn.
#[derive(Clone, Debug)]
pub struct TabItem {
    pub id: TabId,
    pub title: String,
    /// Whether the tab has changes that are not on disk yet.
    pub dirty: bool,
    /// Whether the tab is the one on screen.
    pub active: bool,
    /// Where the tab is drawn.
    pub rect: Rect,
}

/// The tabs of the window, and where they are.
#[derive(Clone, Debug, Default)]
pub struct TabStrip {
    pub items: Vec<TabItem>,
}

impl TabStrip {
    /// Works out where every tab is drawn.
    ///
    /// The tabs are laid out from left to right and stop before the agent
    /// panel; a tab that does not fit is not drawn at all, because a strip
    /// that runs under the panel would hide the tabs behind it.
    pub fn layout(
        tabs: &[Tab],
        dirty: &[bool],
        active: usize,
        strip: Rect,
        measurer: &mut dyn TextMeasurer,
    ) -> TabStrip {
        let mut items = Vec::new();
        let mut x = strip.origin.x;
        for (index, tab) in tabs.iter().enumerate() {
            let width = TAB_PADDING * 2.0
                + measurer.measure(&tab.title, 13.0).width
                + if dirty.get(index).copied().unwrap_or(false) {
                    DIRTY_SPACE
                } else {
                    0.0
                };
            if x + width > strip.origin.x + strip.size.width {
                break;
            }
            items.push(TabItem {
                id: tab.id,
                title: tab.title.clone(),
                dirty: dirty.get(index).copied().unwrap_or(false),
                active: index == active,
                rect: Rect::new(x, strip.origin.y, width, strip.size.height),
            });
            x += width;
        }
        TabStrip { items }
    }

    /// The tab that was clicked, if one was.
    pub fn hit(&self, point: Point) -> Option<TabId> {
        self.items
            .iter()
            .find(|item| item.rect.contains(point))
            .map(|item| item.id)
    }
}

/// One row of the file tree.
#[derive(Clone, Debug)]
pub struct TreeRow {
    pub path: PathBuf,
    pub name: String,
    /// How deep the row is below the root.
    pub depth: usize,
    /// Whether the row is a directory.
    pub directory: bool,
    /// Whether the directory is open.
    pub expanded: bool,
    /// Where the row is drawn.
    pub rect: Rect,
}

/// The rows of the file tree, in the order they are drawn.
pub fn tree_rows(nodes: &[codeweaver_workspace::TreeNode], panel: Rect) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let mut y = panel.origin.y;
    for node in nodes {
        if y + ROW_HEIGHT > panel.origin.y + panel.size.height {
            break;
        }
        rows.push(TreeRow {
            path: node.path.clone(),
            name: node.name.clone(),
            depth: node.depth,
            directory: node.kind == codeweaver_workspace::NodeKind::Directory,
            expanded: node.expanded,
            rect: Rect::new(panel.origin.x, y, panel.size.width, ROW_HEIGHT),
        });
        y += ROW_HEIGHT;
    }
    rows
}

/// The row that was clicked, if one was.
pub fn tree_hit(rows: &[TreeRow], point: Point) -> Option<&TreeRow> {
    rows.iter().find(|row| row.rect.contains(point))
}

/// The name of a file, without its directory.
///
/// The tab shows the name of the file, not the path: a tab that shows the
/// whole path is a tab nobody can read at a glance.
pub fn title_of(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_owned())
        .unwrap_or_else(|| path.display().to_string())
}
