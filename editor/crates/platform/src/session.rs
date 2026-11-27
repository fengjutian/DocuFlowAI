//! The shape of the window, remembered from one run to the next.
//!
//! The editor used to start empty every time: the files a person had open,
//! the columns they were in and the one they were reading were gone with the
//! window. This module is the file where that shape is kept:
//!
//! ```json
//! {
//!   "workspace": "C:\\work\\project.code-workspace",
//!   "files": ["C:\\work\\project\\src\\main.rs", "C:\\work\\notes.md"],
//!   "columns": [
//!     { "tabs": [0], "front": 0 },
//!     { "tabs": [1], "front": 0 }
//!   ],
//!   "active": 1
//! }
//! ```
//!
//! The files are the paths on disk, in the order of the tab strip; a column
//! names its tabs by their place in that list. What the person was reading is
//! the tab in front of the active column.
//!
//! The module knows nothing about the window: it reads and writes a shape and
//! says what is wrong with it. Opening the files, and putting them in their
//! columns, is the window's job.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The name of the file the shape is remembered in.
pub const FILE_NAME: &str = "session.json";

/// One column of the window: the tabs it shows and the one in front.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Column {
    /// The tabs of the column, as places in `Session::files`.
    pub tabs: Vec<usize>,
    /// Which of them is in front, as a place in `tabs`.
    pub front: usize,
}

/// The shape of the window: what was open, and how it was laid out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// The workspace file that was open, when one was.
    pub workspace: Option<String>,
    /// The files that were open, in the order of the tab strip.
    pub files: Vec<String>,
    /// The columns of the window, from left to right.
    pub columns: Vec<Column>,
    /// Which column had the keyboard.
    pub active: usize,
}

impl Session {
    /// Whether there is nothing worth remembering.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.workspace.is_none()
    }
}

/// What the file says, and what was wrong with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    /// The shape the file asks for.
    pub session: Session,
    /// What could not be read, one line per mistake.
    pub errors: Vec<String>,
}

/// The place the file of the user lives in.
///
/// It sits next to the settings of the user: Windows keeps both under
/// `%APPDATA%`, the other systems under the folder the environment names. A
/// machine that names none of them has no session, which is not an error.
pub fn user_path() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(base).join("CodeWeaver").join(FILE_NAME));
    }
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(base).join("CodeWeaver").join(FILE_NAME));
    }
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("CodeWeaver")
            .join(FILE_NAME)
    })
}

/// Reads the shape from the text of a file.
///
/// A line that is wrong does not take the rest of the file with it: the parts
/// that can be read are read, and what was wrong is named one line at a time.
pub fn parse(text: &str) -> Loaded {
    let mut errors = Vec::new();
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            return Loaded {
                session: Session::default(),
                errors: vec![format!("会话文件读不出来：{error}")],
            };
        }
    };
    let Some(object) = value.as_object() else {
        return Loaded {
            session: Session::default(),
            errors: vec!["会话文件要是一个对象：{ \"files\": [...], ... }".to_owned()],
        };
    };

    let workspace = match object.get("workspace") {
        None | Some(Value::Null) => None,
        Some(Value::String(path)) if !path.is_empty() => Some(path.clone()),
        Some(Value::String(_)) => None,
        Some(_) => {
            errors.push("workspace 不是一个路径字符串，没有用上".to_owned());
            None
        }
    };

    let mut files: Vec<String> = Vec::new();
    match object.get("files") {
        None => {}
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                match item.as_str() {
                    Some(path) if !path.is_empty() => {
                        if files.iter().any(|kept| kept == path) {
                            errors.push(format!(
                                "第 {} 个文件 {path} 写了两次，只记一次",
                                index + 1
                            ));
                        } else {
                            files.push(path.to_owned());
                        }
                    }
                    _ => errors.push(format!("第 {} 个文件不是一个路径", index + 1)),
                }
            }
        }
        Some(_) => errors.push("files 不是一个列表".to_owned()),
    }

    let mut columns: Vec<Column> = Vec::new();
    match object.get("columns") {
        None => {}
        Some(Value::Array(items)) => {
            for (number, item) in items.iter().enumerate() {
                let Some(entry) = item.as_object() else {
                    errors.push(format!("第 {} 栏不是一个对象", number + 1));
                    continue;
                };
                let mut tabs: Vec<usize> = Vec::new();
                match entry.get("tabs") {
                    Some(Value::Array(places)) => {
                        for (place, value) in places.iter().enumerate() {
                            let Some(index) = value.as_u64().map(|index| index as usize) else {
                                errors.push(format!(
                                    "第 {} 栏里的第 {} 个标签不是一个数字",
                                    number + 1,
                                    place + 1
                                ));
                                continue;
                            };
                            if index >= files.len() {
                                errors.push(format!(
                                    "第 {} 栏里的第 {} 个标签指到了一个不存在的文件",
                                    number + 1,
                                    place + 1
                                ));
                                continue;
                            }
                            if tabs.contains(&index)
                                || columns.iter().any(|column| column.tabs.contains(&index))
                            {
                                errors.push(format!(
                                    "第 {} 栏里的第 {} 个标签是重复的",
                                    number + 1,
                                    place + 1
                                ));
                                continue;
                            }
                            tabs.push(index);
                        }
                    }
                    _ => errors.push(format!("第 {} 栏没有 tabs 列表", number + 1)),
                }
                if tabs.is_empty() {
                    // A column that shows nothing is not a column.
                    continue;
                }
                let front = match entry.get("front") {
                    None => 0,
                    Some(value) => match value.as_u64() {
                        Some(front) if (front as usize) < tabs.len() => front as usize,
                        _ => {
                            errors.push(format!(
                                "第 {} 栏的 front 指到了一个不存在的标签，改成第一个",
                                number + 1
                            ));
                            0
                        }
                    },
                };
                columns.push(Column { tabs, front });
            }
        }
        Some(_) => errors.push("columns 不是一个列表".to_owned()),
    }

    // A file that is open is in some column: a file written without columns
    // is one column with everything in it, not a window with nothing on
    // screen.
    if columns.is_empty() && !files.is_empty() {
        columns.push(Column {
            tabs: (0..files.len()).collect(),
            front: 0,
        });
    }

    let active = match object.get("active") {
        None => 0,
        Some(value) => match value.as_u64() {
            Some(index) => index as usize,
            None => {
                errors.push("active 不是一个数字，改成第一栏".to_owned());
                0
            }
        },
    };
    let active = if columns.is_empty() {
        0
    } else if active >= columns.len() {
        errors.push(format!(
            "active 指到了不存在的第 {} 栏，改成第一栏",
            active + 1
        ));
        0
    } else {
        active
    };

    Loaded {
        session: Session {
            workspace,
            files,
            columns,
            active,
        },
        errors,
    }
}

/// Reads the shape from a file.
///
/// A file that is not there is not a mistake: it is the first run, and the
/// editor starts the way it always did.
pub fn read(path: &Path) -> Loaded {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Loaded::default();
        }
        Err(error) => {
            return Loaded {
                session: Session::default(),
                errors: vec![format!("{} 读不出来：{error}", path.display())],
            };
        }
    };
    let mut loaded = parse(&text);
    if !loaded.errors.is_empty() {
        let name = path.display().to_string();
        loaded.errors = loaded
            .errors
            .into_iter()
            .map(|error| format!("{name}：{error}"))
            .collect();
    }
    loaded
}

/// Writes the shape into a file.
pub fn write(path: &Path, session: &Session) -> Result<(), String> {
    let columns: Vec<Value> = session
        .columns
        .iter()
        .map(|column| {
            json!({
                "tabs": column.tabs,
                "front": column.front,
            })
        })
        .collect();
    let value = json!({
        "workspace": session.workspace,
        "files": session.files,
        "columns": columns,
        "active": session.active,
    });
    let mut text = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("会话文件写不出来：{error}"))?;
    text.push('\n');
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)
            .map_err(|error| format!("{} 建不出来：{error}", folder.display()))?;
    }
    fs::write(path, text).map_err(|error| format!("{} 写不进去：{error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch_dir;

    #[test]
    fn the_shape_of_the_window_is_read_from_the_file() {
        let loaded = parse(
            r#"{
                "workspace": "C:\\work\\project.code-workspace",
                "files": ["C:\\work\\a.rs", "C:\\work\\b.md"],
                "columns": [
                    { "tabs": [0], "front": 0 },
                    { "tabs": [1], "front": 0 }
                ],
                "active": 1
            }"#,
        );
        assert!(loaded.errors.is_empty(), "got {:?}", loaded.errors);
        assert_eq!(
            loaded.session.workspace.as_deref(),
            Some("C:\\work\\project.code-workspace")
        );
        assert_eq!(loaded.session.files.len(), 2);
        assert_eq!(loaded.session.columns.len(), 2);
        assert_eq!(loaded.session.columns[1].tabs, vec![1]);
        assert_eq!(loaded.session.active, 1);
    }

    #[test]
    fn a_file_that_is_open_without_columns_is_one_column() {
        let loaded = parse(r#"{ "files": ["a.rs", "b.md"] }"#);
        assert!(loaded.errors.is_empty(), "got {:?}", loaded.errors);
        assert_eq!(loaded.session.columns.len(), 1);
        assert_eq!(loaded.session.columns[0].tabs, vec![0, 1]);
        assert_eq!(loaded.session.columns[0].front, 0);
    }

    #[test]
    fn a_file_that_is_not_json_says_so() {
        let loaded = parse("this is not json");
        assert!(loaded.session.is_empty());
        assert!(loaded.errors[0].contains("读不出来"), "got {:?}", loaded.errors);
    }

    #[test]
    fn a_file_that_is_not_an_object_says_what_it_should_be() {
        let loaded = parse(r#"["a.rs"]"#);
        assert!(loaded.errors[0].contains("对象"), "got {:?}", loaded.errors);
    }

    #[test]
    fn files_that_are_not_a_list_are_named() {
        let loaded = parse(r#"{ "files": "a.rs" }"#);
        assert!(loaded.session.files.is_empty());
        assert!(
            loaded.errors[0].contains("files 不是一个列表"),
            "got {:?}",
            loaded.errors
        );
    }

    #[test]
    fn a_column_that_points_past_the_files_is_named() {
        let loaded = parse(
            r#"{ "files": ["a.rs"], "columns": [{ "tabs": [0, 3], "front": 0 }] }"#,
        );
        assert_eq!(loaded.session.columns[0].tabs, vec![0]);
        assert!(
            loaded.errors.iter().any(|error| error.contains("不存在的文件")),
            "got {:?}",
            loaded.errors
        );
    }

    #[test]
    fn a_front_that_points_past_its_column_is_made_the_first() {
        let loaded = parse(
            r#"{ "files": ["a.rs", "b.rs"], "columns": [{ "tabs": [0, 1], "front": 9 }] }"#,
        );
        assert_eq!(loaded.session.columns[0].front, 0);
        assert!(
            loaded.errors.iter().any(|error| error.contains("front")),
            "got {:?}",
            loaded.errors
        );
    }

    #[test]
    fn a_column_that_shows_nothing_is_not_a_column() {
        let loaded = parse(
            r#"{ "files": ["a.rs"], "columns": [{ "tabs": [], "front": 0 }, { "tabs": [0], "front": 0 }] }"#,
        );
        assert_eq!(loaded.session.columns.len(), 1);
        assert_eq!(loaded.session.columns[0].tabs, vec![0]);
    }

    #[test]
    fn the_active_column_stays_inside_the_columns_that_are_there() {
        let loaded = parse(
            r#"{ "files": ["a.rs"], "columns": [{ "tabs": [0] }], "active": 4 }"#,
        );
        assert_eq!(loaded.session.active, 0);
        assert!(
            loaded.errors.iter().any(|error| error.contains("active")),
            "got {:?}",
            loaded.errors
        );
    }

    #[test]
    fn a_file_that_is_written_twice_is_named() {
        let loaded = parse(r#"{ "files": ["a.rs", "a.rs"] }"#);
        assert_eq!(loaded.session.files, vec!["a.rs".to_owned()]);
        assert!(
            loaded.errors.iter().any(|error| error.contains("两次")),
            "got {:?}",
            loaded.errors
        );
    }

    #[test]
    fn a_session_that_was_written_can_be_read_again() {
        let folder = scratch_dir("session-roundtrip");
        let path = folder.join(FILE_NAME);
        let session = Session {
            workspace: Some("C:\\work\\project.code-workspace".to_owned()),
            files: vec!["C:\\work\\a.rs".to_owned(), "C:\\work\\b.md".to_owned()],
            columns: vec![
                Column {
                    tabs: vec![0],
                    front: 0,
                },
                Column {
                    tabs: vec![1],
                    front: 0,
                },
            ],
            active: 1,
        };
        write(&path, &session).expect("the session is written");
        let loaded = read(&path);
        assert!(loaded.errors.is_empty(), "got {:?}", loaded.errors);
        assert_eq!(loaded.session, session);
    }

    #[test]
    fn a_file_that_is_not_there_reads_as_an_empty_session() {
        let path = scratch_dir("session-missing").join(FILE_NAME);
        let loaded = read(&path);
        assert!(loaded.session.is_empty());
        assert!(loaded.errors.is_empty(), "got {:?}", loaded.errors);
    }

    #[test]
    fn an_empty_session_is_known_to_be_empty() {
        assert!(Session::default().is_empty());
        let session = Session {
            files: vec!["a.rs".to_owned()],
            ..Session::default()
        };
        assert!(!session.is_empty());
    }
}
