//! The settings the user can change.
//!
//! A few switches of the editor used to live on the command line: the sizes
//! at which the highlighter gives up. A setting that can only be changed by
//! restarting the editor with a different word on the command line is a
//! setting most people never change, so the editor has a panel for them.
//!
//! The panel is a list of settings, each with a few choices. The choices are
//! steps rather than free text: a number that has to be typed cannot be
//! changed without a keyboard layout, and a setting with a handful of
//! sensible values does not need to take any value at all.

use codeweaver_syntax::HighlightPolicy;

/// What a setting is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The size at which the highlighter gives up, in bytes.
    Bytes,
    /// The number of lines at which the highlighter gives up.
    Lines,
}

/// One value a setting can take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// How the choice reads to a person.
    pub label: String,
    /// The number behind the choice.
    pub value: u64,
}

/// One setting the user can change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What the setting is about.
    pub kind: Kind,
    /// What the setting is, in words.
    pub title: String,
    /// The choices, from the smallest to the largest.
    pub choices: Vec<Choice>,
    /// Which choice is picked.
    pub picked: usize,
}

/// One line of the panel, ready to be drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Whether this is the setting the user is on.
    pub selected: bool,
    /// What the setting is, in words.
    pub title: String,
    /// The value it has now.
    pub value: String,
}

/// The settings panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The settings it lists.
    pub items: Vec<Item>,
    /// Which setting the user is on.
    pub selected: usize,
}

impl Settings {
    /// The panel for a policy, with the values of that policy picked.
    pub fn new(policy: HighlightPolicy) -> Self {
        let bytes = Item {
            kind: Kind::Bytes,
            title: "高亮大小上限".to_owned(),
            choices: vec![
                choice("100 KB", 100_000),
                choice("250 KB", 250_000),
                choice("500 KB", 500_000),
                choice("1 MB", 1_000_000),
                choice("2 MB", 2_000_000),
                choice("5 MB", 5_000_000),
                choice("10 MB", 10_000_000),
                choice("20 MB", 20_000_000),
            ],
            picked: 0,
        };
        let lines = Item {
            kind: Kind::Lines,
            title: "高亮行数上限".to_owned(),
            choices: vec![
                choice("5,000 行", 5_000),
                choice("10,000 行", 10_000),
                choice("20,000 行", 20_000),
                choice("50,000 行", 50_000),
                choice("100,000 行", 100_000),
                choice("200,000 行", 200_000),
            ],
            picked: 0,
        };
        let mut items = vec![bytes, lines];
        for item in &mut items {
            let wanted = match item.kind {
                Kind::Bytes => policy.max_bytes as u64,
                Kind::Lines => policy.max_lines as u64,
            };
            item.picked = nearest(&item.choices, wanted);
        }
        Settings { items, selected: 0 }
    }

    /// Moves to the setting above or below.
    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let count = self.items.len() as isize;
        let next = self.selected as isize + delta;
        // The list does not wrap around: the setting below the last one is
        // nowhere, and a panel that jumps back to the top is a panel the user
        // loses track of.
        self.selected = next.clamp(0, count - 1) as usize;
    }

    /// Changes the value of the setting the user is on.
    ///
    /// Returns whether the value changed: a setting that is already at the
    /// end of its steps does not move.
    pub fn change_value(&mut self, delta: isize) -> bool {
        let Some(item) = self.items.get_mut(self.selected) else {
            return false;
        };
        if item.choices.is_empty() {
            return false;
        }
        let count = item.choices.len() as isize;
        let next = item.picked as isize + delta;
        if next < 0 || next >= count {
            return false;
        }
        item.picked = next as usize;
        true
    }

    /// The policy the panel describes.
    pub fn policy(&self) -> HighlightPolicy {
        let mut policy = HighlightPolicy::default();
        for item in &self.items {
            let Some(choice) = item.choices.get(item.picked) else {
                continue;
            };
            match item.kind {
                Kind::Bytes => policy.max_bytes = choice.value as usize,
                Kind::Lines => policy.max_lines = choice.value as usize,
            }
        }
        policy
    }

    /// The lines the panel draws.
    pub fn rows(&self) -> Vec<Row> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| Row {
                selected: index == self.selected,
                title: item.title.clone(),
                value: item
                    .choices
                    .get(item.picked)
                    .map(|choice| choice.label.clone())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// The line under the panel, telling the user what the keys do.
    pub fn hint(&self) -> &'static str {
        "↑↓ 选设置 · ←→ 改值 · Enter 关闭"
    }

    /// The name of the panel.
    pub fn title(&self) -> &'static str {
        "设置"
    }
}

/// One choice of a setting.
fn choice(label: &str, value: u64) -> Choice {
    Choice {
        label: label.to_owned(),
        value,
    }
}

/// The choice whose value is the nearest to the one asked for.
fn nearest(choices: &[Choice], wanted: u64) -> usize {
    choices
        .iter()
        .enumerate()
        .min_by_key(|(_, choice)| choice.value.abs_diff(wanted))
        .map(|(index, _)| index)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_starts_on_the_values_the_editor_has() {
        let panel = Settings::new(HighlightPolicy {
            max_bytes: 2_000_000,
            max_lines: 20_000,
        });
        assert_eq!(panel.items.len(), 2);
        assert_eq!(panel.items[0].picked, 4);
        assert_eq!(panel.items[1].picked, 2);
        assert_eq!(panel.policy().max_bytes, 2_000_000);
        assert_eq!(panel.policy().max_lines, 20_000);
    }

    #[test]
    fn the_panel_starts_on_the_default_values() {
        let panel = Settings::new(HighlightPolicy::default());
        assert_eq!(panel.policy(), HighlightPolicy::default());
    }

    #[test]
    fn a_value_that_is_not_one_of_the_choices_picks_the_nearest() {
        let panel = Settings::new(HighlightPolicy {
            max_bytes: 700_000,
            max_lines: 50_000,
        });
        // 700 KB is nearer to 500 KB than to 1 MB.
        assert_eq!(panel.policy().max_bytes, 500_000);
    }

    #[test]
    fn changing_the_value_moves_to_the_next_choice() {
        let mut panel = Settings::new(HighlightPolicy::default());
        assert!(panel.change_value(1));
        assert_eq!(panel.policy().max_bytes, 2_000_000);
        assert!(panel.change_value(-1));
        assert_eq!(panel.policy().max_bytes, 1_000_000);
    }

    #[test]
    fn a_value_at_the_end_of_its_steps_does_not_move() {
        let mut panel = Settings::new(HighlightPolicy::default());
        for _ in 0..10 {
            panel.change_value(1);
        }
        let last = panel.policy().max_bytes;
        assert!(!panel.change_value(1), "the value is already at the last step");
        assert_eq!(panel.policy().max_bytes, last);
        assert!(panel.change_value(-1), "the value moves back down");
    }

    #[test]
    fn the_selection_stays_inside_the_list() {
        let mut panel = Settings::new(HighlightPolicy::default());
        panel.move_selection(-1);
        assert_eq!(panel.selected, 0);
        panel.move_selection(5);
        assert_eq!(panel.selected, panel.items.len() - 1);
        panel.move_selection(-5);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn the_rows_show_the_setting_and_its_value() {
        let panel = Settings::new(HighlightPolicy::default());
        let rows = panel.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].selected);
        assert!(!rows[1].selected);
        assert_eq!(rows[0].title, "高亮大小上限");
        assert_eq!(rows[0].value, "1 MB");
        assert_eq!(rows[1].value, "50,000 行");
    }
}
