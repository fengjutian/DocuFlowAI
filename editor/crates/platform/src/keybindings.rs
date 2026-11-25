//! The file where a person changes the keys.
//!
//! The keys the editor has are written down once, in the registry of the
//! commands. The registry is the truth; a file is only a set of changes to it.
//! The file is called `keybindings.json` and reads the way a person writes it:
//!
//! ```json
//! [
//!   { "key": "ctrl+shift+p", "command": "editor.format" },
//!   { "key": "alt+shift+m", "command": "editor.rename" }
//! ]
//! ```
//!
//! It is looked for in the folder of the project that is open and in the
//! folder of the user; the project wins when both name the same command.
//!
//! A change that cannot be made is **said to be wrong**, one line per change,
//! with the reason: a file that is silently half applied is a file the person
//! cannot trust.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::commands::{CommandRegistry, Keybinding};

/// One line of the file: this key runs that command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The key, written the way a person writes it: `ctrl+shift+p`.
    pub key: String,
    /// The id of the command that it runs: `editor.format`.
    pub command: String,
}

/// The name of the file.
pub const FILE_NAME: &str = "keybindings.json";

/// The changes a file asks for, and what was wrong with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// The changes, in the order they were written.
    pub bindings: Vec<Binding>,
    /// What could not be read, one line per mistake.
    pub errors: Vec<String>,
}

/// Reads a `keybindings.json` from its text.
///
/// A line that cannot be read does not take the rest of the file with it: the
/// lines that are well written are still applied, and the ones that are not
/// are named one by one.
pub fn parse(text: &str) -> Changes {
    let mut changes = Changes::default();
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            changes
                .errors
                .push(format!("键位文件读不出来：{error}"));
            return changes;
        }
    };
    let Some(entries) = value.as_array() else {
        changes
            .errors
            .push("键位文件要是一个列表：[{ \"key\": ..., \"command\": ... }]".to_owned());
        return changes;
    };
    for (index, entry) in entries.iter().enumerate() {
        let number = index + 1;
        let key = entry.get("key").and_then(Value::as_str);
        let command = entry.get("command").and_then(Value::as_str);
        match (key, command) {
            (Some(key), Some(command)) => changes.bindings.push(Binding {
                key: key.to_owned(),
                command: command.to_owned(),
            }),
            (None, _) => changes
                .errors
                .push(format!("第 {number} 条没有 key，写不成键位")),
            (_, None) => changes
                .errors
                .push(format!("第 {number} 条没有 command，不知道是哪个命令的键")),
        }
    }
    changes
}

/// The place the file of the user lives in.
///
/// Windows keeps the settings of a user under `%APPDATA%`; the other systems
/// under the folder of the user or the one the environment names. A machine
/// that says none of them has no file of the user, which is not an error.
pub fn user_path() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(base).join("CodeWeaver").join(FILE_NAME));
    }
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(base).join("CodeWeaver").join(FILE_NAME));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("CodeWeaver").join(FILE_NAME))
}

/// Reads one file, naming the file in what it says about a mistake.
///
/// A file that is not there is not a mistake: the editor starts without one.
pub fn read(path: &Path) -> Changes {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Changes::default(),
        Err(error) => {
            return Changes {
                bindings: Vec::new(),
                errors: vec![format!("{} 读不出来：{error}", path.display())],
            };
        }
    };
    let mut changes = parse(&text);
    if !changes.errors.is_empty() {
        let name = path.display().to_string();
        changes.errors = changes
            .errors
            .into_iter()
            .map(|error| format!("{name}：{error}"))
            .collect();
    }
    changes
}

/// The changes of the files that are in force here.
///
/// The file of the user is read first and the file of the project after it,
/// so that the project is the one that decides when both name one command.
pub fn load(project: Option<&Path>) -> Changes {
    load_from(project, user_path().as_deref())
}

/// The changes of the two files that are named.
///
/// This is the same as `load`, with the places given: a check does not want
/// to depend on the folder of the user of the machine it runs on.
pub fn load_from(project: Option<&Path>, user: Option<&Path>) -> Changes {
    let mut changes = Changes::default();
    if let Some(user) = user {
        let read = read(user);
        changes.bindings.extend(read.bindings);
        changes.errors.extend(read.errors);
    }
    if let Some(root) = project {
        let read = read(&root.join(FILE_NAME));
        changes.bindings.extend(read.bindings);
        changes.errors.extend(read.errors);
    }
    changes
}

/// Puts the changes into the registry.
///
/// Every change that cannot be made is named: a command that is not there, or
/// a key that cannot be read. The changes that can be made are made, whatever
/// the others did.
pub fn apply(registry: &mut CommandRegistry, bindings: &[Binding]) -> Vec<String> {
    let mut errors = Vec::new();
    for binding in bindings {
        match Keybinding::parse(&binding.key) {
            Ok(key) => {
                if let Err(said) = registry.rebind(&binding.command, key) {
                    errors.push(format!("键位文件：{said}"));
                }
            }
            Err(said) => errors.push(format!(
                "键位文件：命令 {} 的键位「{}」读不出来：{said}",
                binding.command, binding.key
            )),
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{Command, CommandRegistry};

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
                Command::new("editor.rename", "重命名", "编辑")
                    .with_key("f2")
                    .expect("the key is read"),
            )
            .expect("the command is added");
        registry
    }

    #[test]
    fn a_file_is_read_into_the_changes_it_asks_for() {
        let changes = parse(
            r#"[{ "key": "ctrl+shift+p", "command": "editor.format" },
                { "key": "alt+shift+m", "command": "editor.rename" }]"#,
        );
        assert!(changes.errors.is_empty(), "got {:?}", changes.errors);
        assert_eq!(changes.bindings.len(), 2);
        assert_eq!(changes.bindings[1].command, "editor.rename");
        assert_eq!(changes.bindings[1].key, "alt+shift+m");
    }

    #[test]
    fn a_line_without_a_key_or_a_command_is_named_by_its_number() {
        let changes = parse(r#"[{ "command": "editor.format" }, { "key": "f2" }]"#);
        assert_eq!(changes.bindings.len(), 0);
        assert_eq!(changes.errors.len(), 2);
        assert!(changes.errors[0].contains("第 1 条"), "got {:?}", changes.errors);
        assert!(changes.errors[1].contains("第 2 条"), "got {:?}", changes.errors);
    }

    #[test]
    fn a_file_that_is_not_a_list_says_what_it_should_be() {
        let changes = parse(r#"{ "key": "f2" }"#);
        assert!(changes.bindings.is_empty());
        assert!(changes.errors[0].contains("列表"), "got {:?}", changes.errors);
    }

    #[test]
    fn a_file_that_is_not_json_says_so_and_nothing_more() {
        let changes = parse("this is not json");
        assert!(changes.bindings.is_empty());
        assert!(changes.errors[0].contains("读不出来"), "got {:?}", changes.errors);
    }

    #[test]
    fn a_change_of_a_command_that_is_not_there_is_refused() {
        let mut registry = registry();
        let errors = apply(
            &mut registry,
            &[Binding {
                key: "ctrl+shift+p".to_owned(),
                command: "editor.formt".to_owned(),
            }],
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("editor.formt"), "got {errors:?}");
    }

    #[test]
    fn a_key_that_cannot_be_read_is_refused_and_the_others_are_made() {
        let mut registry = registry();
        let errors = apply(
            &mut registry,
            &[
                Binding {
                    key: "ctrl+strange+p".to_owned(),
                    command: "editor.format".to_owned(),
                },
                Binding {
                    key: "alt+shift+m".to_owned(),
                    command: "editor.rename".to_owned(),
                },
            ],
        );
        assert_eq!(errors.len(), 1, "got {errors:?}");
        assert!(errors[0].contains("strange"), "got {errors:?}");
        assert_eq!(
            registry
                .get("editor.rename")
                .and_then(|command| command.keybinding.as_ref())
                .map(|key| key.text()),
            Some("Alt+Shift+M".to_owned())
        );
    }

    #[test]
    fn the_key_a_command_has_after_a_change_is_the_new_one() {
        let mut registry = registry();
        apply(
            &mut registry,
            &[Binding {
                key: "ctrl+shift+p".to_owned(),
                command: "editor.format".to_owned(),
            }],
        );
        let pressed = crate::commands::PressedKey {
            letter: Some('p'),
            ctrl: true,
            shift: true,
            ..crate::commands::PressedKey::default()
        };
        assert_eq!(
            registry.by_key(&pressed).map(|command| command.id.as_str()),
            Some("editor.format")
        );
        let old = crate::commands::PressedKey {
            letter: Some('f'),
            alt: true,
            shift: true,
            ..crate::commands::PressedKey::default()
        };
        assert!(registry.by_key(&old).is_none(), "the old key is gone");
    }

    #[test]
    fn a_file_that_is_not_there_asks_for_nothing() {
        let missing = PathBuf::from("this-file-is-not-there").join(FILE_NAME);
        let changes = read(&missing);
        assert!(changes.bindings.is_empty());
        assert!(changes.errors.is_empty(), "got {:?}", changes.errors);
    }
}
