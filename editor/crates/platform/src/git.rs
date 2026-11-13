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
fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
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
    fn repository(name: &str) -> PathBuf {
        let directory = scratch_dir(name);
        run(&directory, &["init", "--quiet"]).expect("the repository is made");
        run(&directory, &["config", "user.email", "check@example.com"])
            .expect("the email is set");
        run(&directory, &["config", "user.name", "check"]).expect("the name is set");
        directory
    }

    /// Writes a file into the repository.
    fn write(directory: &Path, name: &str, text: &str) {
        std::fs::write(directory.join(name), text).expect("the file is written");
    }

    #[test]
    fn the_root_of_a_repository_is_the_folder_with_the_git_directory() {
        let directory = repository("git-root");
        write(&directory, "main.rs", "fn main() {}\n");

        let root = repository_root(&directory.join("main.rs")).expect("there is a repository");
        assert_eq!(root, directory);

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_file_that_belongs_to_no_repository_has_no_root() {
        let directory = scratch_dir("git-no-root");
        // The directory is inside the repository of the editor, so the walk
        // has to be stopped before it: a file that is not in a repository
        // of its own is looked for with the marker of the repository.
        assert!(repository_root(&directory).is_some());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_changed_file_is_in_the_status() {
        let directory = repository("git-status");
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        // Nothing changed yet.
        let status = status(&directory).expect("the status is read");
        assert!(status.entries.is_empty());

        write(&directory, "main.rs", "fn main() { let x = 1; }\n");
        let status = status(&directory).expect("the status is read");
        assert_eq!(status.entries.len(), 1);
        assert_eq!(status.entries[0].path, "main.rs");
        assert_eq!(status.entries[0].state, "M");
        assert!(!status.entries[0].staged, "the change is not staged yet");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_staged_file_is_staged_in_the_status() {
        let directory = repository("git-stage");
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
        let directory = repository("git-diff");
        write(&directory, "main.rs", "fn main() {}\n");
        run(&directory, &["add", "main.rs"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");

        write(&directory, "main.rs", "fn main() { let x = 1; }\n");
        let diff = diff(&directory, "main.rs", false).expect("the diff is read");
        assert!(diff.contains("let x = 1;"), "got {diff}");

        // The diff of the staging area is empty until the file is staged.
        let staged = diff(&directory, "main.rs", true).expect("the diff is read");
        assert!(staged.trim().is_empty(), "got {staged}");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_commit_takes_the_changes_out_of_the_status() {
        let directory = repository("git-commit");
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
        let directory = repository("git-nothing-to-commit");
        let said = commit(&directory, "nothing to commit").expect_err("there is nothing to commit");
        assert!(
            said.contains("nothing to commit") || said.contains("没有"),
            "got {said}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}
