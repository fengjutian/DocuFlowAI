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

use crate::zoom::{self, Zoom};

/// What a setting is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The size at which the highlighter gives up, in bytes.
    Bytes,
    /// The number of lines at which the highlighter gives up.
    Lines,
    /// The size of the text of the document, in percent.
    FontSize,
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

/// One command whose key can be seen and changed here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRow {
    /// The id of the command.
    pub command: String,
    /// The name of the command, for the person.
    pub title: String,
    /// The key it has now, the way a person reads it.
    pub key: String,
}

/// How many rows the panel shows at once.
///
/// The panel lists every command of the editor, and a list that is taller
/// than the window is a list whose end nobody sees: the rows around the one
/// that is picked are the ones that are shown.
const VISIBLE_ROWS: usize = 14;

/// The settings panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The settings it lists.
    pub items: Vec<Item>,
    /// Which row the user is on: the settings first, then the keys.
    pub selected: usize,
    /// The commands whose keys can be seen and changed here.
    keys: Vec<KeyRow>,
    /// Whether the panel is waiting for the key that is to be given.
    recording: bool,
}

impl Settings {
    /// The panel for a policy, with the values of that policy picked.
    pub fn new(policy: HighlightPolicy, zoom: Zoom) -> Self {
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
                Kind::FontSize => continue,
            };
            item.picked = nearest(&item.choices, wanted);
        }
        // The size of the text is a setting of its own: it is remembered
        // between the runs, so it is written down in the file of the settings.
        items.push(Item {
            kind: Kind::FontSize,
            title: "字号".to_owned(),
            choices: zoom::STEPS
                .iter()
                .map(|step| Choice {
                    label: format!("{step}%"),
                    value: *step as u64,
                })
                .collect(),
            picked: zoom.nearest_step(),
        });
        Settings {
            items,
            selected: 0,
            keys: Vec::new(),
            recording: false,
        }
    }

    /// The panel with the values of a policy and the keys of the commands.
    pub fn with_keys(policy: HighlightPolicy, zoom: Zoom, keys: Vec<KeyRow>) -> Self {
        let mut settings = Self::new(policy, zoom);
        settings.keys = keys;
        settings
    }

    /// What the setting the user is on is about.
    pub fn kind_of_the_picked(&self) -> Option<Kind> {
        if self.on_a_key() {
            return None;
        }
        self.items.get(self.selected).map(|item| item.kind)
    }

    /// How many rows the panel has.
    pub fn len(&self) -> usize {
        self.items.len() + self.keys.len()
    }

    /// Whether the row the user is on is one of the keys.
    pub fn on_a_key(&self) -> bool {
        self.selected >= self.items.len()
    }

    /// Whether the panel is waiting for the key that is to be given.
    pub fn recording(&self) -> bool {
        self.recording
    }

    /// Moves to the row above or below.
    pub fn move_selection(&mut self, delta: isize) {
        if self.len() == 0 {
            return;
        }
        let count = self.len() as isize;
        let next = self.selected as isize + delta;
        // The list does not wrap around: the row below the last one is
        // nowhere, and a panel that jumps back to the top is a panel the user
        // loses track of.
        self.selected = next.clamp(0, count - 1) as usize;
    }

    /// Changes the value of the setting the user is on.
    ///
    /// Returns whether the value changed: a setting that is already at the
    /// end of its steps does not move, and a row of a key is not a value.
    pub fn change_value(&mut self, delta: isize) -> bool {
        if self.on_a_key() {
            return false;
        }
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

    /// Starts waiting for the key that goes to the command that is picked.
    ///
    /// Returns whether the panel is waiting now: Enter on a setting that is
    /// not a key does nothing.
    pub fn begin_recording(&mut self) -> bool {
        if !self.on_a_key() {
            return false;
        }
        self.recording = true;
        true
    }

    /// Stops waiting for a key, and keeps the key the command had.
    pub fn cancel_recording(&mut self) {
        self.recording = false;
    }

    /// The command whose key is picked, when a key is picked.
    pub fn selected_key(&self) -> Option<&KeyRow> {
        if !self.on_a_key() {
            return None;
        }
        self.keys.get(self.selected - self.items.len())
    }

    /// Gives the picked command the key that was pressed.
    ///
    /// The key is written the way a person reads it: `Ctrl+Shift+P`. A key of
    /// a command that is not in the list is ignored.
    pub fn set_key(&mut self, command: &str, key: &str) {
        for row in &mut self.keys {
            if row.command == command {
                row.key = key.to_owned();
            }
        }
        self.recording = false;
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
                Kind::FontSize => {}
            }
        }
        policy
    }

    /// The size of the text the panel describes.
    pub fn zoom(&self) -> Zoom {
        for item in &self.items {
            if item.kind != Kind::FontSize {
                continue;
            }
            let Some(choice) = item.choices.get(item.picked) else {
                break;
            };
            return Zoom::of(choice.value as f32);
        }
        Zoom::default()
    }

    /// The lines of the settings, the way the panel draws them.
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

    /// Every row of the panel: the settings, then the keys.
    pub fn all_rows(&self) -> Vec<Row> {
        let mut rows = self.rows();
        for (index, key) in self.keys.iter().enumerate() {
            let selected = self.items.len() + index == self.selected;
            let value = if selected && self.recording {
                "按新的键…".to_owned()
            } else if key.key.is_empty() {
                "（没有键）".to_owned()
            } else {
                key.key.clone()
            };
            rows.push(Row {
                selected,
                title: key.title.clone(),
                value,
            });
        }
        rows
    }

    /// The rows the panel shows now: the picked one and the ones around it.
    pub fn visible_rows(&self) -> Vec<Row> {
        let rows = self.all_rows();
        if rows.len() <= VISIBLE_ROWS {
            return rows;
        }
        // The picked row is kept in the middle of the window when it can be,
        // so that the list does not jump one row at a time.
        let half = VISIBLE_ROWS / 2;
        let start = self
            .selected
            .saturating_sub(half)
            .min(rows.len() - VISIBLE_ROWS);
        rows[start..start + VISIBLE_ROWS].to_vec()
    }

    /// The line under the panel, telling the user what the keys do.
    pub fn hint(&self) -> &'static str {
        "↑↓ 选 · ←→ 改值 · Enter 改键位（再按新键，Esc 取消）· Esc 关闭"
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
        let panel = Settings::new(
            HighlightPolicy {
                max_bytes: 2_000_000,
                max_lines: 20_000,
            },
            Zoom::default(),
        );
        assert_eq!(panel.items.len(), 3);
        assert_eq!(panel.items[0].picked, 4);
        assert_eq!(panel.items[1].picked, 2);
        assert_eq!(panel.policy().max_bytes, 2_000_000);
        assert_eq!(panel.policy().max_lines, 20_000);
    }

    #[test]
    fn the_panel_starts_on_the_default_values() {
        let panel = Settings::new(HighlightPolicy::default(), Zoom::default());
        assert_eq!(panel.policy(), HighlightPolicy::default());
    }

    #[test]
    fn a_value_that_is_not_one_of_the_choices_picks_the_nearest() {
        let panel = Settings::new(
            HighlightPolicy {
                max_bytes: 700_000,
                max_lines: 50_000,
            },
            Zoom::default(),
        );
        // 700 KB is nearer to 500 KB than to 1 MB.
        assert_eq!(panel.policy().max_bytes, 500_000);
    }

    #[test]
    fn changing_the_value_moves_to_the_next_choice() {
        let mut panel = Settings::new(HighlightPolicy::default(), Zoom::default());
        assert!(panel.change_value(1));
        assert_eq!(panel.policy().max_bytes, 2_000_000);
        assert!(panel.change_value(-1));
        assert_eq!(panel.policy().max_bytes, 1_000_000);
    }

    #[test]
    fn a_value_at_the_end_of_its_steps_does_not_move() {
        let mut panel = Settings::new(HighlightPolicy::default(), Zoom::default());
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
        let mut panel = Settings::new(HighlightPolicy::default(), Zoom::default());
        panel.move_selection(-1);
        assert_eq!(panel.selected, 0);
        panel.move_selection(5);
        assert_eq!(panel.selected, panel.items.len() - 1);
        panel.move_selection(-5);
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn the_rows_show_the_setting_and_its_value() {
        let panel = Settings::new(HighlightPolicy::default(), Zoom::default());
        let rows = panel.rows();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].selected);
        assert!(!rows[1].selected);
        assert_eq!(rows[0].title, "高亮大小上限");
        assert_eq!(rows[0].value, "1 MB");
        assert_eq!(rows[1].value, "50,000 行");
        assert_eq!(rows[2].title, "字号");
        assert_eq!(rows[2].value, "100%");
    }

    fn keys() -> Vec<KeyRow> {
        vec![
            KeyRow {
                command: "editor.format".to_owned(),
                title: "编辑 · 格式化文档".to_owned(),
                key: "Alt+Shift+F".to_owned(),
            },
            KeyRow {
                command: "editor.rename".to_owned(),
                title: "编辑 · 重命名".to_owned(),
                key: "F2".to_owned(),
            },
        ]
    }

    #[test]
    fn the_keys_of_the_commands_come_after_the_settings() {
        let panel = Settings::with_keys(HighlightPolicy::default(), Zoom::default(), keys());
        let rows = panel.all_rows();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[3].title, "编辑 · 格式化文档");
        assert_eq!(rows[3].value, "Alt+Shift+F");
        assert!(!rows[3].selected, "the first setting is picked");
        assert_eq!(panel.len(), 5);
    }

    #[test]
    fn the_selection_walks_from_a_setting_to_a_key() {
        let mut panel = Settings::with_keys(HighlightPolicy::default(), Zoom::default(), keys());
        panel.move_selection(3);
        assert!(panel.on_a_key(), "the fourth row is a key");
        assert_eq!(
            panel.selected_key().map(|row| row.command.as_str()),
            Some("editor.format")
        );
        panel.move_selection(1);
        assert_eq!(
            panel.selected_key().map(|row| row.command.as_str()),
            Some("editor.rename")
        );
        panel.move_selection(9);
        assert_eq!(panel.selected, 4, "the selection stops at the last row");
    }

    #[test]
    fn the_arrows_do_not_change_the_value_of_a_key() {
        let mut panel = Settings::with_keys(HighlightPolicy::default(), Zoom::default(), keys());
        panel.move_selection(3);
        assert!(!panel.change_value(1), "a key is not a value");
        assert_eq!(panel.policy(), HighlightPolicy::default());
    }

    #[test]
    fn enter_on_a_key_waits_for_a_key_of_its_own() {
        let mut panel = Settings::with_keys(HighlightPolicy::default(), Zoom::default(), keys());
        assert!(!panel.begin_recording(), "the first row is a setting");
        panel.move_selection(4);
        assert!(panel.begin_recording());
        assert!(panel.recording());
        let rows = panel.all_rows();
        assert_eq!(rows[4].value, "按新的键…", "the row says what it waits for");
        panel.cancel_recording();
        assert!(!panel.recording());
        assert_eq!(panel.all_rows()[4].value, "F2", "the old key is still there");
    }

    #[test]
    fn the_key_that_is_given_is_the_key_of_the_command_from_then_on() {
        let mut panel = Settings::with_keys(HighlightPolicy::default(), Zoom::default(), keys());
        panel.move_selection(3);
        panel.begin_recording();
        panel.set_key("editor.format", "Ctrl+Alt+K");
        assert_eq!(panel.all_rows()[3].value, "Ctrl+Alt+K");
        assert!(!panel.recording(), "the panel has its answer");
    }

    #[test]
    fn the_panel_shows_the_rows_around_the_one_that_is_picked() {
        let many: Vec<KeyRow> = (0..40)
            .map(|index| KeyRow {
                command: format!("editor.command{index}"),
                title: format!("命令 {index}"),
                key: "Ctrl+K".to_owned(),
            })
            .collect();
        let mut panel = Settings::with_keys(HighlightPolicy::default(), Zoom::default(), many);
        let rows = panel.visible_rows();
        assert_eq!(rows.len(), VISIBLE_ROWS, "the panel is not taller than that");
        assert!(rows[0].selected, "the first row is in the window");

        panel.move_selection(30);
        let rows = panel.visible_rows();
        assert_eq!(rows.len(), VISIBLE_ROWS);
        assert!(
            rows.iter().any(|row| row.selected),
            "the picked row is always shown"
        );
        assert_eq!(
            rows.iter().filter(|row| row.selected).count(),
            1,
            "one row is picked"
        );
    }
}
