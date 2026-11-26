//! The columns of the window, and the tabs in them.
//!
//! A window with one column shows one group of tabs. When the person splits
//! the window, there are two: each group has its own tabs and its own tab in
//! front, and the tabs of one group are not the tabs of the other.
//!
//! The groups do **not** own the tabs themselves — the tab bar of the window
//! owns those. A group says *which* tabs it shows, by their place in the tab
//! bar of the window, so that the text, the scroll position and the history of
//! a file stay where they are, no matter which column shows it.
//!
//! The places of the columns in the window are not here: that is the job of
//! `layout.rs`, and the two are kept apart so that both can be tested without
//! a window.

/// One column of the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    /// The tabs of this column, by their place in the tab bar of the window.
    tabs: Vec<usize>,
    /// Which of them is in front.
    ///
    /// It is a place in `tabs`, not a place in the tab bar: the tabs of the
    /// window move when one of them is closed, but the order inside a column
    /// does not.
    front: usize,
}

impl Group {
    /// The tabs of this column.
    pub fn tabs(&self) -> &[usize] {
        &self.tabs
    }

    /// The tab in front of this column, by its place in the tab bar.
    pub fn front(&self) -> Option<usize> {
        self.tabs.get(self.front).copied()
    }

    /// Whether this column shows the tab with that place.
    pub fn holds(&self, tab: usize) -> bool {
        self.tabs.contains(&tab)
    }

    /// Brings the tab to the front of this column.
    ///
    /// Returns `false` when the column does not show that tab.
    pub fn front_tab(&mut self, tab: usize) -> bool {
        let Some(place) = self.tabs.iter().position(|held| *held == tab) else {
            return false;
        };
        self.front = place;
        true
    }

    /// Puts the tab at the front of this column.
    ///
    /// A tab that is already here is only brought to the front: the column
    /// does not show the same tab twice.
    pub fn push(&mut self, tab: usize) {
        if let Some(place) = self.tabs.iter().position(|held| *held == tab) {
            self.front = place;
            return;
        }
        self.tabs.push(tab);
        self.front = self.tabs.len() - 1;
    }

    /// Takes the tab out of this column.
    ///
    /// Returns `false` when the column does not show that tab.
    pub fn take(&mut self, tab: usize) -> bool {
        let Some(place) = self.tabs.iter().position(|held| *held == tab) else {
            return false;
        };
        self.tabs.remove(place);
        // The tab that was in front is gone: the one that took its place is in
        // front now.
        if self.front == place {
            self.front = self.front.min(self.tabs.len().saturating_sub(1));
        } else if self.front > place {
            self.front -= 1;
        }
        true
    }

    /// Whether the column shows no tabs at all.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Moves the places of the tabs after a closed one.
    ///
    /// When a tab is closed, the tabs behind it move one place forward in the
    /// tab bar of the window. The columns have to move them too, or they
    /// would point at the wrong files.
    pub fn shift_after(&mut self, closed: usize) {
        for tab in &mut self.tabs {
            if *tab > closed {
                *tab -= 1;
            }
        }
    }
}

/// The columns of the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Groups {
    /// The columns, from left to right.
    columns: Vec<Group>,
    /// Which column is the active one.
    active: usize,
}

impl Groups {
    /// A window with no column at all.
    pub fn new() -> Self {
        Groups {
            columns: Vec::new(),
            active: 0,
        }
    }

    /// The number of columns.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Which column is the active one.
    pub fn active(&self) -> usize {
        self.active
    }

    /// The active column.
    pub fn active_group(&self) -> Option<&Group> {
        self.columns.get(self.active)
    }

    /// The tab in front of the active column, by its place in the tab bar.
    pub fn front(&self) -> Option<usize> {
        self.active_group().and_then(|group| group.front())
    }

    /// The tab in front of a column, by its place in the tab bar.
    pub fn front_of(&self, column: usize) -> Option<usize> {
        self.columns.get(column).and_then(|group| group.front())
    }

    /// The tabs of a column.
    pub fn tabs_of(&self, column: usize) -> &[usize] {
        self.columns.get(column).map(|group| group.tabs()).unwrap_or(&[])
    }

    /// Which column shows the tab with that place.
    pub fn column_of(&self, tab: usize) -> Option<usize> {
        self.columns.iter().position(|group| group.holds(tab))
    }

    /// Makes a column the active one.
    ///
    /// Returns `false` when there is no such column.
    pub fn activate(&mut self, column: usize) -> bool {
        if column >= self.columns.len() {
            return false;
        }
        self.active = column;
        true
    }

    /// Puts a tab in the active column and brings it to the front.
    ///
    /// A tab that is in another column moves here: a file is in one column at
    /// a time, or the two columns would edit the same buffer behind the
    /// person's back.
    pub fn add(&mut self, tab: usize) {
        // A tab that is already in this column comes to the front without
        // moving: the strip does not shuffle under the cursor.
        if let Some(column) = self.columns.get(self.active)
            && column.holds(tab)
        {
            self.columns[self.active].front_tab(tab);
            return;
        }
        for group in &mut self.columns {
            group.take(tab);
        }
        if self.columns.is_empty() {
            self.columns.push(Group::default());
        }
        let active = self.active.min(self.columns.len() - 1);
        self.columns[active].push(tab);
    }

    /// Opens a new column to the right of the active one, with a tab of its
    /// own, and makes that column the active one.
    ///
    /// Returns the place of the new column.
    pub fn split(&mut self, tab: usize) -> usize {
        if self.columns.is_empty() {
            self.columns.push(Group::default());
        }
        let active = self.active.min(self.columns.len() - 1);
        let mut group = Group::default();
        group.push(tab);
        self.columns.insert(active + 1, group);
        self.active = active + 1;
        self.active
    }

    /// Brings a tab to the front of the column that shows it, and makes that
    /// column the active one.
    ///
    /// Returns `false` when no column shows that tab.
    pub fn focus(&mut self, tab: usize) -> bool {
        let Some(column) = self.column_of(tab) else {
            return false;
        };
        self.active = column;
        self.columns[column].front_tab(tab);
        true
    }

    /// Takes a tab out of the window.
    ///
    /// The column it was in goes away when it was its last tab, unless it is
    /// the only column there is: a window always shows something.
    pub fn remove(&mut self, tab: usize) -> bool {
        let Some(column) = self.column_of(tab) else {
            return false;
        };
        self.columns[column].take(tab);
        // The tabs behind the closed one moved one place forward in the tab
        // bar: the columns have to move them too.
        for group in &mut self.columns {
            group.shift_after(tab);
        }
        if self.columns[column].is_empty() && self.columns.len() > 1 {
            self.columns.remove(column);
            if self.active > column {
                self.active -= 1;
            }
            self.active = self.active.min(self.columns.len() - 1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_with_one_column_shows_one_tab() {
        let mut groups = Groups::new();
        groups.add(0);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups.front(), Some(0));
        assert_eq!(groups.tabs_of(0), &[0]);
    }

    #[test]
    fn a_tab_that_is_opened_again_comes_to_the_front() {
        let mut groups = Groups::new();
        groups.add(0);
        groups.add(1);
        groups.add(0);
        assert_eq!(groups.front(), Some(0));
        assert_eq!(groups.tabs_of(0), &[0, 1]);
    }

    #[test]
    fn a_split_makes_a_column_of_its_own() {
        let mut groups = Groups::new();
        groups.add(0);
        let column = groups.split(1);
        assert_eq!(column, 1);
        assert_eq!(groups.len(), 2);
        // The new column is the active one, and it shows the new tab.
        assert_eq!(groups.active(), 1);
        assert_eq!(groups.front(), Some(1));
        // The old column still shows the tab it had.
        assert_eq!(groups.tabs_of(0), &[0]);
    }

    #[test]
    fn each_column_knows_its_own_tabs() {
        let mut groups = Groups::new();
        groups.add(0);
        groups.split(1);
        groups.activate(0);
        groups.add(2);
        assert_eq!(groups.tabs_of(0), &[0, 2]);
        assert_eq!(groups.tabs_of(1), &[1]);
        assert_eq!(groups.column_of(2), Some(0));
        assert_eq!(groups.column_of(1), Some(1));
    }

    #[test]
    fn a_file_that_is_in_another_column_moves_to_the_active_one() {
        let mut groups = Groups::new();
        groups.add(0);
        groups.split(1);
        groups.activate(1);
        groups.add(0);
        assert_eq!(groups.tabs_of(0), &[] as &[usize]);
        assert_eq!(groups.tabs_of(1), &[1, 0]);
        assert_eq!(groups.front(), Some(0));
    }

    #[test]
    fn a_tab_that_is_shown_somewhere_is_brought_to_the_front_there() {
        let mut groups = Groups::new();
        groups.add(0);
        groups.split(1);
        groups.activate(1);
        assert!(groups.focus(0));
        assert_eq!(groups.active(), 0);
        assert_eq!(groups.front(), Some(0));
    }

    #[test]
    fn a_tab_that_is_nowhere_is_not_brought_to_the_front() {
        let mut groups = Groups::new();
        groups.add(0);
        assert!(!groups.focus(7));
    }

    #[test]
    fn the_last_tab_of_a_column_takes_the_column_with_it() {
        let mut groups = Groups::new();
        groups.add(0);
        groups.split(1);
        groups.remove(1);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups.front(), Some(0));
    }

    #[test]
    fn a_window_with_one_column_keeps_it_when_its_tab_goes() {
        let mut groups = Groups::new();
        groups.add(0);
        groups.remove(0);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups.front(), None);
    }

    #[test]
    fn the_tabs_behind_a_closed_one_move_one_place_forward() {
        let mut groups = Groups::new();
        groups.add(0);
        groups.add(1);
        groups.add(2);
        groups.split(3);
        // The tab in front of the second column is 3; the first one closes.
        groups.remove(0);
        assert_eq!(groups.tabs_of(0), &[0, 1]);
        assert_eq!(groups.tabs_of(1), &[2]);
        assert_eq!(groups.front_of(1), Some(2));
    }

    #[test]
    fn the_active_column_stays_the_active_one_when_another_column_goes() {
        let mut groups = Groups::new();
        groups.add(0);
        groups.split(1);
        groups.split(2);
        assert_eq!(groups.active(), 2);
        // The column in the middle goes away: the active one is still the
        // one that was active.
        groups.remove(1);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups.active(), 1);
        // The tab that was the third one moved one place forward in the tab
        // bar of the window, because the second one closed.
        assert_eq!(groups.front(), Some(1));
    }
}
