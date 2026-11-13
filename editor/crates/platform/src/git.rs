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

/// One commit of the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCommit {
    /// The number that names the commit.
    pub id: String,
    /// The message of the commit.
    pub message: String,
}

/// Puts the changes of one file into the staging area.
pub fn stage(root: &Path, path: &str) -> Result<(), String> {
    run(
        root,
        &["add".to_owned(), "--".to_owned(), path.to_owned()],
    )
    .map(|_| ())
}

/// Takes the changes of one file out of the staging area.
///
/// The file itself is not touched: only the index is, so the file goes back
/// to being a change that is not staged.
///
/// `git restore` needs git 2.23; the older way of saying the same thing is
/// the reset, and it is tried when the restore is refused.
pub fn unstage(root: &Path, path: &str) -> Result<(), String> {
    let restored = run(
        root,
        &[
            "restore".to_owned(),
            "--staged".to_owned(),
            "--".to_owned(),
            path.to_owned(),
        ],
    );
    match restored {
        Ok(_) => Ok(()),
        Err(_) => run(
            root,
            &[
                "reset".to_owned(),
                "-q".to_owned(),
                "HEAD".to_owned(),
                "--".to_owned(),
                path.to_owned(),
            ],
        )
        .map(|_| ()),
    }
}

/// The commits of the repository, the newest first.
///
/// The number that names the commit and the message of it are what is
/// wanted; the rest of what git says is not shown.
pub fn commits(root: &Path, count: usize) -> Result<Vec<GitCommit>, String> {
    let output = run(
        root,
        &[
            "log".to_owned(),
            "--format=%h %s".to_owned(),
            format!("-n {count}"),
        ],
    )?;
    let mut commits = Vec::new();
    for line in output.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        // The number of the commit and its message are separated by a space.
        let (id, message) = match line.split_once(' ') {
            Some((id, message)) => (id.to_owned(), message.to_owned()),
            None => (line.to_owned(), String::new()),
        };
        commits.push(GitCommit { id, message });
    }
    Ok(commits)
}

/// What one commit changed.
pub fn show(root: &Path, id: &str) -> Result<String, String> {
    run(root, &["show".to_owned(), id.to_owned()])
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
    /// The commits of the repository, the newest first.
    pub commits: Vec<GitCommit>,
    /// Whether the panel is showing the commits instead of the files.
    pub showing_commits: bool,
}

impl GitPanel {
    /// Makes a panel of the files that changed.
    pub fn new(root: PathBuf, entries: Vec<GitEntry>) -> Self {
        GitPanel {
            root,
            entries,
            selected: 0,
            diff: String::new(),
            commits: Vec::new(),
            showing_commits: false,
        }
    }

    /// The file that is picked.
    pub fn selected_entry(&self) -> Option<&GitEntry> {
        self.entries.get(self.selected)
    }

    /// The commit that is picked.
    pub fn selected_commit(&self) -> Option<&GitCommit> {
        self.commits.get(self.selected)
    }

    /// Which one is picked, counted from the first one.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether the panel is showing the commits instead of the files.
    pub fn is_showing_commits(&self) -> bool {
        self.showing_commits
    }

    /// The keys the panel takes, said at the bottom of it.
    pub fn hint(&self) -> &'static str {
        if self.showing_commits {
            COMMIT_HINT
        } else {
            HINT
        }
    }

    /// How many things there are to pick from.
    fn count(&self) -> usize {
        if self.showing_commits {
            self.commits.len()
        } else {
            self.entries.len()
        }
    }

    /// Picks the one after this one, going round to the first one.
    pub fn select_next(&mut self) {
        let count = self.count();
        if count == 0 {
            return;
        }
        self.selected = (self.selected + 1) % count;
    }

    /// Picks the one before this one, going round to the last one.
    pub fn select_previous(&mut self) {
        let count = self.count();
        if count == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            count - 1
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
/// The width of the column that says which commit it is.
const COMMIT_ID_WIDTH: f32 = 72.0;

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

    // The list of the files that changed, or of the commits.
    let list = Rect::new(16.0, 16.0, LIST_WIDTH, size.height - 60.0);
    scene.push_text(
        if panel.is_showing_commits() {
            "最近的提交".to_owned()
        } else {
            "改过的文件".to_owned()
        },
        Rect::new(list.origin.x, list.origin.y, list.size.width, 20.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.60, 0.65, 0.74),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );

    if panel.is_showing_commits() && panel.commits.is_empty() {
        scene.push_text(
            "还没有提交".to_owned(),
            Rect::new(list.origin.x, list.origin.y + 26.0, list.size.width, 20.0),
            TextStyle {
                clip: viewport,
                color: Color::rgb(0.52, 0.57, 0.66),
                font_size: 13.0,
                line_height: 18.0,
                family: TextFamily::SansSerif,
            },
        );
    } else if !panel.is_showing_commits() && panel.entries.is_empty() {
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
    if panel.is_showing_commits() {
        // The commits, the newest first.
        let shown = panel.commits.len().min(room);
        let first = panel
            .selected()
            .saturating_sub(shown - 1)
            .min(panel.commits.len().saturating_sub(1));
        for (index, commit) in panel.commits.iter().enumerate().skip(first).take(shown) {
            let row = index - first;
            let y = list.origin.y + 26.0 + row as f32 * ROW_HEIGHT;
            if index == panel.selected() {
                scene.push_quad(
                    Rect::new(list.origin.x - 4.0, y, list.size.width, ROW_HEIGHT),
                    Color::rgb(0.18, 0.30, 0.52),
                    viewport,
                );
            }
            // The number that names the commit, in a colour of its own.
            scene.push_text(
                commit.id.clone(),
                Rect::new(list.origin.x, y + 1.0, COMMIT_ID_WIDTH, 20.0),
                TextStyle {
                    clip: viewport,
                    color: Color::rgb(0.42, 0.78, 0.56),
                    font_size: 13.0,
                    line_height: 18.0,
                    family: TextFamily::SansSerif,
                },
            );
            scene.push_text(
                commit.message.clone(),
                Rect::new(
                    list.origin.x + COMMIT_ID_WIDTH + 4.0,
                    y + 1.0,
                    (list.size.width - COMMIT_ID_WIDTH - 8.0).max(1.0),
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
    } else {
        let shown = panel.entries.len().min(room);
        let first = panel
            .selected()
            .saturating_sub(shown - 1)
            .min(panel.entries.len().saturating_sub(1));
        for (index, entry) in panel.entries.iter().enumerate().skip(first).take(shown) {
            let row = index - first;
            let y = list.origin.y + 26.0 + row as f32 * ROW_HEIGHT;
            if index == panel.selected() {
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
        panel.hint().to_owned(),
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
pub const HINT: &str = "↑↓ 挑文件 · S 暂存 · U 取消暂存 · C 提交 · L 提交记录 · Esc 关掉";

/// The keys the panel takes when it is showing the commits.
pub const COMMIT_HINT: &str = "↑↓ 挑提交 · Esc 回到改过的文件";

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

    #[test]
    fn a_staged_file_can_be_taken_out_of_the_staging_area() {
        let Some(directory) = repository("git-unstage") else {
            return;
        };
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        write(&directory, "main.rs", "fn main() { let x = 1; }\n");
        stage(&directory, "main.rs").expect("the file is staged");
        assert!(status(&directory).expect("the status is read").entries[0].staged);

        unstage(&directory, "main.rs").expect("the file is taken out of the staging area");

        let status = status(&directory).expect("the status is read");
        assert_eq!(status.entries.len(), 1, "the file is still there");
        assert!(
            !status.entries[0].staged,
            "the change is not staged any more"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_commits_of_the_repository_are_known() {
        let Some(directory) = repository("git-log") else {
            return;
        };
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        let commits = commits(&directory, 10).expect("the commits are read");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].message, "the first one");
        assert!(!commits[0].id.is_empty(), "the commit has a number");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn what_a_commit_changed_is_known() {
        let Some(directory) = repository("git-show") else {
            return;
        };
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        write(&directory, "main.rs", "fn main() { let x = 1; }\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the second one"]).expect("the commit is made");

        let commits = commits(&directory, 10).expect("the commits are read");
        let newest = commits.first().expect("there is a commit");
        assert_eq!(newest.message, "the second one", "the newest is first");

        let shown = show(&directory, &newest.id).expect("the commit is shown");
        assert!(shown.contains("let x = 1;"), "got {shown}");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_panel_picks_the_commits_when_it_is_showing_them() {
        let panel = GitPanel::new(
            PathBuf::from("C:\\project"),
            vec![GitEntry {
                path: "main.rs".to_owned(),
                state: "M".to_owned(),
                staged: false,
            }],
        );
        assert!(!panel.is_showing_commits());
        assert_eq!(panel.hint(), HINT);

        let mut panel = panel;
        panel.showing_commits = true;
        panel.commits = vec![
            GitCommit {
                id: "abc1234".to_owned(),
                message: "the first one".to_owned(),
            },
            GitCommit {
                id: "def5678".to_owned(),
                message: "the second one".to_owned(),
            },
        ];
        assert!(panel.is_showing_commits());
        assert_eq!(panel.hint(), COMMIT_HINT);
        assert_eq!(panel.selected_commit().map(|commit| commit.id.as_str()), Some("abc1234"));

        // The picking walks the commits, not the files.
        panel.select_next();
        assert_eq!(panel.selected_commit().map(|commit| commit.id.as_str()), Some("def5678"));
        panel.select_next();
        assert_eq!(panel.selected_commit().map(|commit| commit.id.as_str()), Some("abc1234"));
    }
}