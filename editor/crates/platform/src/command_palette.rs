//! The panel that lists the commands of the editor.
//!
//! It is opened with `Ctrl+Shift+P`, the way VS Code opens its own: the
//! panel shows every command the editor has, the person writes a few letters
//! to narrow the list down, and the command that is picked is run.
//!
//! Running a command from the panel is the same thing as pressing its key:
//! both go through the one place that knows what a command does.

use codeweaver_render::{Color, FrameScene, Rect, TextFamily, TextStyle};
use codeweaver_ui::TextMeasurer;
use winit::keyboard::{Key, NamedKey};

use crate::commands::{Command, CommandRegistry, Context};
use crate::rename::RenameBox;

/// The width of the panel.
const WIDTH: f32 = 640.0;
/// The height of one command of the list.
const ITEM_HEIGHT: f32 = 24.0;
/// How many commands are shown at once.
const VISIBLE: usize = 10;
/// The padding around the list.
const PADDING: f32 = 8.0;
/// The height of the line the text is written in.
const HEADER_HEIGHT: f32 = 36.0;

/// What the panel wants the editor to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteCommand {
    /// Nothing.
    None,
    /// The panel is closed.
    Close,
    /// The command with that id is to be run.
    Run(String),
}

/// One command of the list.
#[derive(Clone, Debug)]
pub struct PaletteRow {
    /// The command itself.
    pub command: Command,
    /// Whether it can be used right now.
    pub enabled: bool,
}

/// The panel that lists the commands of the editor.
pub struct CommandPalette {
    /// The text that is written to narrow the list down.
    input: RenameBox,
    /// Every command that is on offer.
    rows: Vec<PaletteRow>,
    /// The rows that are shown now, as places in `rows`.
    shown: Vec<usize>,
    /// Which of the shown rows is picked.
    selected: usize,
}

impl CommandPalette {
    /// Opens the panel with the commands that the editor has.
    ///
    /// The commands are shown in groups, the way a person reads them: the
    /// group of the command comes first, and the title of the command inside
    /// the group.
    pub fn open(registry: &CommandRegistry, context: &Context) -> Self {
        let mut rows: Vec<PaletteRow> = registry
            .all()
            .iter()
            .map(|command| PaletteRow {
                enabled: command.when.allows(context),
                command: command.clone(),
            })
            .collect();
        rows.sort_by(|left, right| {
            left.command
                .category
                .cmp(&right.command.category)
                .then_with(|| left.command.title.cmp(&right.command.title))
        });
        let shown = (0..rows.len()).collect();
        Self {
            input: RenameBox::message("命令"),
            rows,
            shown,
            selected: 0,
        }
    }

    /// The commands that are shown now.
    pub fn rows(&self) -> Vec<&PaletteRow> {
        self.shown
            .iter()
            .filter_map(|index| self.rows.get(*index))
            .collect()
    }

    /// The command that is picked.
    pub fn selected_command(&self) -> Option<&Command> {
        self.shown
            .get(self.selected)
            .and_then(|index| self.rows.get(*index))
            .map(|row| &row.command)
    }

    /// The text that is written in the box.
    pub fn input(&self) -> &RenameBox {
        &self.input
    }

    /// Whether the panel has any command to show.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Gives a key to the panel.
    pub fn handle_key<T: AsRef<str>>(
        &mut self,
        key: &Key<T>,
        text: Option<&str>,
        control: bool,
    ) -> PaletteCommand {
        match key {
            Key::Named(NamedKey::Escape) => return PaletteCommand::Close,
            Key::Named(NamedKey::Enter) => {
                let Some(command) = self.selected_command() else {
                    return PaletteCommand::None;
                };
                return PaletteCommand::Run(command.id.clone());
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.select_next();
                return PaletteCommand::None;
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.select_previous();
                return PaletteCommand::None;
            }
            _ => {}
        }
        // Everything else goes into the box: the list follows the text.
        self.input.handle_key(key, text, control);
        self.filter();
        PaletteCommand::None
    }

    /// Keeps the commands that hold what is written.
    ///
    /// The case of the letters does not count: `for` finds「格式化」as well
    /// as `FOR`.
    fn filter(&mut self) {
        let needle = self.input.text().trim().to_lowercase();
        self.shown = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                if needle.is_empty() {
                    return true;
                }
                row.command.title.to_lowercase().contains(&needle)
                    || row.command.id.to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect();
        self.selected = 0;
    }

    /// Picks the command after this one.
    fn select_next(&mut self) {
        if self.shown.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.shown.len();
    }

    /// Picks the command before this one.
    fn select_previous(&mut self) {
        if self.shown.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.shown.len() - 1
        } else {
            self.selected - 1
        };
    }
}

/// Draws the panel over the window.
///
/// The panel covers the window: it is what the person is looking at, and the
/// text behind it is not.
pub fn draw(
    scene: &mut FrameScene,
    viewport: Rect,
    palette: &CommandPalette,
    measurer: &mut dyn TextMeasurer,
) {
    scene.push_quad(viewport, Color::rgb(0.055, 0.063, 0.082), viewport);

    let rows = palette.rows();
    let shown = rows.len().min(VISIBLE);
    let width = WIDTH.min(viewport.size.width - 40.0).max(200.0);
    let height = HEADER_HEIGHT + shown as f32 * ITEM_HEIGHT + PADDING * 2.0;
    let panel = Rect::new(
        (viewport.size.width - width) / 2.0,
        viewport.origin.y + 32.0,
        width,
        height,
    );
    scene.push_quad(panel, Color::rgb(0.10, 0.11, 0.15), viewport);
    scene.push_quad(
        Rect::new(panel.origin.x, panel.origin.y, panel.size.width, 2.0),
        Color::rgb(0.30, 0.58, 0.96),
        viewport,
    );

    // The line the text is written in.
    let header = Rect::new(
        panel.origin.x + PADDING,
        panel.origin.y + PADDING,
        panel.size.width - PADDING * 2.0,
        HEADER_HEIGHT,
    );
    let label = "命令";
    let label_width = measurer.measure(label, 13.0).width;
    scene.push_text(
        label.to_owned(),
        Rect::new(
            header.origin.x,
            header.origin.y + 6.0,
            label_width + 4.0,
            20.0,
        ),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.55, 0.60, 0.68),
            font_size: 13.0,
            line_height: 20.0,
            family: TextFamily::SansSerif,
        },
    );
    let text = palette.input().text();
    let shown_text = if text.is_empty() { "（还没有写）" } else { text };
    scene.push_text(
        shown_text.to_owned(),
        Rect::new(
            header.origin.x + label_width + 8.0,
            header.origin.y + 4.0,
            (header.size.width - label_width - 16.0).max(1.0),
            24.0,
        ),
        TextStyle {
            clip: header,
            color: if text.is_empty() {
                Color::rgb(0.38, 0.42, 0.50)
            } else {
                Color::rgb(0.86, 0.89, 0.94)
            },
            font_size: 14.0,
            line_height: 20.0,
            family: TextFamily::SansSerif,
        },
    );

    // The commands.
    for (index, row) in rows.iter().take(VISIBLE).enumerate() {
        let y = panel.origin.y + PADDING + HEADER_HEIGHT + index as f32 * ITEM_HEIGHT;
        if index == palette.selected {
            scene.push_quad(
                Rect::new(panel.origin.x, y, panel.size.width, ITEM_HEIGHT),
                Color::rgb(0.18, 0.30, 0.52),
                viewport,
            );
        }
        // A command that cannot be used is drawn dim: it is there, but it is
        // not for now.
        let color = if row.enabled {
            Color::rgb(0.88, 0.90, 0.95)
        } else {
            Color::rgb(0.45, 0.49, 0.57)
        };
        scene.push_text(
            row.command.title.clone(),
            Rect::new(
                panel.origin.x + PADDING + 4.0,
                y + 2.0,
                panel.size.width * 0.7,
                ITEM_HEIGHT,
            ),
            TextStyle {
                clip: viewport,
                color,
                font_size: 14.0,
                line_height: 20.0,
                family: TextFamily::SansSerif,
            },
        );
        if let Some(keybinding) = row.command.keybinding.as_ref() {
            let keys = keybinding.text();
            let width = measurer.measure(&keys, 12.0).width;
            scene.push_text(
                keys,
                Rect::new(
                    panel.origin.x + panel.size.width - width - PADDING - 4.0,
                    y + 4.0,
                    width + 2.0,
                    18.0,
                ),
                TextStyle {
                    clip: viewport,
                    color: Color::rgb(0.52, 0.57, 0.66),
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
    use crate::commands::{When};

    fn registry() -> CommandRegistry {
        let mut registry = CommandRegistry::new();
        registry
            .register(
                Command::new("editor.format", "格式化文档", "编辑")
                    .with_key("alt+shift+f")
                    .expect("the key is read"),
            )
            .expect("the command is added");
        registry
            .register(
                Command::new("editor.rename", "重命名", "编辑").when(When::HasNameAtCaret),
            )
            .expect("the command is added");
        registry
            .register(
                Command::new("terminal.open", "打开终端", "终端")
                    .with_key("ctrl+shift+t")
                    .expect("the key is read"),
            )
            .expect("the command is added");
        registry
    }

    #[test]
    fn the_panel_lists_the_commands_of_the_editor() {
        let palette = CommandPalette::open(&registry(), &Context::default());
        let titles: Vec<&str> = palette
            .rows()
            .into_iter()
            .map(|row| row.command.title.as_str())
            .collect();
        assert_eq!(titles, vec!["格式化文档", "重命名", "打开终端"]);
    }

    #[test]
    fn writing_in_the_box_narrows_the_list_down() {
        let mut palette = CommandPalette::open(&registry(), &Context::default());
        for letter in ["t", "e", "r"] {
            palette.handle_key(&Key::Character(letter), Some(letter), false);
        }
        let titles: Vec<&str> = palette
            .rows()
            .iter()
            .map(|row| row.command.title.as_str())
            .collect();
        assert_eq!(titles, vec!["打开终端"]);
    }

    #[test]
    fn the_letters_are_matched_without_the_case_counting() {
        let mut palette = CommandPalette::open(&registry(), &Context::default());
        palette.handle_key(&Key::Character("F"), Some("F"), false);
        let titles: Vec<&str> = palette
            .rows()
            .iter()
            .map(|row| row.command.title.as_str())
            .collect();
        assert_eq!(titles, vec!["格式化文档"]);
    }

    #[test]
    fn a_command_that_cannot_be_used_is_still_listed() {
        let palette = CommandPalette::open(&registry(), &Context::default());
        let rename = palette
            .rows()
            .into_iter()
            .find(|row| row.command.id == "editor.rename")
            .expect("the command is listed");
        assert!(!rename.enabled, "there is no name at the caret");
    }

    #[test]
    fn the_enter_key_runs_the_command_that_is_picked() {
        let mut palette = CommandPalette::open(&registry(), &Context::default());
        palette.handle_key(&Key::<&str>::Named(NamedKey::ArrowDown), None, false);
        let command = palette.handle_key(&Key::<&str>::Named(NamedKey::Enter), None, false);
        assert_eq!(command, PaletteCommand::Run("editor.rename".to_owned()));
    }

    #[test]
    fn the_escape_key_closes_the_panel() {
        let mut palette = CommandPalette::open(&registry(), &Context::default());
        assert_eq!(
            palette.handle_key(&Key::<&str>::Named(NamedKey::Escape), None, false),
            PaletteCommand::Close
        );
    }
}
