//! The model of the editor's tab strip.
//!
//! A tab is what the user sees in the strip above the editor: a title, the
//! file behind it, and whether it has changes that are not on disk yet. This
//! module decides exactly three things:
//!
//! * which tabs are open, and in which order they sit in the strip,
//! * which one of them is focused (the one "in front"),
//! * whether closing a tab needs the user to confirm first.
//!
//! What this module deliberately does **not** own:
//!
//! * **The text.** A tab has no buffer. The buffers belong to the editor core;
//!   the dirty flag here is a *summary* of what the editor core knows, not a
//!   copy of the text.
//! * **The window.** Nothing here draws anything or pops up a dialog. The
//!   platform layer asks [`close_decision`] what to do and shows the
//!   confirmation itself.
//!
//! Keeping the rules here means they can be tested without a window: opening
//! the same file twice, closing the focused tab, cycling through the strip.
//!
//! # The shape of the strip
//!
//! The strip has an order — left to right — and one focused tab. Activating a
//! tab focuses it but does **not** move it. That mirrors what editors do: the
//! tab you come back to stays where it was, so the strip does not shuffle
//! under the cursor while the user works. The focused tab is the one the
//! editor shows and the one a command applies to.
//!
//! # Product rules
//!
//! * *One file, one tab.* Opening a file that is already open focuses the
//!   existing tab instead of creating a second one — see [`TabBar::open`].
//! * *Closing a dirty tab asks first.* A tab with unsaved changes must not
//!   disappear behind the user's back — see [`close_decision`].

use std::path::{Path, PathBuf};

/// The identity of a tab. Stable for the life of the tab.
///
/// The platform layer needs something it can hold on to: the window remembers
/// which tab was focused, a command remembers which tab it was aimed at. The
/// id never changes and is never reused, so a stale id is *unknown* rather
/// than *wrong* — which is what makes `close` and `activate` able to report
/// "no such tab" instead of touching the wrong one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(u64);

impl TabId {
    /// The number behind the id.
    ///
    /// Exposed as a plain number so the platform layer can log it or show it
    /// in a status line without knowing how ids are made.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// What kind of thing a tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    /// A file on disk.
    File,
    /// A file that has never been saved.
    ///
    /// A scratch tab has no path behind it, so it is never merged with
    /// another tab: two unnamed scratch buffers are two different things.
    Scratch,
}

/// One entry in the tab strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    /// The identity of this tab.
    pub id: TabId,
    /// The name shown on the tab.
    ///
    /// This is the name the caller asked for. Deriving a title from the file
    /// name is a window's job, because only the window knows whether the
    /// strip shows `main.rs` or the full path.
    pub title: String,
    /// The file behind the tab; `None` for a scratch tab.
    pub path: Option<PathBuf>,
    /// Whether the tab shows a file or a scratch buffer.
    pub kind: TabKind,
    /// Whether the tab has changes that are not on disk yet.
    ///
    /// The editor core owns the text; this flag is only the summary of it
    /// that the strip needs to draw the "modified" marker and to decide
    /// whether closing the tab has to ask first.
    pub dirty: bool,
}

/// The tabs of one window, in strip order, with one of them focused.
#[derive(Clone, Debug)]
pub struct TabBar {
    /// The open tabs, left to right.
    tabs: Vec<Tab>,
    /// The focused tab.
    ///
    /// Stored as an id rather than an index so that closing some other tab
    /// cannot silently shift the focus onto a neighbour. It is `None` exactly
    /// when the strip is empty.
    active: Option<TabId>,
    /// The id the next tab will get.
    ///
    /// Ids start at 1: 0 is kept free to mean "no tab", so a default or
    /// zeroed value is never mistaken for a real tab.
    next_id: u64,
}

impl TabBar {
    /// A strip with no tabs.
    pub fn new() -> Self {
        TabBar {
            tabs: Vec::new(),
            active: None,
            next_id: 1,
        }
    }

    /// Opens a file as a tab and focuses it.
    ///
    /// A file that is already open is **not** opened twice: the existing tab
    /// is focused and its id is returned. This is a product rule, not an
    /// optimisation — two tabs on the same file would let the user edit one
    /// copy and save over the other, so the strip holds one tab per file.
    ///
    /// The path is stored as it was given, and so is the title. When a file is
    /// opened a second time under a different spelling, the tab keeps the
    /// first spelling: it is the one the user opened first.
    pub fn open(&mut self, path: impl Into<PathBuf>, title: impl Into<String>) -> TabId {
        let path = path.into();
        if let Some(id) = self.find_file(&path) {
            // Already open: focus it instead of opening a second tab.
            self.activate(id);
            return id;
        }

        let id = self.allocate_id();
        self.tabs.push(Tab {
            id,
            title: title.into(),
            path: Some(path),
            kind: TabKind::File,
            dirty: false,
        });
        self.active = Some(id);
        id
    }

    /// Opens a tab that has no file behind it yet and focuses it.
    ///
    /// Every call makes a new tab, even for the same title: two scratch
    /// buffers that happen to be called the same thing are still two
    /// different buffers, and merging them would lose one of them.
    pub fn open_scratch(&mut self, title: impl Into<String>) -> TabId {
        let id = self.allocate_id();
        self.tabs.push(Tab {
            id,
            title: title.into(),
            path: None,
            kind: TabKind::Scratch,
            dirty: false,
        });
        self.active = Some(id);
        id
    }

    /// Closes a tab. Returns false when there is no such tab.
    ///
    /// Closing the focused tab focuses its right-hand neighbour, or the
    /// left-hand one when the closed tab was the last in the strip — the
    /// user should always land on a tab that is still open. Closing a tab
    /// that is not focused leaves the focus where it is.
    pub fn close(&mut self, id: TabId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let was_focused = self.active == Some(id);
        self.tabs.remove(index);

        if was_focused {
            self.active = if index < self.tabs.len() {
                // The tab that took the closed one's place.
                Some(self.tabs[index].id)
            } else if index > 0 {
                // The closed tab was the last one: fall back to the one before it.
                Some(self.tabs[index - 1].id)
            } else {
                // The strip is empty now.
                None
            };
        }
        true
    }

    /// Brings a tab to the front. Returns false when there is no such tab.
    ///
    /// The tab keeps its place in the strip; only the focus moves. See the
    /// module documentation for why the strip does not reorder.
    pub fn activate(&mut self, id: TabId) -> bool {
        if self.index_of(id).is_none() {
            return false;
        }
        self.active = Some(id);
        true
    }

    /// Brings the tab at `index` to the front. Returns false when the index is
    /// out of range.
    pub fn activate_index(&mut self, index: usize) -> bool {
        let Some(tab) = self.tabs.get(index) else {
            return false;
        };
        self.active = Some(tab.id);
        true
    }

    /// The tab in front, if there is one.
    pub fn active(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tab(id))
    }

    /// The tab in front, for changes the editor core owns — the title, for
    /// instance.
    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        let active = self.active?;
        self.tabs.iter_mut().find(|tab| tab.id == active)
    }

    /// The index of the tab in front.
    pub fn active_index(&self) -> Option<usize> {
        self.active.and_then(|id| self.index_of(id))
    }

    /// All open tabs, in strip order.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The tab with this id, if it is open.
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    /// The index of a tab, if it is open.
    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// Marks a tab as having unsaved changes (or not). Returns false when
    /// there is no such tab.
    ///
    /// Only the dirty flag is touched. The title, the path and the kind
    /// belong to whoever opened the tab; the editor core owns the text and
    /// only reports what it knows about it.
    pub fn mark_dirty(&mut self, id: TabId, dirty: bool) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return false;
        };
        tab.dirty = dirty;
        true
    }

    /// Brings the next tab to the front, wrapping around.
    ///
    /// A strip with one tab stays where it is, and an empty strip does
    /// nothing at all.
    pub fn next(&mut self) {
        let Some(index) = self.active_index() else {
            return;
        };
        let len = self.tabs.len();
        if len < 2 {
            return;
        }
        self.active = Some(self.tabs[(index + 1) % len].id);
    }

    /// Brings the previous tab to the front, wrapping around.
    ///
    /// A strip with one tab stays where it is, and an empty strip does
    /// nothing at all.
    pub fn previous(&mut self) {
        let Some(index) = self.active_index() else {
            return;
        };
        let len = self.tabs.len();
        if len < 2 {
            return;
        }
        self.active = Some(self.tabs[(index + len - 1) % len].id);
    }

    /// How many tabs are open.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether the strip is empty.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Hands out the next id.
    fn allocate_id(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Finds the tab that shows `path`, if the strip has one.
    fn find_file(&self, path: &Path) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|tab| {
                tab.kind == TabKind::File
                    && tab
                        .path
                        .as_deref()
                        .is_some_and(|open| same_file(open, path))
            })
            .map(|tab| tab.id)
    }
}

impl Default for TabBar {
    /// A strip with no tabs, same as [`TabBar::new`].
    fn default() -> Self {
        TabBar::new()
    }
}

/// What should happen to a tab the user wants to close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseDecision {
    /// The tab has no unsaved changes: close it.
    Close,
    /// The tab has unsaved changes: the window has to ask the user.
    Ask,
}

/// The decision for a tab, given what is known about it.
///
/// The rule is a product rule, not a technical one: a tab with unsaved
/// changes must never disappear without asking, because the text behind it
/// exists nowhere else. The window layer turns [`CloseDecision::Ask`] into a
/// dialog — this function only decides *whether* to ask.
pub fn close_decision(tab: &Tab) -> CloseDecision {
    if tab.dirty {
        CloseDecision::Ask
    } else {
        CloseDecision::Close
    }
}

/// Whether two paths point at the same file.
///
/// The rule follows the file system of the platform, because a wrong answer
/// here either opens a file twice or merges two different files into one
/// tab:
///
/// * On **Windows** the comparison is case-insensitive. `C:\a\file.rs` and
///   `c:\A\FILE.RS` are the same file, so they must be the same tab.
/// * Everywhere else the comparison is exact. On a case-sensitive file
///   system `main.rs` and `MAIN.RS` are two different files, and treating
///   them as one would let the user save one over the other.
///
/// The comparison is *lexical*: the paths are split into components and
/// compared one by one, so redundant separators and `.` components do not
/// matter. `..` is not resolved — that needs the file system, and the caller
/// is expected to hand in paths that already point at a file.
fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    #[cfg(windows)]
    {
        windows_same_file(left, right)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Compares two Windows paths component by component, ignoring case.
#[cfg(windows)]
fn windows_same_file(left: &Path, right: &Path) -> bool {
    use std::path::Component;

    // `.` never says anything about which file is meant, so it is dropped on
    // both sides; `Components` already drops it everywhere but at the front
    // of a relative path.
    let mut left = left.components().filter(|c| !matches!(c, Component::CurDir));
    let mut right = right
        .components()
        .filter(|c| !matches!(c, Component::CurDir));

    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) => {
                if !windows_same_component(a, b) {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

/// Compares two Windows path components, ignoring case.
#[cfg(windows)]
fn windows_same_component(
    left: std::path::Component<'_>,
    right: std::path::Component<'_>,
) -> bool {
    use std::path::Component;

    match (left, right) {
        (Component::Prefix(a), Component::Prefix(b)) => {
            // `C:` and `c:` are the same drive.
            eq_ignore_case(a.as_os_str(), b.as_os_str())
        }
        (Component::RootDir, Component::RootDir) => true,
        (Component::ParentDir, Component::ParentDir) => true,
        (Component::Normal(a), Component::Normal(b)) => eq_ignore_case(a, b),
        _ => false,
    }
}

/// Compares two path fragments, ignoring case.
#[cfg(windows)]
fn eq_ignore_case(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    match (left.to_str(), right.to_str()) {
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        // A path that is not valid UTF-8 is rare enough that comparing the raw
        // bytes with ASCII case folding is good enough.
        _ => left
            .as_encoded_bytes()
            .eq_ignore_ascii_case(right.as_encoded_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_strip_is_empty() {
        let bar = TabBar::new();
        assert!(bar.is_empty());
        assert_eq!(bar.len(), 0);
        assert!(bar.active().is_none());
        assert!(bar.active_index().is_none());
    }

    #[test]
    fn opening_a_file_puts_it_in_the_strip_and_focuses_it() {
        let mut bar = TabBar::new();
        let id = bar.open("src/main.rs", "main.rs");

        assert_eq!(bar.len(), 1);
        assert_eq!(bar.active().map(|tab| tab.id), Some(id));
        assert_eq!(bar.active_index(), Some(0));
        assert_eq!(bar.index_of(id), Some(0));

        let tab = bar.tab(id).expect("the tab is open");
        assert_eq!(tab.kind, TabKind::File);
        assert_eq!(tab.path.as_deref(), Some(Path::new("src/main.rs")));
        assert_eq!(tab.title, "main.rs");
        assert!(!tab.dirty);
    }

    #[test]
    fn opening_the_same_file_twice_returns_the_same_tab() {
        let mut bar = TabBar::new();
        let first = bar.open("src/main.rs", "main.rs");
        let second = bar.open("src/main.rs", "main.rs");

        assert_eq!(first, second, "one file, one tab");
        assert_eq!(bar.len(), 1, "the file must not be opened twice");
    }

    #[test]
    fn reopening_a_file_focuses_the_existing_tab() {
        let mut bar = TabBar::new();
        let first = bar.open("a.rs", "a.rs");
        let second = bar.open("b.rs", "b.rs");
        assert_eq!(bar.active().map(|tab| tab.id), Some(second));

        let again = bar.open("a.rs", "a.rs");

        assert_eq!(again, first);
        assert_eq!(
            bar.active().map(|tab| tab.id),
            Some(first),
            "the existing tab comes to the front"
        );
        assert_eq!(bar.len(), 2);
    }

    #[test]
    fn paths_are_compared_by_components_not_by_spelling() {
        let mut bar = TabBar::new();
        let first = bar.open("dir/file.rs", "file.rs");
        let second = bar.open("./dir/file.rs", "file.rs");
        let third = bar.open("dir/file.rs/", "file.rs");

        assert_eq!(first, second);
        assert_eq!(first, third);
        assert_eq!(bar.len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn paths_that_differ_only_in_case_are_the_same_file_on_windows() {
        let mut bar = TabBar::new();
        let first = bar.open(r"C:\a\file.rs", "file.rs");
        let second = bar.open(r"c:\A\FILE.RS", "file.rs");

        assert_eq!(first, second, "the file system is case-insensitive");
        assert_eq!(bar.len(), 1);
    }

    #[cfg(not(windows))]
    #[test]
    fn paths_that_differ_only_in_case_are_different_files_elsewhere() {
        let mut bar = TabBar::new();
        let first = bar.open("a/file.rs", "file.rs");
        let second = bar.open("A/file.rs", "file.rs");

        assert_ne!(first, second, "the file system is case-sensitive");
        assert_eq!(bar.len(), 2);
    }

    #[test]
    fn closing_the_focused_tab_focuses_the_next_one() {
        let mut bar = TabBar::new();
        let first = bar.open("a.rs", "a.rs");
        let second = bar.open("b.rs", "b.rs");
        let third = bar.open("c.rs", "c.rs");
        assert_eq!(bar.active().map(|tab| tab.id), Some(third));

        assert!(bar.close(third));

        assert_eq!(bar.len(), 2);
        assert_eq!(
            bar.active().map(|tab| tab.id),
            Some(second),
            "the tab behind the closed one becomes the current tab"
        );
        assert_eq!(bar.active_index(), Some(1));
        assert_eq!(bar.index_of(first), Some(0));
    }

    #[test]
    fn closing_the_last_tab_focuses_the_one_before_it() {
        let mut bar = TabBar::new();
        let first = bar.open("a.rs", "a.rs");
        let second = bar.open("b.rs", "b.rs");

        assert!(bar.close(second));

        assert_eq!(bar.active().map(|tab| tab.id), Some(first));
        assert_eq!(bar.active_index(), Some(0));
    }

    #[test]
    fn closing_the_last_tab_empties_the_strip() {
        let mut bar = TabBar::new();
        let only = bar.open("a.rs", "a.rs");

        assert!(bar.close(only));

        assert!(bar.is_empty());
        assert_eq!(bar.len(), 0);
        assert!(bar.active().is_none());
        assert!(bar.active_index().is_none());
    }

    #[test]
    fn closing_a_tab_that_is_not_focused_keeps_the_focus() {
        let mut bar = TabBar::new();
        let first = bar.open("a.rs", "a.rs");
        let second = bar.open("b.rs", "b.rs");

        assert!(bar.close(first));

        assert_eq!(bar.len(), 1);
        assert_eq!(bar.active().map(|tab| tab.id), Some(second));
        assert_eq!(bar.active_index(), Some(0));
    }

    #[test]
    fn closing_an_unknown_tab_changes_nothing() {
        let mut bar = TabBar::new();
        let id = bar.open("a.rs", "a.rs");
        let stranger = TabId(999);

        assert!(!bar.close(stranger));

        assert_eq!(bar.len(), 1);
        assert_eq!(bar.active().map(|tab| tab.id), Some(id));
        assert_eq!(bar.index_of(stranger), None);
    }

    #[test]
    fn activating_an_unknown_tab_changes_nothing() {
        let mut bar = TabBar::new();
        let id = bar.open("a.rs", "a.rs");

        assert!(!bar.activate(TabId(999)));

        assert_eq!(bar.active().map(|tab| tab.id), Some(id));
    }

    #[test]
    fn activating_moves_the_focus_without_moving_the_tab() {
        let mut bar = TabBar::new();
        let first = bar.open("a.rs", "a.rs");
        let second = bar.open("b.rs", "b.rs");
        let third = bar.open("c.rs", "c.rs");

        assert!(bar.activate(first));

        assert_eq!(bar.active().map(|tab| tab.id), Some(first));
        assert_eq!(
            bar.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![first, second, third],
            "the strip keeps its order"
        );
        assert_eq!(bar.active_index(), Some(0));
    }

    #[test]
    fn activating_an_index_out_of_range_changes_nothing() {
        let mut bar = TabBar::new();
        let id = bar.open("a.rs", "a.rs");

        assert!(!bar.activate_index(7));

        assert_eq!(bar.active().map(|tab| tab.id), Some(id));
        assert_eq!(bar.active_index(), Some(0));
    }

    #[test]
    fn activating_an_index_focuses_that_tab() {
        let mut bar = TabBar::new();
        bar.open("a.rs", "a.rs");
        let second = bar.open("b.rs", "b.rs");

        assert!(bar.activate_index(0));

        assert_eq!(bar.active_index(), Some(0));
        assert_ne!(bar.active().map(|tab| tab.id), Some(second));
    }

    #[test]
    fn mark_dirty_only_changes_the_dirty_flag() {
        let mut bar = TabBar::new();
        let id = bar.open("src/main.rs", "main.rs");
        let before = bar.tab(id).expect("the tab is open").clone();

        assert!(bar.mark_dirty(id, true));

        let after = bar.tab(id).expect("the tab is open");
        assert!(after.dirty);
        assert_eq!(after.title, before.title);
        assert_eq!(after.path, before.path);
        assert_eq!(after.kind, before.kind);
        assert_eq!(after.id, before.id);

        assert!(bar.mark_dirty(id, false));
        assert!(!bar.tab(id).expect("the tab is open").dirty);
    }

    #[test]
    fn mark_dirty_on_an_unknown_tab_reports_failure() {
        let mut bar = TabBar::new();
        bar.open("a.rs", "a.rs");

        assert!(!bar.mark_dirty(TabId(999), true));
    }

    #[test]
    fn cycling_moves_the_focus_and_wraps_around() {
        let mut bar = TabBar::new();
        let first = bar.open("a.rs", "a.rs");
        let second = bar.open("b.rs", "b.rs");
        let third = bar.open("c.rs", "c.rs");
        assert_eq!(bar.active().map(|tab| tab.id), Some(third));

        bar.next();
        assert_eq!(
            bar.active().map(|tab| tab.id),
            Some(first),
            "the cycle wraps around"
        );
        bar.next();
        assert_eq!(bar.active().map(|tab| tab.id), Some(second));
        bar.next();
        assert_eq!(bar.active().map(|tab| tab.id), Some(third));

        bar.previous();
        assert_eq!(bar.active().map(|tab| tab.id), Some(second));
        bar.previous();
        assert_eq!(bar.active().map(|tab| tab.id), Some(first));
        bar.previous();
        assert_eq!(
            bar.active().map(|tab| tab.id),
            Some(third),
            "the cycle wraps around backwards"
        );
    }

    #[test]
    fn cycling_with_one_tab_stays_put() {
        let mut bar = TabBar::new();
        let id = bar.open("a.rs", "a.rs");

        bar.next();
        assert_eq!(bar.active().map(|tab| tab.id), Some(id));
        bar.previous();
        assert_eq!(bar.active().map(|tab| tab.id), Some(id));
    }

    #[test]
    fn cycling_on_an_empty_strip_does_nothing() {
        let mut bar = TabBar::new();

        bar.next();
        bar.previous();

        assert!(bar.is_empty());
        assert!(bar.active().is_none());
    }

    #[test]
    fn scratch_tabs_are_never_merged() {
        let mut bar = TabBar::new();
        let first = bar.open_scratch("untitled");
        let second = bar.open_scratch("untitled");

        assert_ne!(first, second, "two scratch buffers are two tabs");
        assert_eq!(bar.len(), 2);

        let tab = bar.tab(second).expect("the tab is open");
        assert_eq!(tab.kind, TabKind::Scratch);
        assert_eq!(tab.path, None);
    }

    #[test]
    fn a_scratch_tab_does_not_shadow_a_file() {
        let mut bar = TabBar::new();
        let scratch = bar.open_scratch("main.rs");
        let file = bar.open("main.rs", "main.rs");

        assert_ne!(scratch, file);
        assert_eq!(bar.len(), 2);
    }

    #[test]
    fn tab_ids_start_at_one_and_increase() {
        let mut bar = TabBar::new();
        let first = bar.open("a.rs", "a.rs");
        let second = bar.open("b.rs", "b.rs");
        let third = bar.open_scratch("c");

        assert_eq!(first.get(), 1, "0 is reserved for 'no tab'");
        assert_eq!(second.get(), 2);
        assert_eq!(third.get(), 3);
        assert!(first < second && second < third);
    }

    #[test]
    fn ids_are_not_reused_after_a_tab_is_closed() {
        let mut bar = TabBar::new();
        let first = bar.open("a.rs", "a.rs");
        bar.close(first);
        let second = bar.open("b.rs", "b.rs");

        assert_ne!(first, second);
        assert_eq!(second.get(), 2);
    }

    #[test]
    fn the_title_and_the_path_come_from_the_caller() {
        let mut bar = TabBar::new();
        let id = bar.open("src/lib.rs", "the name I want");

        let tab = bar.tab(id).expect("the tab is open");
        assert_eq!(tab.title, "the name I want");
        assert_eq!(tab.path, Some(PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn the_focused_tab_can_be_changed() {
        let mut bar = TabBar::new();
        let id = bar.open("a.rs", "a.rs");

        let tab = bar.active_mut().expect("there is a focused tab");
        tab.title = "renamed".to_owned();
        tab.dirty = true;

        assert_eq!(bar.tab(id).expect("the tab is open").title, "renamed");
        assert!(bar.tab(id).expect("the tab is open").dirty);
    }

    #[test]
    fn close_decision_closes_a_clean_tab() {
        let mut bar = TabBar::new();
        let id = bar.open("a.rs", "a.rs");

        let tab = bar.tab(id).expect("the tab is open");
        assert_eq!(close_decision(tab), CloseDecision::Close);
    }

    #[test]
    fn close_decision_asks_before_closing_a_dirty_tab() {
        let mut bar = TabBar::new();
        let id = bar.open("a.rs", "a.rs");
        bar.mark_dirty(id, true);

        let tab = bar.tab(id).expect("the tab is open");
        assert_eq!(close_decision(tab), CloseDecision::Ask);
    }

    #[test]
    fn close_decision_asks_before_closing_a_dirty_scratch_tab() {
        let mut bar = TabBar::new();
        let id = bar.open_scratch("untitled");
        bar.mark_dirty(id, true);

        let tab = bar.tab(id).expect("the tab is open");
        assert_eq!(
            close_decision(tab),
            CloseDecision::Ask,
            "an unsaved scratch buffer is unsaved too"
        );
    }
}
