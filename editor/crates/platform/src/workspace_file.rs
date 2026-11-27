//! The workspace file: the file that names the folders of a project.
//!
//! A project that has more than one folder is written down in a file of its
//! own, the way VS Code writes it down:
//!
//! ```json
//! {
//!   "folders": [
//!     { "path": "../editor" },
//!     { "path": "../console", "name": "控制台" }
//!   ],
//!   "settings": { "editor.fontSize": 16 }
//! }
//! ```
//!
//! The module reads that file and writes it again. It does not know what the
//! folders are for: turning the folders into the tree of files is the job of
//! the tree, and using the settings is the job of the settings.
//!
//! A path in the file is **relative to the folder of the file itself**, unless
//! it is absolute. That is what makes a workspace file portable: the whole
//! folder can be moved somewhere else and the file still means the same thing.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};

/// The suffix of the file that holds a workspace.
pub const SUFFIX: &str = ".code-workspace";

/// A folder that a workspace names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// The path of the folder, the way the file writes it.
    pub path: String,
    /// The name the file gives the folder, when it gives one.
    pub name: Option<String>,
}

/// The contents of a workspace file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    /// The folders the workspace names, in the order the file names them.
    pub folders: Vec<Folder>,
    /// The settings inside the file.
    pub settings: BTreeMap<String, String>,
}

/// A folder of a workspace, with the path it has on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// The name the folder is shown with.
    pub name: String,
    /// Where the folder is on disk.
    pub path: PathBuf,
    /// Whether the folder is there. A folder that is not there is still a root
    /// of the workspace: the person has to be able to see that it is missing.
    pub there: bool,
}

/// A workspace that is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// The file the workspace was read from.
    pub path: PathBuf,
    /// The name the workspace is shown with.
    pub name: String,
    /// The folders the workspace names, in the order the file names them.
    pub roots: Vec<Root>,
    /// The settings inside the file. They are read, but nothing uses them yet.
    pub settings: BTreeMap<String, String>,
}

impl Workspace {
    /// Reads the workspace file and works out the folders it names.
    ///
    /// The paths of the folders are relative to the folder of the file itself,
    /// unless they are absolute.
    pub fn open(path: &Path) -> Result<Workspace, String> {
        let contents = read(path)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        Ok(Workspace {
            path: path.to_path_buf(),
            name: name_of(path),
            roots: contents.roots(base),
            settings: contents.settings,
        })
    }

    /// The folders of the workspace that are not on disk.
    pub fn missing(&self) -> Vec<&Root> {
        self.roots.iter().filter(|root| !root.there).collect()
    }

    /// The contents that would be written back into the file.
    ///
    /// The paths are written the way they can be read again: relative to the
    /// folder of the file, with a forward slash between the parts.
    pub fn contents(&self) -> Contents {
        let base = self.path.parent().unwrap_or_else(|| Path::new("."));
        let folders = self
            .roots
            .iter()
            .map(|root| {
                let own = root
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                let name = match own {
                    Some(own) if own != root.name => Some(root.name.clone()),
                    _ => None,
                };
                Folder {
                    path: relative_to(base, &root.path),
                    name,
                }
            })
            .collect();
        Contents {
            folders,
            settings: self.settings.clone(),
        }
    }

    /// Adds a folder to the workspace.
    ///
    /// The path is taken from the folder of the workspace file itself, unless
    /// it is absolute: a path that is written down in the file means the same
    /// thing wherever the file is.
    ///
    /// A folder that is not on disk is **not added**: a root that is not there
    /// is a row in the tree that is always empty, and the person has to be
    /// told that the folder is missing rather than shown an empty row.
    ///
    /// A folder that is already in the workspace is not added a second time.
    pub fn add_root(&mut self, path: &str) -> Result<String, String> {
        let base = self.path.parent().unwrap_or_else(|| Path::new("."));
        let path = resolve(base, path.trim());
        if !path.is_dir() {
            return Err(format!("{} 不在了", path.display()));
        }
        if self.roots.iter().any(|root| root.path == path) {
            return Err(format!(
                "{} 已经在工作区里了",
                root_name(&path)
            ));
        }
        let name = root_name(&path);
        self.roots.push(Root {
            name: name.clone(),
            path,
            there: true,
        });
        Ok(name)
    }

    /// Takes a folder out of the workspace.
    ///
    /// The folder itself is not touched: only the workspace stops naming it.
    pub fn remove_root(&mut self, path: &Path) -> Result<String, String> {
        let Some(index) = self.roots.iter().position(|root| root.path == path) else {
            return Err("这个文件夹不在工作区里".to_owned());
        };
        Ok(self.roots.remove(index).name)
    }
}

/// The name a folder is shown with: the name of the folder itself.
fn root_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl Contents {
    /// The folders of the contents, with the paths they have on disk.
    ///
    /// A relative path is taken from the folder of the workspace file itself.
    pub fn roots(&self, base: &Path) -> Vec<Root> {
        self.folders
            .iter()
            .map(|folder| {
                let path = resolve(base, &folder.path);
                let name = match &folder.name {
                    Some(name) => name.clone(),
                    None => path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| folder.path.clone()),
                };
                Root {
                    name,
                    there: path.is_dir(),
                    path,
                }
            })
            .collect()
    }
}

/// Whether the path names a workspace file.
pub fn is_workspace_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_lowercase().ends_with(SUFFIX))
        .unwrap_or(false)
}

/// The name a workspace file is shown with: the name of the file without the
/// suffix.
pub fn name_of(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.strip_suffix(SUFFIX) {
        Some(short) if !short.is_empty() => short.to_owned(),
        _ => name,
    }
}

/// Reads the workspace file.
///
/// A file that cannot be read or cannot be made sense of is **named**: the
/// person has to know which part of the file is wrong, not only that
/// something went wrong.
pub fn read(path: &Path) -> Result<Contents, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("{} 读不出来：{error}", path.display()))?;
    parse(&text).map_err(|error| format!("{}：{error}", path.display()))
}

/// Writes the workspace file.
pub fn write(path: &Path, contents: &Contents) -> Result<(), String> {
    let mut folders = Vec::new();
    for folder in &contents.folders {
        let mut entry = Map::new();
        entry.insert("path".to_owned(), Value::String(folder.path.clone()));
        if let Some(name) = &folder.name {
            entry.insert("name".to_owned(), Value::String(name.clone()));
        }
        folders.push(Value::Object(entry));
    }
    let mut settings = Map::new();
    for (key, value) in &contents.settings {
        settings.insert(key.clone(), Value::String(value.clone()));
    }
    let mut file = Map::new();
    file.insert("folders".to_owned(), Value::Array(folders));
    file.insert("settings".to_owned(), Value::Object(settings));
    let text = serde_json::to_string_pretty(&Value::Object(file))
        .map_err(|error| format!("写不出工作区文件：{error}"))?;
    if let Some(folder) = path.parent()
        && !folder.as_os_str().is_empty()
    {
        fs::create_dir_all(folder)
            .map_err(|error| format!("{} 建不出来：{error}", folder.display()))?;
    }
    fs::write(path, format!("{text}\n"))
        .map_err(|error| format!("{} 写不进去：{error}", path.display()))
}

/// Reads the contents of a workspace file out of its text.
fn parse(text: &str) -> Result<Contents, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("这个文件不是合法的 JSON：{error}"))?;
    let Some(file) = value.as_object() else {
        return Err("最外层必须是一个对象 {…}".to_owned());
    };

    let mut folders = Vec::new();
    match file.get("folders") {
        None | Some(Value::Null) => {}
        Some(Value::Array(entries)) => {
            for (index, entry) in entries.iter().enumerate() {
                let Some(entry) = entry.as_object() else {
                    return Err(format!("folders 里的第 {} 项不是一个对象", index + 1));
                };
                let Some(path) = entry.get("path").and_then(Value::as_str) else {
                    return Err(format!("folders 里的第 {} 项没有 path", index + 1));
                };
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                folders.push(Folder {
                    path: path.to_owned(),
                    name,
                });
            }
        }
        Some(_) => return Err("folders 必须是一个数组".to_owned()),
    }

    let mut settings = BTreeMap::new();
    match file.get("settings") {
        None | Some(Value::Null) => {}
        Some(Value::Object(entries)) => {
            for (key, value) in entries {
                let text = match value {
                    Value::String(text) => text.clone(),
                    Value::Null => continue,
                    other => other.to_string(),
                };
                settings.insert(key.clone(), text);
            }
        }
        Some(_) => return Err("settings 必须是一个对象".to_owned()),
    }

    Ok(Contents { folders, settings })
}

/// The path of a folder of a workspace: relative to the base unless it is
/// absolute.
fn resolve(base: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// The path of a folder the way it can be written into the file again.
fn relative_to(base: &Path, path: &Path) -> String {
    let text = match path.strip_prefix(base) {
        Ok(rest) => rest.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    };
    text.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::scratch_dir;

    /// Writes a file in a scratch directory and gives its path.
    fn write_file(directory: &Path, name: &str, text: &str) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, text).expect("the file can be written");
        path
    }

    #[test]
    fn a_workspace_file_is_recognised_by_its_suffix() {
        assert!(is_workspace_file(Path::new("project.code-workspace")));
        assert!(is_workspace_file(Path::new("C:/a/project.code-workspace")));
        assert!(!is_workspace_file(Path::new("project.json")));
        assert!(!is_workspace_file(Path::new("project")));
    }

    #[test]
    fn the_name_of_a_workspace_is_the_name_of_its_file() {
        assert_eq!(name_of(Path::new("a/project.code-workspace")), "project");
        assert_eq!(
            name_of(Path::new("C:/a/project.code-workspace")),
            "project"
        );
    }

    #[test]
    fn the_folders_of_the_file_are_read() {
        let contents = parse(
            r#"{
                "folders": [
                    { "path": "../editor" },
                    { "path": "../console", "name": "控制台" }
                ]
            }"#,
        )
        .expect("the file is read");
        assert_eq!(contents.folders.len(), 2);
        assert_eq!(contents.folders[0].path, "../editor");
        assert_eq!(contents.folders[0].name, None);
        assert_eq!(contents.folders[1].path, "../console");
        assert_eq!(contents.folders[1].name.as_deref(), Some("控制台"));
    }

    #[test]
    fn a_file_without_folders_is_an_empty_workspace_and_not_a_mistake() {
        let contents = parse("{}").expect("the file is read");
        assert!(contents.folders.is_empty());
        let contents = parse(r#"{ "folders": [] }"#).expect("the file is read");
        assert!(contents.folders.is_empty());
    }

    #[test]
    fn the_settings_of_the_file_are_read() {
        let contents = parse(
            r#"{ "settings": { "editor.fontSize": 16, "editor.tabSize": "4" } }"#,
        )
        .expect("the file is read");
        assert_eq!(contents.settings.get("editor.fontSize").map(String::as_str), Some("16"));
        assert_eq!(contents.settings.get("editor.tabSize").map(String::as_str), Some("4"));
    }

    #[test]
    fn a_file_that_is_not_json_says_so() {
        let said = parse("{ this is not json").expect_err("the file is not read");
        assert!(said.contains("JSON"), "got {said}");
    }

    #[test]
    fn a_file_whose_folders_are_not_an_array_says_so() {
        let said = parse(r#"{ "folders": "editor" }"#).expect_err("the file is not read");
        assert!(said.contains("folders"), "got {said}");
        assert!(said.contains("数组"), "got {said}");
    }

    #[test]
    fn a_folder_without_a_path_says_which_one_it_is() {
        let said = parse(r#"{ "folders": [ { "path": "a" }, { "name": "b" } ] }"#)
            .expect_err("the file is not read");
        assert!(said.contains("第 2 项"), "got {said}");
        assert!(said.contains("path"), "got {said}");
    }

    #[test]
    fn a_relative_path_is_taken_from_the_folder_of_the_file() {
        let base = Path::new("C:/work");
        let contents = parse(r#"{ "folders": [ { "path": "../editor" } ] }"#)
            .expect("the file is read");
        let roots = contents.roots(base);
        assert_eq!(roots[0].path, PathBuf::from("C:/work").join("../editor"));
    }

    #[test]
    fn an_absolute_path_stays_where_it_is() {
        let contents = parse(r#"{ "folders": [ { "path": "D:/other" } ] }"#)
            .expect("the file is read");
        let roots = contents.roots(Path::new("C:/work"));
        assert_eq!(roots[0].path, PathBuf::from("D:/other"));
    }

    #[test]
    fn the_name_of_a_folder_is_the_name_the_file_gives_it() {
        let contents = parse(
            r#"{ "folders": [ { "path": "a" }, { "path": "b", "name": "乙" } ] }"#,
        )
        .expect("the file is read");
        let roots = contents.roots(Path::new("C:/work"));
        assert_eq!(roots[0].name, "a");
        assert_eq!(roots[1].name, "乙");
    }

    #[test]
    fn a_folder_that_is_not_there_is_said_to_be_missing() {
        let directory = scratch_dir("workspace-missing");
        let file = write_file(
            &directory,
            "project.code-workspace",
            r#"{ "folders": [ { "path": "nowhere" } ] }"#,
        );
        let workspace = Workspace::open(&file).expect("the workspace is opened");
        assert_eq!(workspace.missing().len(), 1);
        assert_eq!(workspace.missing()[0].name, "nowhere");
    }

    #[test]
    fn a_workspace_is_written_and_read_again_the_same_way() {
        let directory = scratch_dir("workspace-round-trip");
        let file = directory.join("project.code-workspace");
        let contents = Contents {
            folders: vec![
                Folder {
                    path: "editor".to_owned(),
                    name: None,
                },
                Folder {
                    path: "console".to_owned(),
                    name: Some("控制台".to_owned()),
                },
            ],
            settings: BTreeMap::new(),
        };
        write(&file, &contents).expect("the file is written");

        let read_back = read(&file).expect("the file is read");
        assert_eq!(read_back, contents);
    }

    #[test]
    fn the_file_that_is_written_can_be_opened_as_a_workspace() {
        let directory = scratch_dir("workspace-open");
        let folder = directory.join("editor");
        fs::create_dir_all(&folder).expect("the folder can be created");
        let file = write_file(
            &directory,
            "project.code-workspace",
            r#"{ "folders": [ { "path": "editor" } ] }"#,
        );
        let workspace = Workspace::open(&file).expect("the workspace is opened");
        assert_eq!(workspace.name, "project");
        assert_eq!(workspace.roots.len(), 1);
        assert_eq!(workspace.roots[0].path, folder);
        assert!(workspace.roots[0].there, "the folder is there");
        assert!(workspace.missing().is_empty());
    }

    #[test]
    fn a_workspace_that_is_written_again_keeps_its_folders() {
        let directory = scratch_dir("workspace-contents");
        let folder = directory.join("editor");
        fs::create_dir_all(&folder).expect("the folder can be created");
        let file = directory.join("project.code-workspace");
        write(
            &file,
            &Contents {
                folders: vec![Folder {
                    path: "editor".to_owned(),
                    name: None,
                }],
                settings: BTreeMap::new(),
            },
        )
        .expect("the file is written");

        let workspace = Workspace::open(&file).expect("the workspace is opened");
        let again = workspace.contents();
        assert_eq!(again.folders.len(), 1);
        assert_eq!(again.folders[0].path, "editor");
        assert_eq!(again.folders[0].name, None);
    }

    #[test]
    fn a_workspace_that_cannot_be_read_says_where_it_is() {
        let directory = scratch_dir("workspace-unreadable");
        let file = directory.join("project.code-workspace");
        let said = Workspace::open(&file).expect_err("the workspace is not opened");
        assert!(said.contains("project.code-workspace"), "got {said}");
    }

    #[test]
    fn a_folder_can_be_added_to_the_workspace() {
        let directory = scratch_dir("workspace-add-root");
        let folder = directory.join("editor");
        fs::create_dir_all(&folder).expect("the folder can be created");
        let file = write_file(
            &directory,
            "project.code-workspace",
            r#"{ "folders": [ { "path": "nowhere" } ] }"#,
        );
        let mut workspace = Workspace::open(&file).expect("the workspace is opened");

        // The path is taken from the folder of the workspace file itself.
        let name = workspace.add_root("editor").expect("the folder is added");
        assert_eq!(name, "editor");
        assert_eq!(workspace.roots.len(), 2);
        assert_eq!(workspace.roots[1].path, folder);
        assert!(workspace.roots[1].there, "the folder is there");

        // The file is written again with the folder in it.
        let contents = workspace.contents();
        assert_eq!(contents.folders.len(), 2);
        assert_eq!(contents.folders[1].path, "editor");
    }

    #[test]
    fn a_folder_that_is_not_there_is_not_added() {
        let directory = scratch_dir("workspace-add-missing");
        let file = write_file(
            &directory,
            "project.code-workspace",
            r#"{ "folders": [] }"#,
        );
        let mut workspace = Workspace::open(&file).expect("the workspace is opened");

        let said = workspace.add_root("nowhere").expect_err("the folder is not there");
        assert!(said.contains("不在了"), "got {said}");
        assert!(workspace.roots.is_empty(), "nothing was added");
    }

    #[test]
    fn a_folder_that_is_already_there_is_not_added_again() {
        let directory = scratch_dir("workspace-add-twice");
        let folder = directory.join("editor");
        fs::create_dir_all(&folder).expect("the folder can be created");
        let file = write_file(
            &directory,
            "project.code-workspace",
            r#"{ "folders": [ { "path": "editor" } ] }"#,
        );
        let mut workspace = Workspace::open(&file).expect("the workspace is opened");

        let said = workspace.add_root("editor").expect_err("it is already there");
        assert!(said.contains("已经"), "got {said}");
        assert_eq!(workspace.roots.len(), 1);
    }

    #[test]
    fn a_folder_can_be_taken_out_of_the_workspace() {
        let directory = scratch_dir("workspace-remove-root");
        let folder = directory.join("editor");
        fs::create_dir_all(&folder).expect("the folder can be created");
        let file = write_file(
            &directory,
            "project.code-workspace",
            r#"{ "folders": [ { "path": "editor" }, { "path": "console" } ] }"#,
        );
        let mut workspace = Workspace::open(&file).expect("the workspace is opened");

        let name = workspace.remove_root(&folder).expect("the folder is taken out");
        assert_eq!(name, "editor");
        assert_eq!(workspace.roots.len(), 1);
        assert_eq!(workspace.contents().folders.len(), 1);

        // A folder that is not in the workspace cannot be taken out.
        let said = workspace
            .remove_root(&folder)
            .expect_err("it is not in the workspace");
        assert!(said.contains("不在"), "got {said}");
    }
}
