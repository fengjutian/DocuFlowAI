//! The box that asks which folder of a workspace goes.
//!
//! A workspace names several folders, and "take one of them out" cannot be
//! answered by a name that is already known: the box lists the folders of
//! the workspace and lets the person pick one.
//!
//! The box is a thing of its own: it has no window in it, and it does not
//! know what happens to the folder that is picked.

use std::path::PathBuf;

/// One folder of a workspace, as the box shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootItem {
    /// The name the folder is shown with.
    pub name: String,
    /// Where the folder is on disk.
    pub path: PathBuf,
}

/// The box that asks which folder of the workspace is taken out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceBox {
    /// The folders of the workspace, in the order the workspace has them.
    roots: Vec<RootItem>,
    /// Which one is picked, counted from the first one.
    selected: usize,
}

impl WorkspaceBox {
    /// Opens the box with the folders of a workspace.
    pub fn new(roots: Vec<RootItem>) -> Self {
        WorkspaceBox { roots, selected: 0 }
    }

    /// The folders the box shows.
    pub fn roots(&self) -> &[RootItem] {
        &self.roots
    }

    /// Which one is picked, counted from the first one.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The one that is picked.
    pub fn selected_root(&self) -> Option<&RootItem> {
        self.roots.get(self.selected)
    }

    /// Picks the one after this one.
    ///
    /// The last one is the end of the list: there is nothing after it.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.roots.len() {
            self.selected += 1;
        }
    }

    /// Picks the one before this one.
    ///
    /// The first one is the beginning of the list: there is nothing before
    /// it.
    pub fn select_previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box with two folders in it.
    fn a_box() -> WorkspaceBox {
        WorkspaceBox::new(vec![
            RootItem {
                name: "editor".to_owned(),
                path: PathBuf::from("C:/work/editor"),
            },
            RootItem {
                name: "console".to_owned(),
                path: PathBuf::from("C:/work/console"),
            },
        ])
    }

    #[test]
    fn a_new_box_picks_the_first_folder() {
        let box_ = a_box();
        assert_eq!(box_.selected(), 0);
        assert_eq!(box_.selected_root().map(|root| root.name.as_str()), Some("editor"));
    }

    #[test]
    fn the_folder_after_the_one_that_is_picked_is_picked() {
        let mut box_ = a_box();
        box_.select_next();
        assert_eq!(box_.selected_root().map(|root| root.name.as_str()), Some("console"));
    }

    #[test]
    fn the_folder_before_the_one_that_is_picked_is_picked() {
        let mut box_ = a_box();
        box_.select_next();
        box_.select_previous();
        assert_eq!(box_.selected_root().map(|root| root.name.as_str()), Some("editor"));
    }

    #[test]
    fn the_end_of_the_list_is_the_end() {
        let mut box_ = a_box();
        box_.select_next();
        box_.select_next();
        assert_eq!(box_.selected(), 1, "there is no folder after the last one");
        box_.select_previous();
        box_.select_previous();
        assert_eq!(box_.selected(), 0, "there is no folder before the first one");
    }

    #[test]
    fn a_box_with_one_folder_in_it_stays_where_it_is() {
        let mut box_ = WorkspaceBox::new(vec![RootItem {
            name: "editor".to_owned(),
            path: PathBuf::from("C:/work/editor"),
        }]);
        box_.select_previous();
        assert_eq!(box_.selected(), 0);
        box_.select_next();
        assert_eq!(box_.selected(), 0);
    }

    #[test]
    fn a_box_with_nothing_in_it_picks_nothing() {
        let box_ = WorkspaceBox::new(Vec::new());
        assert!(box_.selected_root().is_none());
        assert!(box_.roots().is_empty());
    }
}
