//! The workspace file tree: the model behind the file panel.
//!
//! The tree is a snapshot of a directory tree on disk, read lazily: only the
//! directories that are open are read, and a directory is read again when it
//! is expanded or when [`FileTree::refresh`] is called. Opening a workspace
//! therefore does not walk it.
//!
//! Two separate rules decide what enters the tree:
//!
//! * The *ignored names* list. It holds directory names that never enter the
//!   tree, at any depth: `git`, `target`, `node_modules`, `dist`, `.venv`,
//!   `__pycache__` and `codeweaver-data` by default.
//! * The *hidden entries* rule. Every entry whose name starts with a dot is
//!   hidden by default, files as well as directories.
//!
//! Both rules are expressed by one list, so that a caller can replace them
//! together through [`FileTree::with_ignored`]. A plain entry is a directory
//! name that is skipped at any depth; the special entry `.*` stands for "every
//! name that starts with a dot". The default list is the seven directory names
//! above plus `.*`, so a list without `.*` shows hidden entries.
//!
//! The tree does not watch the file system: it is a snapshot that the caller
//! refreshes when it wants to see new files. Changes to files that are open
//! are reported by [`crate::watch`], which is a separate model.

use std::{
    cmp::Ordering,
    fs, io,
    path::{Path, PathBuf},
};

/// The directory names that never enter the tree by default.
const DEFAULT_IGNORED_NAMES: &[&str] = &[
    "git",
    "target",
    "node_modules",
    "dist",
    ".venv",
    "__pycache__",
    "codeweaver-data",
];

/// The ignore entry that stands for "every name that starts with a dot".
const HIDDEN_MARKER: &str = ".*";

/// What a tree node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
}

/// A node of the file tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub kind: NodeKind,
    /// The children of a directory; empty for a file. A directory that has not
    /// been expanded yet has no children.
    pub children: Vec<TreeNode>,
    /// Whether the directory is expanded. Always false for a file.
    pub expanded: bool,
    /// How deep the node is below the root; the root itself is 0.
    pub depth: usize,
}

/// The file tree of a workspace.
///
/// The tree starts with the root directory expanded, so the panel shows the
/// root's direct children right away. Everything below that is read from disk
/// when a directory is expanded.
#[derive(Clone, Debug)]
pub struct FileTree {
    /// The root node. It is always expanded and is never a row of the tree.
    root: TreeNode,
    /// The names that never enter the tree.
    ignored: Vec<String>,
    /// The rows of the tree, in the order they are drawn.
    visible: Vec<TreeNode>,
}

impl FileTree {
    /// A tree rooted at `root`, with the default ignore rules.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let mut ignored: Vec<String> = DEFAULT_IGNORED_NAMES
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        ignored.push(HIDDEN_MARKER.to_owned());
        Self::build(root.into(), ignored)
    }

    /// A tree rooted at `root`, with `ignored` as its ignore rules.
    ///
    /// The list replaces the default one; it is not added to it. A plain entry
    /// is a directory name that is skipped at any depth, and the special entry
    /// `.*` hides every name that starts with a dot. The default list is the
    /// seven directory names plus `.*`, so a list without `.*` shows hidden
    /// entries.
    pub fn with_ignored(root: impl Into<PathBuf>, ignored: Vec<String>) -> Self {
        Self::build(root.into(), ignored)
    }

    /// Builds a tree whose root is expanded and already read.
    fn build(root: PathBuf, ignored: Vec<String>) -> Self {
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        let mut node = TreeNode {
            name,
            path: root.clone(),
            kind: NodeKind::Directory,
            children: Vec::new(),
            expanded: true,
            depth: 0,
        };
        // A root that cannot be read is not an error here: the tree starts
        // empty and fills in once the directory shows up.
        node.children = read_children(&root, 1, &ignored).unwrap_or_default();

        let mut tree = FileTree {
            root: node,
            ignored,
            visible: Vec::new(),
        };
        tree.rebuild();
        tree
    }

    /// The root directory of the tree.
    pub fn root(&self) -> &Path {
        &self.root.path
    }

    /// The visible nodes, in the order they are drawn: a directory comes
    /// before its children, and the children of a directory that is not
    /// expanded are not in the list at all.
    ///
    /// The root itself is not in the list; the list holds the root's children
    /// and their descendants.
    pub fn nodes(&self) -> &[TreeNode] {
        &self.visible
    }

    /// Expands a collapsed directory or collapses an expanded one.
    /// Returns true when the tree changed.
    ///
    /// Expanding reads the directory from disk, so a directory that cannot be
    /// read is left alone and reported as "no change". The root is always
    /// expanded and cannot be collapsed; a file cannot be expanded at all.
    pub fn toggle(&mut self, path: &Path) -> bool {
        if same_path(&self.root.path, path) {
            return false;
        }

        let ignored = &self.ignored;
        let changed = match find_visible(&mut self.root, path, true) {
            None => false,
            Some(node) => {
                if node.kind != NodeKind::Directory {
                    false
                } else if node.expanded {
                    node.expanded = false;
                    true
                } else {
                    match read_children(&node.path, node.depth + 1, ignored) {
                        Ok(children) => {
                            node.children = children;
                            node.expanded = true;
                            true
                        }
                        Err(_) => false,
                    }
                }
            }
        };

        if changed {
            self.rebuild();
        }
        changed
    }

    /// Whether the path is an expanded directory.
    pub fn is_expanded(&self, path: &Path) -> bool {
        if same_path(&self.root.path, path) {
            return true;
        }
        find_node(&self.root, path)
            .map(|node| node.expanded)
            .unwrap_or(false)
    }

    /// Re-reads the directories that are expanded, so files that appeared or
    /// disappeared show up. Returns an error when a directory cannot be read.
    ///
    /// Directories that are collapsed are left alone: their contents are read
    /// again when they are expanded.
    pub fn refresh(&mut self) -> io::Result<()> {
        let ignored = &self.ignored;
        let mut first_error = None;
        refresh_node(&mut self.root, ignored, &mut first_error);
        self.rebuild();

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Whether the path is in the tree (used for hit testing and for deciding
    /// which row is highlighted).
    ///
    /// The tree knows the root, the rows it shows, and the nodes it read
    /// earlier and now hides because their directory is collapsed. A path
    /// inside a directory that was never read is not in the tree, because the
    /// tree never saw it.
    pub fn contains(&self, path: &Path) -> bool {
        find_node(&self.root, path).is_some()
    }

    /// Rebuilds the rows of the tree from the nodes.
    fn rebuild(&mut self) {
        self.visible.clear();
        collect_visible(&self.root, &mut self.visible);
    }
}

/// Reads the children of a directory, sorted and filtered.
fn read_children(directory: &Path, depth: usize, ignored: &[String]) -> io::Result<Vec<TreeNode>> {
    let mut children = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let is_directory = path.is_dir();
        if is_ignored(&name, is_directory, ignored) {
            continue;
        }
        children.push(TreeNode {
            name,
            path,
            kind: if is_directory {
                NodeKind::Directory
            } else {
                NodeKind::File
            },
            children: Vec::new(),
            expanded: false,
            depth,
        });
    }
    sort_children(&mut children);
    Ok(children)
}

/// Whether an entry is kept out of the tree.
fn is_ignored(name: &str, is_directory: bool, ignored: &[String]) -> bool {
    for entry in ignored {
        if entry == HIDDEN_MARKER {
            if name.starts_with('.') {
                return true;
            }
        } else if is_directory && name.to_lowercase() == entry.to_lowercase() {
            return true;
        }
    }
    false
}

/// Sorts directories before files, then by name.
fn sort_children(children: &mut [TreeNode]) {
    children.sort_by(|left, right| match (left.kind, right.kind) {
        (NodeKind::Directory, NodeKind::File) => Ordering::Less,
        (NodeKind::File, NodeKind::Directory) => Ordering::Greater,
        _ => compare_names(&left.name, &right.name),
    });
}

/// Compares names without regard to case, then by the name itself, so that
/// two entries that differ only in case keep a stable order.
fn compare_names(left: &str, right: &str) -> Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

/// Collects the rows of the tree: a node, then the children of a directory
/// that is expanded.
fn collect_visible(node: &TreeNode, out: &mut Vec<TreeNode>) {
    for child in &node.children {
        out.push(child.clone());
        if child.kind == NodeKind::Directory && child.expanded {
            collect_visible(child, out);
        }
    }
}

/// Finds a node anywhere in the tree.
fn find_node<'a>(node: &'a TreeNode, path: &Path) -> Option<&'a TreeNode> {
    if same_path(&node.path, path) {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_node(child, path) {
            return Some(found);
        }
    }
    None
}

/// Finds a node that the tree currently shows.
///
/// `visible` says whether the node's ancestors are all expanded; a node
/// hidden behind a collapsed directory is not a row and cannot be toggled.
fn find_visible<'a>(
    node: &'a mut TreeNode,
    path: &Path,
    visible: bool,
) -> Option<&'a mut TreeNode> {
    if visible && same_path(&node.path, path) {
        return Some(node);
    }
    let children_visible = visible && node.expanded;
    for child in &mut node.children {
        if let Some(found) = find_visible(child, path, children_visible) {
            return Some(found);
        }
    }
    None
}

/// Re-reads a node and the expanded directories below it.
fn refresh_node(node: &mut TreeNode, ignored: &[String], first_error: &mut Option<io::Error>) {
    match read_children(&node.path, node.depth + 1, ignored) {
        Ok(mut children) => {
            // A child that is still there keeps the state it had, so that the
            // directories the user opened stay open.
            for child in &mut children {
                if child.kind != NodeKind::Directory {
                    continue;
                }
                if let Some(previous) = node
                    .children
                    .iter()
                    .find(|previous| previous.name == child.name)
                    && previous.kind == NodeKind::Directory
                {
                    child.expanded = previous.expanded;
                }
            }
            node.children = children;
        }
        Err(error) => {
            if first_error.is_none() {
                *first_error = Some(error);
            }
            return;
        }
    }

    for child in &mut node.children {
        if child.kind == NodeKind::Directory && child.expanded {
            refresh_node(child, ignored, first_error);
        }
    }
}

/// Whether two paths point at the same place.
///
/// Windows file names are not case sensitive, so a path that arrives with a
/// different case still names the same file.
fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory inside the build output, empty and unique per test.
    fn scratch(name: &str) -> PathBuf {
        let directory = scratch_root().join(name);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory
    }

    /// Where the scratch files live: the build output, which is ignored by git.
    fn scratch_root() -> PathBuf {
        match std::env::var("CARGO_TARGET_TMPDIR") {
            Ok(directory) => PathBuf::from(directory),
            Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp"),
        }
    }

    /// Writes a file, creating the directories above it.
    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("the scratch directory can be created");
        }
        fs::write(path, text).expect("the scratch file can be written");
    }

    /// The names of the rows of the tree.
    fn names(tree: &FileTree) -> Vec<&str> {
        tree.nodes().iter().map(|node| node.name.as_str()).collect()
    }

    #[test]
    fn starts_with_the_root_expanded() {
        let root = scratch("tree-starts-expanded");
        write_file(&root.join("main.rs"), "fn main() {}\n");
        fs::create_dir_all(root.join("src")).unwrap();

        let tree = FileTree::new(&root);

        assert_eq!(tree.root(), root.as_path());
        assert_eq!(names(&tree), vec!["src", "main.rs"]);
        assert!(
            !tree.nodes().iter().any(|node| same_path(&node.path, &root)),
            "the root itself is not a row"
        );
        assert_eq!(tree.nodes()[0].depth, 1);
        assert_eq!(tree.nodes()[0].kind, NodeKind::Directory);
        assert_eq!(tree.nodes()[1].kind, NodeKind::File);
        assert!(tree.is_expanded(&root));
    }

    #[test]
    fn sorts_directories_before_files_and_names_case_insensitively() {
        let root = scratch("tree-sorts");
        write_file(&root.join("zebra.md"), "z\n");
        write_file(&root.join("Banana.md"), "b\n");
        fs::create_dir_all(root.join("Zoo")).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();

        let tree = FileTree::new(&root);

        // `Zoo` sorts after `assets`, even though `Z` comes before `a` when
        // the case is counted.
        assert_eq!(names(&tree), vec!["assets", "Zoo", "Banana.md", "zebra.md"]);
    }

    #[test]
    fn sorts_names_that_differ_only_in_case_by_the_name_itself() {
        // Two files that differ only in case cannot live side by side on
        // Windows, so the tie-break is checked on the comparison itself.
        assert_eq!(compare_names("Apple.md", "apple.md"), Ordering::Less);
        assert_eq!(compare_names("apple.md", "Apple.md"), Ordering::Greater);
        assert_eq!(compare_names("apple.md", "apple.md"), Ordering::Equal);
    }

    #[test]
    fn ignores_the_default_directories_at_any_depth() {
        let root = scratch("tree-ignores-directories");
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join("src").join("node_modules")).unwrap();
        fs::create_dir_all(root.join("src").join("sub").join("target")).unwrap();
        write_file(&root.join("src").join("main.rs"), "fn main() {}\n");

        let mut tree = FileTree::new(&root);

        assert_eq!(names(&tree), vec!["src"]);
        assert!(tree.toggle(&root.join("src")));
        assert_eq!(names(&tree), vec!["src", "sub", "main.rs"]);

        // The `target` directory below `sub` is ignored as well.
        assert!(tree.toggle(&root.join("src").join("sub")));
        assert_eq!(names(&tree), vec!["src", "sub", "main.rs"]);
    }

    #[test]
    fn hides_hidden_entries_by_default() {
        let root = scratch("tree-hides-hidden");
        write_file(&root.join(".hidden"), "x\n");
        fs::create_dir_all(root.join(".hidden-dir")).unwrap();
        write_file(&root.join("visible.txt"), "x\n");

        let tree = FileTree::new(&root);

        assert_eq!(names(&tree), vec!["visible.txt"]);
    }

    #[test]
    fn with_ignored_replaces_the_default_rules() {
        let root = scratch("tree-with-ignored");
        fs::create_dir_all(root.join("build")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        write_file(&root.join(".hidden"), "x\n");

        // The caller's list replaces the default one: `build` is skipped and
        // `target` is no longer skipped.
        let tree = FileTree::with_ignored(&root, vec!["build".to_owned()]);
        assert_eq!(names(&tree), vec!["target", ".hidden"]);

        // A list without the hidden marker shows the hidden entries.
        let tree = FileTree::with_ignored(&root, vec![HIDDEN_MARKER.to_owned()]);
        assert_eq!(names(&tree), vec!["build", "target"]);
    }

    #[test]
    fn toggling_a_file_does_nothing() {
        let root = scratch("tree-toggles-a-file");
        let file = root.join("main.rs");
        write_file(&file, "fn main() {}\n");

        let mut tree = FileTree::new(&root);

        assert!(!tree.toggle(&file));
        assert_eq!(names(&tree), vec!["main.rs"]);
        assert!(!tree.is_expanded(&file));
    }

    #[test]
    fn toggling_a_directory_reads_it_from_disk() {
        let root = scratch("tree-toggles-a-directory");
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();
        write_file(&source.join("main.rs"), "fn main() {}\n");

        let mut tree = FileTree::new(&root);
        assert_eq!(names(&tree), vec!["src"]);

        assert!(tree.toggle(&source));
        assert_eq!(names(&tree), vec!["src", "main.rs"]);
        assert_eq!(tree.nodes()[1].depth, 2);
        assert!(tree.is_expanded(&source));

        assert!(tree.toggle(&source));
        assert_eq!(names(&tree), vec!["src"]);
        assert!(!tree.is_expanded(&source));
    }

    #[test]
    fn toggling_a_directory_that_cannot_be_read_fails_without_panicking() {
        let root = scratch("tree-toggles-a-missing-directory");
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();

        let mut tree = FileTree::new(&root);

        // The directory is gone before it is ever opened.
        fs::remove_dir_all(&source).unwrap();

        assert!(!tree.toggle(&source));
        assert_eq!(names(&tree), vec!["src"]);
        assert!(!tree.is_expanded(&source));
    }

    #[test]
    fn refresh_only_reads_the_expanded_directories() {
        let root = scratch("tree-refresh");
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();
        write_file(&source.join("old.rs"), "old\n");

        let mut tree = FileTree::new(&root);
        assert!(tree.toggle(&source));
        assert_eq!(names(&tree), vec!["src", "old.rs"]);

        // A file that appears in an open directory shows up after a refresh.
        write_file(&source.join("new.rs"), "new\n");
        write_file(&root.join("top.rs"), "top\n");
        tree.refresh().unwrap();
        assert_eq!(names(&tree), vec!["src", "new.rs", "old.rs", "top.rs"]);

        // A file that appears in a closed directory stays out of the tree
        // until the directory is opened again.
        assert!(tree.toggle(&source));
        write_file(&source.join("later.rs"), "later\n");
        tree.refresh().unwrap();
        assert_eq!(names(&tree), vec!["src", "top.rs"]);

        assert!(tree.toggle(&source));
        assert_eq!(
            names(&tree),
            vec!["src", "later.rs", "new.rs", "old.rs", "top.rs"]
        );
    }

    #[test]
    fn refresh_reports_a_directory_that_cannot_be_read() {
        let root = scratch("tree-refresh-missing");
        write_file(&root.join("main.rs"), "fn main() {}\n");

        let mut tree = FileTree::new(&root);

        // The root itself is gone, so it cannot be read any more.
        fs::remove_dir_all(&root).unwrap();

        assert!(tree.refresh().is_err());
    }

    #[test]
    fn refresh_drops_the_entries_that_disappeared() {
        let root = scratch("tree-refresh-drops");
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();
        write_file(&source.join("old.rs"), "old\n");

        let mut tree = FileTree::new(&root);
        assert!(tree.toggle(&source));
        assert!(tree.contains(&source));

        fs::remove_dir_all(&source).unwrap();
        tree.refresh().unwrap();

        assert!(!tree.contains(&source));
        assert_eq!(names(&tree), Vec::<&str>::new());
    }

    #[test]
    fn contains_finds_the_paths_of_the_tree() {
        let root = scratch("tree-contains");
        let source = root.join("src");
        fs::create_dir_all(&source).unwrap();
        let file = source.join("main.rs");
        write_file(&file, "fn main() {}\n");
        let outside = scratch_root().join("not-in-the-tree.txt");

        let mut tree = FileTree::new(&root);

        assert!(tree.contains(&root));
        assert!(tree.contains(&source));
        // The file is inside a directory that was never opened, so the tree
        // does not know it yet.
        assert!(!tree.contains(&file));

        assert!(tree.toggle(&source));
        assert!(tree.contains(&file));
        assert!(!tree.contains(&outside));
    }
}
