//! What git knows about the files of the project.
//!
//! The editor asks the `git` of the machine: a library would be more
//! controllable, but it would have to deal with the credentials of the
//! machine itself, and the command line does that already.
//!
//! The root of the repository is the folder that holds the `.git` directory:
//! the file that is open decides which repository is meant.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// One file that git knows has changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitEntry {
    /// The path of the file, relative to the root of the repository.
    pub path: String,
    /// What happened to the file: `M` (changed), `A` (added), `??` (not
    /// under version control), `D` (removed)...
    pub state: String,
    /// Whether the change is in the staging area.
    pub staged: bool,
}

impl GitEntry {
    /// What happened to the file, in words.
    ///
    /// The letters of git are for git: the panel says what happened in the
    /// language of the person who reads it.
    pub fn label(&self) -> String {
        let word = match self.state.as_str() {
            "??" => "未纳入",
            "A" => "新增",
            "D" => "删除",
            "R" => "改名",
            "C" => "复制",
            "U" => "冲突",
            "M" => "改了",
            other => other,
        };
        if self.staged {
            format!("已暂存·{word}")
        } else {
            word.to_owned()
        }
    }
}

/// The files of the project that git knows have changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// The files, in the order git gave them.
    pub entries: Vec<GitEntry>,
}

/// The root of the repository the given path belongs to.
///
/// The root is the folder that holds the `.git` directory. A file that
/// belongs to no repository has no root.
pub fn repository_root(path: &Path) -> Option<PathBuf> {
    let mut directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
    loop {
        if directory.join(".git").exists() {
            return Some(directory);
        }
        if !directory.pop() {
            return None;
        }
    }
}

/// Reads what git knows about the files of the repository.
pub fn status(root: &Path) -> Result<GitStatus, String> {
    let output = run(root, &["status", "--porcelain"])?;
    let mut entries = Vec::new();
    for line in output.lines() {
        // Every line is two letters of state and the path of the file.
        if line.len() < 4 {
            continue;
        }
        let mut characters = line.chars();
        let staged_state = characters.next().unwrap_or(' ');
        let worktree_state = characters.next().unwrap_or(' ');
        let path = line[3..].trim();
        // A file that was renamed is given as "old -> new".
        let path = match path.split_once(" -> ") {
            Some((_, new)) => new.trim(),
            None => path,
        };
        let state = if staged_state == '?' {
            "??".to_owned()
        } else if staged_state == ' ' {
            worktree_state.to_string()
        } else {
            staged_state.to_string()
        };
        entries.push(GitEntry {
            path: path.to_owned(),
            state,
            staged: staged_state != ' ' && staged_state != '?',
        });
    }
    Ok(GitStatus { entries })
}

/// The diff of one file.
///
/// The diff of the staging area is asked for when the file is staged: what
/// is staged and what is not are two different diffs.
pub fn diff(root: &Path, path: &str, staged: bool) -> Result<String, String> {
    let mut args = vec!["diff".to_owned()];
    if staged {
        args.push("--cached".to_owned());
    }
    args.push("--".to_owned());
    args.push(path.to_owned());
    run(root, &args)
}

/// Puts the changes of one file into the staging area.
pub fn stage(root: &Path, path: &str) -> Result<(), String> {
    run(
        root,
        &["add".to_owned(), "--".to_owned(), path.to_owned()],
    )
    .map(|_| ())
}

/// Commits what is in the staging area.
///
/// What git says about the commit is returned: the number of files that
/// changed, or why there was nothing to commit.
pub fn commit(root: &Path, message: &str) -> Result<String, String> {
    run(
        root,
        &["commit".to_owned(), "-m".to_owned(), message.to_owned()],
    )
}

/// Runs a git command in the repository and returns what it said.
pub(crate) fn run(root: &Path, args: &[impl AsRef<str>]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args.iter().map(|argument| argument.as_ref()))
        .current_dir(root)
        .output()
        .map_err(|error| format!("git 起不来：{error}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if said.is_empty() {
            format!("git 失败了（退出码 {:?}）", output.status.code())
        } else {
            said
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Whether the git of the machine can be started from this process.
///
/// The binary of the tests is built inside the workspace, and the workspace
/// carries a low integrity label: a low integrity process cannot start the
/// `git` of the machine, which runs at the ordinary level. The checks that
/// need git are then not run at all, and it has to be said out loud: a check
/// that did not run is not a check that passed.
///
/// The checks are run for real from a copy of the test binary outside the
/// workspace, the way the window checks run the editor.
#[cfg(test)]
pub(crate) fn git_is_blocked() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};

    static SAID: AtomicBool = AtomicBool::new(false);
    match run(&PathBuf::from("."), &["--version"]) {
        Ok(_) => false,
        Err(error) => {
            if !SAID.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "BLOCKED: git cannot be started from here ({error}); the checks of git \
                     are not run at all. Run them from a copy of the test binary outside the \
                     workspace: tools\\git-unit-check.ps1"
                );
            }
            true
        }
    }
}

/// What the panel of the changes shows.
///
/// The panel is a picture of what git knows: the files that changed, and the
/// diff of the one that is picked.
pub struct GitPanel {
    /// The root of the repository.
    pub root: PathBuf,
    /// The files that changed.
    pub entries: Vec<GitEntry>,
    /// Which file is picked, counted from the first one.
    pub selected: usize,
    /// The diff of the picked file.
    pub diff: String,
}

impl GitPanel {
    /// Makes a panel of the files that changed.
    pub fn new(root: PathBuf, entries: Vec<GitEntry>) -> Self {
        GitPanel {
            root,
            entries,
            selected: 0,
            diff: String::new(),
        }
    }

    /// The file that is picked.
    pub fn selected_entry(&self) -> Option<&GitEntry> {
        self.entries.get(self.selected)
    }

    /// Picks the file after this one, going round to the first one.
    pub fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.entries.len();
    }

    /// Picks the file before this one, going round to the last one.
    pub fn select_previous(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.entries.len() - 1
        } else {
            self.selected - 1
        };
    }
}

/// The height of one row of the list of files.
const ROW_HEIGHT: f32 = 22.0;
/// The width of the list of files.
const LIST_WIDTH: f32 = 360.0;
/// The width of the column that says what happened to the file.
const LABEL_WIDTH: f32 = 92.0;

/// The ground the panel of the changes stands on.
///
/// The ground is drawn on its own when something is drawn over the panel:
/// the text of the panel would lie on top of it.
pub fn draw_background(
    scene: &mut codeweaver_render::FrameScene,
    viewport: codeweaver_render::Rect,
) {
    use codeweaver_render::Color;

    scene.push_quad(viewport, Color::rgb(0.055, 0.063, 0.082), viewport);
}

/// Draws the panel of the changes.
///
/// The panel covers the window: it is a thing the user works in for a
/// moment, and the text of the document is not what they are looking at
/// while it is open.
pub fn draw_panel(
    scene: &mut codeweaver_render::FrameScene,
    viewport: codeweaver_render::Rect,
    size: codeweaver_render::Size,
    panel: &GitPanel,
    measurer: &mut dyn codeweaver_ui::TextMeasurer,
) {
    use codeweaver_render::{Color, Rect, TextFamily, TextStyle};

    draw_background(scene, viewport);

    // The list of the files that changed.
    let list = Rect::new(16.0, 16.0, LIST_WIDTH, size.height - 60.0);
    scene.push_text(
        "改过的文件".to_owned(),
        Rect::new(list.origin.x, list.origin.y, list.size.width, 20.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.60, 0.65, 0.74),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );

    if panel.entries.is_empty() {
        scene.push_text(
            "没有改动".to_owned(),
            Rect::new(list.origin.x, list.origin.y + 26.0, list.size.width, 20.0),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.52, 0.57, 0.66),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    }

    let room = ((list.size.height - 30.0) / ROW_HEIGHT).floor().max(1.0) as usize;
    let shown = panel.entries.len().min(room);
    let first = panel.selected.saturating_sub(shown - 1).min(panel.entries.len() - 1);
    for (index, entry) in panel.entries.iter().enumerate().skip(first).take(shown) {
        let row = index - first;
        let y = list.origin.y + 26.0 + row as f32 * ROW_HEIGHT;
        if index == panel.selected {
            scene.push_quad(
                Rect::new(list.origin.x - 4.0, y, list.size.width, ROW_HEIGHT),
                Color::rgb(0.18, 0.30, 0.52),
                viewport,
            );
        }
        // The state of the file is in a colour of its own: a file that is
        // staged and a file that is not are two different things.
        let state_color = if entry.staged {
            Color::rgb(0.42, 0.78, 0.56)
        } else {
            Color::rgb(0.93, 0.74, 0.42)
        };
        scene.push_text(
            entry.label(),
            Rect::new(list.origin.x, y + 1.0, LABEL_WIDTH, 20.0),
            TextStyle {
                clip: viewport,
                color: state_color,
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
        scene.push_text(
            entry.path.clone(),
            Rect::new(
                list.origin.x + LABEL_WIDTH + 4.0,
                y + 1.0,
                list.size.width - LABEL_WIDTH - 8.0,
                20.0,
            ),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.86, 0.89, 0.94),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    }

    // The diff of the file that is picked.
    let diff = Rect::new(
        LIST_WIDTH + 32.0,
        16.0,
        (size.width - LIST_WIDTH - 48.0).max(1.0),
        size.height - 60.0,
    );
    scene.push_text(
        "差别".to_owned(),
        Rect::new(diff.origin.x, diff.origin.y, diff.size.width, 20.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.60, 0.65, 0.74),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );

    let lines: Vec<&str> = panel.diff.lines().collect();
    let room = ((diff.size.height - 26.0) / ROW_HEIGHT).floor().max(1.0) as usize;
    let shown = lines.len().min(room);
    let first = lines.len() - shown;
    for (index, line) in lines.iter().skip(first).enumerate() {
        let y = diff.origin.y + 26.0 + index as f32 * ROW_HEIGHT;
        // A line that was taken out and a line that was put in are said in
        // a colour of their own.
        let color = if line.starts_with('+') && !line.starts_with("+++") {
            Color::rgb(0.52, 0.82, 0.58)
        } else if line.starts_with('-') && !line.starts_with("---") {
            Color::rgb(0.92, 0.56, 0.54)
        } else if line.starts_with('@') {
            Color::rgb(0.62, 0.70, 0.86)
        } else {
            Color::rgb(0.72, 0.76, 0.84)
        };
        scene.push_text(
            (*line).to_owned(),
            Rect::new(diff.origin.x, y + 1.0, diff.size.width, 20.0),
            TextStyle {
                clip: viewport,
                color,
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    }
    // The keys the panel takes are said at the bottom of it.
    scene.push_text(
        HINT.to_owned(),
        Rect::new(16.0, size.height - 36.0, size.width - 32.0, 24.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.52, 0.57, 0.66),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );

    let _ = measurer;
}

/// The keys the panel of the changes takes.
pub const HINT: &str = "↑↓ 挑文件 · S 暂存 · C 提交 · Esc 关掉";

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of scratch files for the checks that need one.
    fn scratch_dir(name: &str) -> PathBuf {
        let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory
    }

    /// A repository of its own, with a name to commit under.
    ///
    /// Nothing comes back when git cannot be started here: the checks that
    /// need it are not run at all.
    fn repository(name: &str) -> Option<PathBuf> {
        let directory = scratch_dir(name);
        if git_is_blocked() {
            return None;
        }
        run(&directory, &["init", "--quiet"]).expect("the repository is made");
        run(&directory, &["config", "user.email", "check@example.com"])
            .expect("the email is set");
        run(&directory, &["config", "user.name", "check"]).expect("the name is set");
        Some(directory)
    }

    /// Writes a file into the repository.
    fn write(directory: &Path, name: &str, text: &str) {
        std::fs::write(directory.join(name), text).expect("the file is written");
    }

    #[test]
    fn the_root_of_a_repository_is_the_folder_with_the_git_directory() {
        let Some(directory) = repository("git-root") else {
            return;
        };
        write(&directory, "main.rs", "fn main() {}\n");

        let root = repository_root(&directory.join("main.rs")).expect("there is a repository");
        assert_eq!(root, directory);

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_changed_file_is_in_the_status() {
        let Some(directory) = repository("git-status") else {
            return;
        };
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        // Nothing changed yet.
        let before = status(&directory).expect("the status is read");
        assert!(before.entries.is_empty());

        write(&directory, "main.rs", "fn main() { let x = 1; }\n");
        let after = status(&directory).expect("the status is read");
        assert_eq!(after.entries.len(), 1);
        assert_eq!(after.entries[0].path, "main.rs");
        assert_eq!(after.entries[0].state, "M");
        assert!(!after.entries[0].staged, "the change is not staged yet");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_staged_file_is_staged_in_the_status() {
        let Some(directory) = repository("git-stage") else {
            return;
        };
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        write(&directory, "main.rs", "fn main() { let x = 1; }\n");
        stage(&directory, "main.rs").expect("the file is staged");

        let status = status(&directory).expect("the status is read");
        assert_eq!(status.entries.len(), 1);
        assert!(status.entries[0].staged, "the change is staged");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_diff_of_a_file_says_what_changed() {
        let Some(directory) = repository("git-diff") else {
            return;
        };
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        write(&directory, "main.rs", "fn main() { let x = 1; }\n");
        let working = diff(&directory, "main.rs", false).expect("the diff is read");
        assert!(working.contains("let x = 1;"), "got {working}");

        // The diff of the staging area is empty until the file is staged.
        let staged = diff(&directory, "main.rs", true).expect("the diff is read");
        assert!(staged.trim().is_empty(), "got {staged}");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_commit_takes_the_changes_out_of_the_status() {
        let Some(directory) = repository("git-commit") else {
            return;
        };
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        write(&directory, "main.rs", "fn main() { let x = 1; }\n");
        stage(&directory, "main.rs").expect("the file is staged");
        let said = commit(&directory, "the second one").expect("the commit is made");
        assert!(!said.is_empty(), "git says what it did");

        let status = status(&directory).expect("the status is read");
        assert!(status.entries.is_empty(), "got {:?}", status.entries);

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_commit_with_nothing_staged_says_so() {
        let Some(directory) = repository("git-nothing-to-commit") else {
            return;
        };
        let said = commit(&directory, "nothing to commit").expect_err("there is nothing to commit");
        assert!(
            said.contains("nothing to commit") || said.contains("没有"),
            "got {said}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}