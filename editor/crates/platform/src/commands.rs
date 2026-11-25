//! The commands of the editor.
//!
//! A command is one thing the editor can do. It has a name for the person
//! (`title`), a name for the code (`id`), the key that runs it (`keybinding`)
//! and the condition that says when it can be used (`when`).
//!
//! The registry knows nothing about the editor itself: it says which command
//! is which, and whether it can be used right now. That keeps it testable
//! without a window, and it keeps the keys in one place instead of in twenty
//! branches that each know one feature.

/// A key with the modifiers that go with it.
///
/// It is written the way a person writes it: `ctrl+shift+p`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keybinding {
    /// Whether the control key is part of it.
    pub ctrl: bool,
    /// Whether the alt key is part of it.
    pub alt: bool,
    /// Whether the shift key is part of it.
    pub shift: bool,
    /// Whether the command key is part of it.
    pub cmd: bool,
    /// The key itself: a letter, a digit, or the name of a key (`f9`).
    pub key: String,
}

impl Keybinding {
    /// Reads a keybinding from the way it is written: `ctrl+shift+p`.
    ///
    /// A keybinding that cannot be read is **said to be wrong**: which part
    /// of it is not a key is named, so that the person can fix it.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut binding = Keybinding {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            key: String::new(),
        };
        let parts: Vec<String> = text
            .split('+')
            .map(|part| part.trim().to_lowercase())
            .collect();
        if parts.is_empty() {
            return Err(format!("键位是空的：{text}"));
        }
        for (index, part) in parts.iter().enumerate() {
            let is_the_key = index + 1 == parts.len();
            if !is_the_key {
                match part.as_str() {
                    "ctrl" | "control" => binding.ctrl = true,
                    "alt" | "option" => binding.alt = true,
                    "shift" => binding.shift = true,
                    "cmd" | "command" | "meta" | "win" | "super" => binding.cmd = true,
                    other => return Err(format!("键位 {text} 里的 {other} 不是修饰键")),
                }
                continue;
            }
            if part.is_empty() {
                return Err(format!("键位 {text} 里没有键"));
            }
            binding.key = part.clone();
        }
        Ok(binding)
    }

    /// The keybinding as a person reads it: `Ctrl+Shift+P`.
    pub fn text(&self) -> String {
        let mut text = String::new();
        if self.ctrl {
            text.push_str("Ctrl+");
        }
        if self.alt {
            text.push_str("Alt+");
        }
        if self.shift {
            text.push_str("Shift+");
        }
        if self.cmd {
            text.push_str("Cmd+");
        }
        text.push_str(&self.key.to_uppercase());
        text
    }

    /// Whether this keybinding is the key that was pressed.
    pub fn matches(&self, pressed: &PressedKey) -> bool {
        if self.ctrl != pressed.ctrl
            || self.alt != pressed.alt
            || self.shift != pressed.shift
            || self.cmd != pressed.cmd
        {
            return false;
        }
        if let Some(letter) = pressed.letter {
            return self
                .key
                .chars()
                .next()
                .is_some_and(|key| key.eq_ignore_ascii_case(&letter));
        }
        match pressed.name.as_deref() {
            Some(name) => self.key == name,
            None => false,
        }
    }
}

/// The key that was pressed, and the modifiers that were held with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PressedKey {
    /// The letter that was pressed, when it was a letter or a digit.
    pub letter: Option<char>,
    /// The name of the key that was pressed, when it had one (`f9`).
    pub name: Option<String>,
    /// Whether the control key was held.
    pub ctrl: bool,
    /// Whether the alt key was held.
    pub alt: bool,
    /// Whether the shift key was held.
    pub shift: bool,
    /// Whether the command key was held.
    pub cmd: bool,
}

/// When a command can be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// It can always be used.
    Always,
    /// A file is open.
    HasFile,
    /// There is a name at the caret, to rename it.
    HasNameAtCaret,
    /// The file belongs to a repository, so the changes of it can be shown.
    HasRepository,
}

/// What the commands may ask about.
///
/// The editor tells the registry what is true right now; the registry says
/// which commands that lets it run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// Whether a file is open.
    pub has_file: bool,
    /// Whether there is a name at the caret.
    pub has_name_at_caret: bool,
    /// Whether the open file belongs to a repository.
    pub has_repository: bool,
}

impl When {
    /// Whether the command can be used with what is true right now.
    pub fn allows(&self, context: &Context) -> bool {
        match self {
            When::Always => true,
            When::HasFile => context.has_file,
            When::HasNameAtCaret => context.has_name_at_caret,
            When::HasRepository => context.has_repository,
        }
    }
}

/// A command of the editor.
#[derive(Clone, Debug)]
pub struct Command {
    /// The name of the command, for the code: `editor.format`.
    pub id: String,
    /// The name of the command, for the person: 「格式化文档」.
    pub title: String,
    /// The group the command belongs to, for the person: 「编辑」.
    pub category: String,
    /// The key that runs it, when it has one.
    pub keybinding: Option<Keybinding>,
    /// When it can be used.
    pub when: When,
}

impl Command {
    /// A command with the given name and id.
    pub fn new(id: &str, title: &str, category: &str) -> Self {
        Command {
            id: id.to_owned(),
            title: title.to_owned(),
            category: category.to_owned(),
            keybinding: None,
            when: When::Always,
        }
    }

    /// The command with the key that runs it.
    pub fn with_key(mut self, keybinding: &str) -> Result<Self, String> {
        self.keybinding = Some(Keybinding::parse(keybinding)?);
        Ok(self)
    }

    /// The command that can only be used when something is true.
    pub fn when(mut self, when: When) -> Self {
        self.when = when;
        self
    }
}

/// Two commands that want the same key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The key both of them want.
    pub keybinding: Keybinding,
    /// The command that was registered first.
    pub first: String,
    /// The command that came later.
    pub second: String,
}

/// The commands of the editor.
///
/// The commands are kept in the order of their ids: the list a person reads
/// is the same wherever it is shown, and the tests do not depend on the order
/// the commands were added in.
#[derive(Clone, Debug, Default)]
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    /// A registry without any command in it.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Adds a command.
    ///
    /// A command whose id is already there is **refused**: two commands with
    /// one name are two commands, and only one of them can be the one that
    /// the code asks for.
    pub fn register(&mut self, command: Command) -> Result<(), String> {
        if self.commands.iter().any(|existing| existing.id == command.id) {
            return Err(format!("命令 {} 已经有一个了", command.id));
        }
        let place = self
            .commands
            .iter()
            .position(|existing| existing.id > command.id)
            .unwrap_or(self.commands.len());
        self.commands.insert(place, command);
        Ok(())
    }

    /// All the commands, in the order of their ids.
    pub fn all(&self) -> &[Command] {
        &self.commands
    }

    /// The command with that id.
    pub fn get(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.id == id)
    }

    /// The command that the key runs, if there is one.
    pub fn by_key(&self, pressed: &PressedKey) -> Option<&Command> {
        self.commands
            .iter()
            .find(|command| {
                command
                    .keybinding
                    .as_ref()
                    .is_some_and(|keybinding| keybinding.matches(pressed))
            })
    }

    /// The commands that want the same key.
    ///
    /// Two commands with one key is a mistake that is said out loud: the
    /// person has to know which one of them the key really runs.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        for (index, command) in self.commands.iter().enumerate() {
            let Some(keybinding) = command.keybinding.as_ref() else {
                continue;
            };
            for other in self.commands.iter().skip(index + 1) {
                let Some(other_key) = other.keybinding.as_ref() else {
                    continue;
                };
                if keybinding == other_key {
                    conflicts.push(Conflict {
                        keybinding: keybinding.clone(),
                        first: command.id.clone(),
                        second: other.id.clone(),
                    });
                }
            }
        }
        conflicts
    }

    /// Whether the command can be used right now.
    pub fn is_enabled(&self, id: &str, context: &Context) -> bool {
        self.get(id)
            .is_some_and(|command| command.when.allows(context))
    }
}

/// The commands the editor has.
///
/// This is the one place where they are written down: the key each of them
/// has, the name it is shown with, and when it can be used. The panel of the
/// commands, the state line and the keys all read it from here.
pub fn default_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    let commands: Vec<Command> = vec![
        command("workbench.commandPalette", "命令面板", "视图", "ctrl+shift+p", When::Always),
        command("workbench.settings", "打开设置", "文件", "ctrl+,", When::Always),
        command("workbench.save", "保存文件", "文件", "ctrl+s", When::HasFile),
        command("editor.format", "格式化文档", "编辑", "alt+shift+f", When::HasFile),
        command("editor.rename", "重命名", "编辑", "f2", When::HasNameAtCaret),
        command("editor.codeAction", "代码操作", "编辑", "alt+enter", When::HasFile),
        command("editor.find", "在文件中查找", "编辑", "ctrl+f", When::Always),
        command("search.findInFiles", "在工程中搜索", "搜索", "ctrl+shift+f", When::HasFile),
        command("search.replaceInFiles", "在工程中替换", "搜索", "ctrl+shift+h", When::HasFile),
        command("terminal.toggle", "打开终端", "终端", "ctrl+shift+t", When::Always),
        command("terminal.runCommand", "运行命令…", "终端", "ctrl+shift+u", When::Always),
        command("task.build", "运行构建任务", "终端", "ctrl+shift+b", When::HasFile),
        command("git.showChanges", "显示源代码管理更改", "源代码管理", "ctrl+shift+g", When::HasRepository),
        command("debug.toggleBreakpoint", "切换断点", "调试", "f9", When::HasFile),
        command("debug.start", "开始调试", "调试", "f5", When::Always),
        command("debug.stop", "结束调试", "调试", "f8", When::Always),
        command("debug.evaluate", "调试表达式求值", "调试", "ctrl+shift+e", When::Always),
        command("agent.startTurn", "把当前内容交给 Agent", "Agent", "ctrl+m", When::Always),
    ];
    for command in commands {
        registry
            .register(command)
            .expect("no two commands have one id");
    }
    registry
}

/// A command with its key and its condition.
fn command(
    id: &str,
    title: &str,
    category: &str,
    keybinding: &str,
    when: When,
) -> Command {
    Command::new(id, title, category)
        .with_key(keybinding)
        .expect("the key of a command is written right")
        .when(when)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keybinding_is_read_from_the_way_it_is_written() {
        let binding = Keybinding::parse("ctrl+shift+p").expect("the key is read");
        assert!(binding.ctrl);
        assert!(binding.shift);
        assert!(!binding.alt);
        assert_eq!(binding.key, "p");
    }

    #[test]
    fn a_keybinding_is_written_the_way_a_person_reads_it() {
        let binding = Keybinding::parse("alt+shift+f").expect("the key is read");
        assert_eq!(binding.text(), "Alt+Shift+F");
    }

    #[test]
    fn a_keybinding_that_cannot_be_read_says_which_part_is_wrong() {
        let said = Keybinding::parse("ctrl+strange+p").expect_err("it is not a key");
        assert!(said.contains("strange"), "got {said}");
        let said = Keybinding::parse("ctrl+").expect_err("there is no key");
        assert!(said.contains("没有键"), "got {said}");
    }

    #[test]
    fn a_keybinding_is_the_key_that_was_pressed() {
        let binding = Keybinding::parse("ctrl+shift+f9").expect("the key is read");
        let pressed = PressedKey {
            name: Some("f9".to_owned()),
            ctrl: true,
            shift: true,
            ..PressedKey::default()
        };
        assert!(binding.matches(&pressed));
    }

    #[test]
    fn a_keybinding_is_not_the_key_that_was_pressed_when_the_modifiers_differ() {
        let binding = Keybinding::parse("ctrl+shift+f9").expect("the key is read");
        let pressed = PressedKey {
            name: Some("f9".to_owned()),
            ctrl: true,
            ..PressedKey::default()
        };
        assert!(!binding.matches(&pressed), "the shift key was not held");
    }

    #[test]
    fn a_command_whose_id_is_already_there_is_refused() {
        let mut registry = CommandRegistry::new();
        registry
            .register(Command::new("editor.format", "格式化文档", "编辑"))
            .expect("the command is added");
        let said = registry
            .register(Command::new("editor.format", "格式化", "编辑"))
            .expect_err("the id is taken");
        assert!(said.contains("editor.format"), "got {said}");
    }

    #[test]
    fn the_commands_are_kept_in_the_order_of_their_ids() {
        let mut registry = CommandRegistry::new();
        registry
            .register(Command::new("terminal.open", "打开终端", "终端"))
            .expect("the command is added");
        registry
            .register(Command::new("editor.format", "格式化文档", "编辑"))
            .expect("the command is added");
        let ids: Vec<&str> = registry
            .all()
            .iter()
            .map(|command| command.id.as_str())
            .collect();
        assert_eq!(ids, vec!["editor.format", "terminal.open"]);
    }

    #[test]
    fn the_command_a_key_runs_is_found() {
        let mut registry = CommandRegistry::new();
        registry
            .register(
                Command::new("editor.format", "格式化文档", "编辑")
                    .with_key("alt+shift+f")
                    .expect("the key is read"),
            )
            .expect("the command is added");
        let pressed = PressedKey {
            letter: Some('F'),
            alt: true,
            shift: true,
            ..PressedKey::default()
        };
        assert_eq!(
            registry.by_key(&pressed).map(|command| command.id.as_str()),
            Some("editor.format")
        );
    }

    #[test]
    fn two_commands_that_want_one_key_are_said_out_loud() {
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
                Command::new("editor.rename", "重命名", "编辑")
                    .with_key("alt+shift+f")
                    .expect("the key is read"),
            )
            .expect("the command is added");

        let conflicts = registry.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].first, "editor.format");
        assert_eq!(conflicts[0].second, "editor.rename");
        assert_eq!(conflicts[0].keybinding.text(), "Alt+Shift+F");
    }

    #[test]
    fn a_command_that_needs_something_it_does_not_have_cannot_be_used() {        let mut registry = CommandRegistry::new();
        registry
            .register(
                Command::new("editor.rename", "重命名", "编辑").when(When::HasNameAtCaret),
            )
            .expect("the command is added");

        let empty = Context::default();
        assert!(!registry.is_enabled("editor.rename", &empty));

        let with_a_name = Context {
            has_name_at_caret: true,
            ..Context::default()
        };
        assert!(registry.is_enabled("editor.rename", &with_a_name));
    }

    #[test]
    fn the_commands_of_the_editor_do_not_want_the_same_key() {
        let conflicts = default_registry().conflicts();
        assert!(conflicts.is_empty(), "two commands want one key: {conflicts:?}");
    }

    #[test]
    fn every_command_of_the_editor_has_a_key() {
        let registry = default_registry();
        assert!(
            registry
                .all()
                .iter()
                .all(|command| command.keybinding.is_some()),
            "a command without a key cannot be pressed"
        );
    }
}
