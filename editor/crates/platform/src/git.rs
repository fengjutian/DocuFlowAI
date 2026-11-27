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

/// One branch of the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// The name of the branch.
    ///
    /// A local branch is named by itself (`main`); a branch on a remote
    /// carries the name of its remote in front (`origin/main`).
    pub name: String,
    /// Whether the program is on this branch right now.
    pub current: bool,
    /// Whether the branch is on a remote rather than here.
    pub remote: bool,
    /// The branch on the remote that this one follows, when it follows one.
    ///
    /// Git writes it as `origin/main`: the name of the remote is the part
    /// in front of the slash.
    pub upstream: Option<String>,
}

impl Branch {
    /// The remote that this branch follows, when it follows one.
    pub fn remote_of(&self) -> Option<&str> {
        remote_of(self.upstream.as_deref()?)
    }
}

/// One remote of the repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// The name of the remote.
    pub name: String,
    /// The address it is read from.
    pub fetch_url: String,
    /// The address it is written to.
    ///
    /// It is the same as the address it is read from unless the remote says
    /// otherwise.
    pub push_url: String,
}

impl Remote {
    /// The address the branch is written to.
    ///
    /// A remote that has no address of its own for writing is written to the
    /// address it is read from.
    pub fn address_to_write_to(&self) -> &str {
        if self.push_url.is_empty() {
            &self.fetch_url
        } else {
            &self.push_url
        }
    }
}

/// The remote that a branch follows.
///
/// The upstream of a branch is written as `origin/main`: the name of the
/// remote is the part in front of the slash.
pub fn remote_of(upstream: &str) -> Option<&str> {
    upstream.split_once('/').map(|(remote, _)| remote)
}

/// One thing in the list of the branches and the remotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchItem {
    /// A branch of the repository.
    Branch(Branch),
    /// A remote of the repository.
    Remote(Remote),
}

/// Reads the branches out of what git said.
///
/// The reading is a pure calculation: git is not asked anything, and the
/// words of git are turned into branches. That way the rules of the reading
/// can be checked without a repository and without git.
pub fn parse_branches(output: &str) -> Vec<Branch> {
    let mut branches = Vec::new();
    for line in output.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        // Every line is the mark of the current branch, the whole name of
        // the reference, and the branch it follows.
        let mut parts = line.split('|');
        let mark = parts.next().unwrap_or_default().trim();
        let reference = parts.next().unwrap_or_default().trim();
        let upstream = parts.next().unwrap_or_default().trim();
        let current = mark == "*";
        if let Some(name) = reference.strip_prefix("refs/heads/") {
            branches.push(Branch {
                name: name.to_owned(),
                current,
                remote: false,
                upstream: if upstream.is_empty() {
                    None
                } else {
                    Some(upstream.to_owned())
                },
            });
        } else if let Some(name) = reference.strip_prefix("refs/remotes/") {
            // The pointer of the remote is not a branch: `origin/HEAD` is
            // the branch the remote was on, and it moves under the feet of
            // the person who reads it.
            if name.ends_with("/HEAD") {
                continue;
            }
            branches.push(Branch {
                name: name.to_owned(),
                current,
                remote: true,
                upstream: None,
            });
        }
    }
    branches
}

/// Reads the remotes out of what git said.
///
/// Git writes one line for reading and one line for writing, and the two
/// are the same address unless the remote says otherwise.
pub fn parse_remotes(output: &str) -> Vec<Remote> {
    let mut remotes: Vec<Remote> = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let rest = rest.trim();
        let (address, kind) = match rest.strip_suffix("(fetch)") {
            Some(address) => (address.trim(), "fetch"),
            None => match rest.strip_suffix("(push)") {
                Some(address) => (address.trim(), "push"),
                None => (rest, ""),
            },
        };
        if name.is_empty() {
            continue;
        }
        let index = match remotes.iter().position(|remote| remote.name == name) {
            Some(index) => index,
            None => {
                remotes.push(Remote {
                    name: name.to_owned(),
                    fetch_url: String::new(),
                    push_url: String::new(),
                });
                remotes.len() - 1
            }
        };
        match kind {
            "fetch" => remotes[index].fetch_url = address.to_owned(),
            "push" => remotes[index].push_url = address.to_owned(),
            _ => {}
        }
    }
    remotes
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

/// The branches of the repository: the ones that are here and the ones that
/// are on the remotes.
pub fn branches(root: &Path) -> Result<Vec<Branch>, String> {
    let output = run(
        root,
        &[
            "branch".to_owned(),
            "-a".to_owned(),
            "--format=%(HEAD)|%(refname)|%(upstream:short)".to_owned(),
        ],
    )?;
    Ok(parse_branches(&output))
}

/// The remotes of the repository.
pub fn remotes(root: &Path) -> Result<Vec<Remote>, String> {
    let output = run(root, &["remote".to_owned(), "-v".to_owned()])?;
    Ok(parse_remotes(&output))
}

/// The branch the program is on.
///
/// There is no branch yet in a repository that has no commit; that is said
/// by an empty name, and not by an error.
pub fn current_branch(root: &Path) -> Result<String, String> {
    let output = run(
        root,
        &["rev-parse".to_owned(), "--abbrev-ref".to_owned(), "HEAD".to_owned()],
    )?;
    let name = output.trim();
    Ok(if name == "HEAD" {
        String::new()
    } else {
        name.to_owned()
    })
}

/// Goes to another branch.
///
/// `git switch` needs git 2.23; the older way of saying the same thing is
/// the checkout, and it is tried when the switch is refused.
pub fn switch_to(root: &Path, name: &str) -> Result<(), String> {
    match run(root, &["switch".to_owned(), name.to_owned()]) {
        Ok(_) => Ok(()),
        Err(_) => run(root, &["checkout".to_owned(), name.to_owned()]).map(|_| ()),
    }
}

/// Makes a branch and goes to it.
///
/// The older git has no switch, and the checkout is the way of saying the
/// same thing there.
pub fn create_branch(root: &Path, name: &str) -> Result<(), String> {
    match run(root, &["switch".to_owned(), "-c".to_owned(), name.to_owned()]) {
        Ok(_) => Ok(()),
        Err(_) => run(
            root,
            &["checkout".to_owned(), "-b".to_owned(), name.to_owned()],
        )
        .map(|_| ()),
    }
}

/// Takes what the remotes have, without changing anything here.
///
/// Without a remote, every remote is taken from.
pub fn fetch(root: &Path, remote: Option<&str>) -> Result<String, String> {
    match remote {
        Some(name) => run(root, &["fetch".to_owned(), name.to_owned()]),
        None => run(root, &["fetch".to_owned(), "--all".to_owned()]),
    }
}

/// Brings what the remote has into the branch that is checked out.
///
/// Only a fast-forward is done: a merge would open a window of its own to
/// write the message in, and the editor has no such window. When the
/// branches have gone apart, git says so and nothing is changed.
pub fn pull(root: &Path) -> Result<String, String> {
    run(root, &["pull".to_owned(), "--ff-only".to_owned()])
}

/// Sends what is here to the remote.
///
/// The branch is named so that it goes where the person wants it to go, and
/// not to whatever the remote thinks the branch is called.
pub fn push(root: &Path, remote: Option<&str>, branch: &str) -> Result<String, String> {
    match remote {
        Some(name) => run(
            root,
            &["push".to_owned(), name.to_owned(), branch.to_owned()],
        ),
        None => run(root, &["push".to_owned()]),
    }
}

/// Runs a git command in the repository and returns what it said.
pub(crate) fn run(root: &Path, args: &[impl AsRef<str>]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args.iter().map(|argument| argument.as_ref()))
        .current_dir(root)
        // Git is given nothing to read: a remote that asks for a password
        // would wait for an answer that never comes, and the editor would
        // hang with the window open. Without the answer, git says that it
        // could not ask, and the panel says that back.
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("git 起不来：{error}"))?;
    if !output.status.success() {
        // Git says why it failed in either of the two streams: `git commit`
        // writes "nothing to commit" to the output, and other commands write
        // their complaint to the error. The person has to be told which one
        // it was, so both are read.
        let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let out = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let said = if said.is_empty() { out } else { said };
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

/// Which page of the panel of the changes is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitView {
    /// The files that changed.
    #[default]
    Files,
    /// The commits of the repository.
    Commits,
    /// The branches and the remotes.
    Branches,
}

/// What the box of git is asking for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitAsking {
    /// The name of the branch to make.
    NewBranch,
    /// The confirmation of a push: where it goes.
    Push {
        /// The remote it goes to.
        remote: String,
        /// The branch that goes there.
        branch: String,
    },
}

/// The box that asks git something.
///
/// Pushing is a thing that cannot be undone from here, so it is asked for
/// first: the box says where the branch goes, and the push only happens
/// when the answer is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitBox {
    /// What is being asked.
    pub asking: GitAsking,
    /// The text that is being written.
    pub input: crate::rename::RenameBox,
}

impl GitBox {
    /// A box that asks for the name of a branch to make.
    pub fn new_branch() -> Self {
        GitBox {
            asking: GitAsking::NewBranch,
            input: crate::rename::RenameBox::message("新分支的名字"),
        }
    }

    /// A box that asks whether the branch may be pushed.
    pub fn confirm_push(remote: &str, branch: &str) -> Self {
        GitBox {
            asking: GitAsking::Push {
                remote: remote.to_owned(),
                branch: branch.to_owned(),
            },
            input: crate::rename::RenameBox::message(format!("推到 {remote} 的 {branch}？回车确认")),
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
    /// Which page of the panel is shown.
    pub view: GitView,
    /// The branches and the remotes, in the order they are shown.
    pub branch_items: Vec<BranchItem>,
    /// The branch the program is on, when there is one.
    pub current_branch: String,
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
            view: GitView::Files,
            branch_items: Vec::new(),
            current_branch: String::new(),
        }
    }

    /// Reads the branches and the remotes of the repository.
    ///
    /// The branches come first and the remotes after them: the branches are
    /// what is worked on, and the remotes are where it goes.
    pub fn load_branches(&mut self) -> Result<(), String> {
        let branches = branches(&self.root)?;
        let remotes = remotes(&self.root)?;
        self.current_branch = current_branch(&self.root)?;
        let mut items: Vec<BranchItem> = branches.into_iter().map(BranchItem::Branch).collect();
        items.extend(remotes.into_iter().map(BranchItem::Remote));
        self.branch_items = items;
        Ok(())
    }

    /// Which page of the panel is shown.
    pub fn view(&self) -> GitView {
        self.view
    }

    /// The file that is picked.
    pub fn selected_entry(&self) -> Option<&GitEntry> {
        self.entries.get(self.selected)
    }

    /// The commit that is picked.
    pub fn selected_commit(&self) -> Option<&GitCommit> {
        self.commits.get(self.selected)
    }

    /// The branch that is picked.
    pub fn selected_branch(&self) -> Option<&Branch> {
        match self.branch_items.get(self.selected) {
            Some(BranchItem::Branch(branch)) => Some(branch),
            _ => None,
        }
    }

    /// The remote that is picked.
    pub fn selected_remote(&self) -> Option<&Remote> {
        match self.branch_items.get(self.selected) {
            Some(BranchItem::Remote(remote)) => Some(remote),
            _ => None,
        }
    }

    /// Which one is picked, counted from the first one.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The keys the panel takes, said at the bottom of it.
    pub fn hint(&self) -> &'static str {
        match self.view {
            GitView::Files => HINT,
            GitView::Commits => COMMIT_HINT,
            GitView::Branches => BRANCH_HINT,
        }
    }

    /// How many things there are to pick from.
    fn count(&self) -> usize {
        match self.view {
            GitView::Files => self.entries.len(),
            GitView::Commits => self.commits.len(),
            GitView::Branches => self.branch_items.len(),
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
/// The width of the column that says the name of the remote.
const REMOTE_NAME_WIDTH: f32 = 96.0;

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

    // The list of the files that changed, or of the commits, or of the
    // branches.
    let list = Rect::new(16.0, 16.0, LIST_WIDTH, size.height - 60.0);
    let (title, empty) = match panel.view() {
        GitView::Files => (
            "改过的文件",
            if panel.entries.is_empty() {
                Some("没有改动")
            } else {
                None
            },
        ),
        GitView::Commits => (
            "最近的提交",
            if panel.commits.is_empty() {
                Some("还没有提交")
            } else {
                None
            },
        ),
        GitView::Branches => (
            "分支与远端",
            if panel.branch_items.is_empty() {
                Some("没有分支")
            } else {
                None
            },
        ),
    };
    scene.push_text(
        title.to_owned(),
        Rect::new(list.origin.x, list.origin.y, list.size.width, 20.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.60, 0.65, 0.74),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );

    if let Some(empty) = empty {
        scene.push_text(
            empty.to_owned(),
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
    if panel.view() == GitView::Branches {
        // The branches, and then the remotes.
        let shown = panel.branch_items.len().min(room);
        let first = panel
            .selected()
            .saturating_sub(shown - 1)
            .min(panel.branch_items.len().saturating_sub(1));
        for (index, item) in panel.branch_items.iter().enumerate().skip(first).take(shown) {
            let row = index - first;
            let y = list.origin.y + 26.0 + row as f32 * ROW_HEIGHT;
            if index == panel.selected() {
                scene.push_quad(
                    Rect::new(list.origin.x - 4.0, y, list.size.width, ROW_HEIGHT),
                    Color::rgb(0.18, 0.30, 0.52),
                    viewport,
                );
            }
            match item {
                BranchItem::Branch(branch) => {
                    // The branch that is checked out is in a colour of its
                    // own and has a mark in front of it: where the program
                    // is, is something a person has to see.
                    let (text, color) = if branch.current {
                        (format!("● {}", branch.name), Color::rgb(0.42, 0.78, 0.56))
                    } else if branch.remote {
                        (branch.name.clone(), Color::rgb(0.52, 0.57, 0.66))
                    } else {
                        (branch.name.clone(), Color::rgb(0.86, 0.89, 0.94))
                    };
                    scene.push_text(
                        text,
                        Rect::new(list.origin.x, y + 1.0, list.size.width, 20.0),
                        TextStyle {
                            clip: viewport,
                            color,
                            font_size: 13.0,
                            line_height: 18.0,
                            family: TextFamily::SansSerif,
                        },
                    );
                }
                BranchItem::Remote(remote) => {
                    // The name of the remote, and where it is.
                    scene.push_text(
                        remote.name.clone(),
                        Rect::new(list.origin.x, y + 1.0, REMOTE_NAME_WIDTH, 20.0),
                        TextStyle {
                            clip: viewport,
                            color: Color::rgb(0.93, 0.74, 0.42),
                            font_size: 13.0,
                            line_height: 18.0,
                            family: TextFamily::SansSerif,
                        },
                    );
                    scene.push_text(
                        remote.fetch_url.clone(),
                        Rect::new(
                            list.origin.x + REMOTE_NAME_WIDTH + 4.0,
                            y + 1.0,
                            (list.size.width - REMOTE_NAME_WIDTH - 8.0).max(1.0),
                            20.0,
                        ),
                        TextStyle {
                            clip: viewport,
                            color: Color::rgb(0.62, 0.70, 0.86),
                            font_size: 13.0,
                            line_height: 18.0,
                            family: TextFamily::SansSerif,
                        },
                    );
                }
            }
        }
    } else if panel.view() == GitView::Commits {
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

    // The diff of the file that is picked, or what is known about the branch
    // or the remote that is picked.
    let diff = Rect::new(
        LIST_WIDTH + 32.0,
        16.0,
        (size.width - LIST_WIDTH - 48.0).max(1.0),
        size.height - 60.0,
    );
    let details = match panel.view() {
        GitView::Branches => match (panel.selected_branch(), panel.selected_remote()) {
            (Some(branch), _) => {
                let mut lines = vec![format!("分支：{}", branch.name)];
                lines.push(match branch.upstream.as_deref() {
                    Some(upstream) => format!("跟着：{upstream}"),
                    None => "没有跟着哪个分支".to_owned(),
                });
                lines.push(if branch.remote {
                    "在远端上".to_owned()
                } else {
                    "在这个仓库里".to_owned()
                });
                lines
            }
            (_, Some(remote)) => vec![
                format!("远端：{}", remote.name),
                format!("取回：{}", remote.fetch_url),
                format!("推送：{}", remote.address_to_write_to()),
            ],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    scene.push_text(
        if panel.view() == GitView::Branches {
            "详情".to_owned()
        } else {
            "差别".to_owned()
        },
        Rect::new(diff.origin.x, diff.origin.y, diff.size.width, 20.0),
        TextStyle {
            clip: viewport,
            color: Color::rgb(0.60, 0.65, 0.74),
            font_size: 13.0,
            line_height: 18.0,
            family: TextFamily::SansSerif,
        },
    );

    let lines: Vec<&str> = if details.is_empty() {
        panel.diff.lines().collect()
    } else {
        details.iter().map(String::as_str).collect()
    };
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

/// The keys the panel takes when it is showing the branches and the remotes.
pub const BRANCH_HINT: &str =
    "↑↓ 挑分支 · Enter 切过去 · N 新建分支 · F 取回 · U 更新 · P 推送 · B 回到改过的文件";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch_dir;

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

    /// A repository of its own, with one commit in it.
    fn repository_with_a_commit(name: &str) -> Option<PathBuf> {
        let directory = repository(name)?;
        write(&directory, "notes.txt", "the first line\n");
        run(&directory, &["add", "notes.txt"]).expect("the file is added");
        run(&directory, &["commit", "-m", "the first one"]).expect("the commit is made");
        Some(directory)
    }

    /// A remote of its own: a bare repository is one that has nothing in it
    /// but the history, and it is what a remote is.
    ///
    /// The remote is a folder on this machine: the checks push to it and
    /// pull from it without asking anything of the network.
    fn remote_repository(name: &str) -> Option<PathBuf> {
        let directory = scratch_dir(name);
        if git_is_blocked() {
            return None;
        }
        run(&directory, &["init", "--quiet", "--bare"]).expect("the remote is made");
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
        assert_eq!(panel.view(), GitView::Files);
        assert_eq!(panel.hint(), HINT);

        let mut panel = panel;
        panel.view = GitView::Commits;
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
        assert_eq!(panel.view(), GitView::Commits);
        assert_eq!(panel.hint(), COMMIT_HINT);
        assert_eq!(panel.selected_commit().map(|commit| commit.id.as_str()), Some("abc1234"));

        // The picking walks the commits, not the files.
        panel.select_next();
        assert_eq!(panel.selected_commit().map(|commit| commit.id.as_str()), Some("def5678"));
        panel.select_next();
        assert_eq!(panel.selected_commit().map(|commit| commit.id.as_str()), Some("abc1234"));
    }

    #[test]
    fn the_branches_are_read_out_of_what_git_said() {
        let said = " |refs/heads/feature/one||a6c78ab\n\
                    *|refs/heads/main|origin/main|41356b7\n\
                     |refs/remotes/origin/HEAD||41356b7\n\
                     |refs/remotes/origin/main||41356b7\n";
        let branches = parse_branches(said);
        assert_eq!(branches.len(), 3, "the pointer of the remote is not a branch");

        assert_eq!(branches[0].name, "feature/one");
        assert!(!branches[0].current, "the program is not on it");
        assert!(!branches[0].remote, "it is here");
        assert_eq!(branches[0].upstream, None);

        assert_eq!(branches[1].name, "main");
        assert!(branches[1].current, "the program is on it");
        assert_eq!(branches[1].upstream.as_deref(), Some("origin/main"));
        assert_eq!(branches[1].remote_of(), Some("origin"));

        assert_eq!(branches[2].name, "origin/main");
        assert!(branches[2].remote, "it is on the remote");
        assert!(!branches[2].current);
    }

    #[test]
    fn nothing_that_git_said_is_no_branch() {
        assert!(parse_branches("").is_empty());
        assert!(parse_branches("\n\n").is_empty());
        // A reference that is neither here nor on a remote is not a branch.
        assert!(parse_branches(" |refs/tags/v1||a6c78ab\n").is_empty());
    }

    #[test]
    fn the_remotes_are_read_out_of_what_git_said() {
        let said = "origin\thttps://github.com/x/y.git (fetch)\n\
                    origin\thttps://github.com/x/y.git (push)\n\
                    other\tC:\\work\\other.git (fetch)\n\
                    other\tC:\\work\\other.git (push)\n";
        let remotes = parse_remotes(said);
        assert_eq!(remotes.len(), 2, "the two lines of one remote are one remote");

        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].fetch_url, "https://github.com/x/y.git");
        assert_eq!(remotes[0].push_url, "https://github.com/x/y.git");
        assert_eq!(remotes[0].address_to_write_to(), "https://github.com/x/y.git");

        assert_eq!(remotes[1].name, "other");
        assert_eq!(remotes[1].fetch_url, "C:\\work\\other.git");
    }

    #[test]
    fn a_remote_that_has_no_address_of_its_own_is_written_to_the_one_it_is_read_from() {
        let remotes = parse_remotes("origin\thttps://github.com/x/y.git (fetch)\n");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].push_url, "");
        assert_eq!(
            remotes[0].address_to_write_to(),
            "https://github.com/x/y.git"
        );
    }

    #[test]
    fn the_branches_of_the_repository_are_known() {
        let Some(directory) = repository_with_a_commit("git-branches") else {
            return;
        };
        run(&directory, &["branch", "feature"]).expect("the branch is made");

        let branches = branches(&directory).expect("the branches are read");
        assert_eq!(branches.len(), 2, "got {branches:?}");
        let current = branches.iter().find(|branch| branch.current).expect("one is current");
        assert!(!current.name.is_empty(), "the current branch has a name");
        assert!(
            branches.iter().any(|branch| branch.name == "feature"),
            "the other branch is there: {branches:?}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn another_branch_can_be_went_to() {
        let Some(directory) = repository_with_a_commit("git-switch") else {
            return;
        };
        run(&directory, &["branch", "feature"]).expect("the branch is made");

        switch_to(&directory, "feature").expect("the branch is gone to");
        assert_eq!(current_branch(&directory).expect("the branch is known"), "feature");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_branch_can_be_made_and_gone_to() {
        let Some(directory) = repository_with_a_commit("git-create-branch") else {
            return;
        };

        create_branch(&directory, "made-here").expect("the branch is made");
        assert_eq!(
            current_branch(&directory).expect("the branch is known"),
            "made-here"
        );
        let branches = branches(&directory).expect("the branches are read");
        assert!(
            branches.iter().any(|branch| branch.name == "made-here"),
            "the branch is there: {branches:?}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_remotes_of_the_repository_are_known() {
        let Some(directory) = repository_with_a_commit("git-remotes") else {
            return;
        };
        let Some(remote) = remote_repository("git-remotes-remote") else {
            return;
        };
        let address = remote.to_string_lossy().to_string();
        run(&directory, &["remote", "add", "origin", &address]).expect("the remote is added");

        let remotes = remotes(&directory).expect("the remotes are read");
        assert_eq!(remotes.len(), 1, "got {remotes:?}");
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].fetch_url, address);
        assert_eq!(remotes[0].push_url, address);

        let _ = std::fs::remove_dir_all(&directory);
        let _ = std::fs::remove_dir_all(&remote);
    }

    #[test]
    fn what_is_here_can_be_pushed_to_the_remote() {
        let Some(directory) = repository_with_a_commit("git-push") else {
            return;
        };
        let Some(remote) = remote_repository("git-push-remote") else {
            return;
        };
        let address = remote.to_string_lossy().to_string();
        run(&directory, &["remote", "add", "origin", &address]).expect("the remote is added");

        let branch = current_branch(&directory).expect("the branch is known");
        push(&directory, Some("origin"), &branch).expect("the branch is pushed");

        // The remote has the commit now: it is read back from the remote.
        let said = run(&remote, &["log", "--format=%s"]).expect("the remote is read");
        assert!(said.contains("the first one"), "got {said}");

        let _ = std::fs::remove_dir_all(&directory);
        let _ = std::fs::remove_dir_all(&remote);
    }

    #[test]
    fn what_the_remote_has_can_be_brought_in() {
        let Some(directory) = repository_with_a_commit("git-pull") else {
            return;
        };
        let Some(remote) = remote_repository("git-pull-remote") else {
            return;
        };
        let address = remote.to_string_lossy().to_string();
        run(&directory, &["remote", "add", "origin", &address]).expect("the remote is added");
        let branch = current_branch(&directory).expect("the branch is known");
        push(&directory, Some("origin"), &branch).expect("the branch is pushed");

        // Another clone of the same remote gets a commit that is not here.
        let Some(other) = repository("git-pull-other") else {
            return;
        };
        run(&other, &["remote", "add", "origin", &address]).expect("the remote is added");
        run(&other, &["fetch", "origin"]).expect("the remote is read");
        write(&other, "notes.txt", "the second line\n");
        run(&other, &["add", "notes.txt"]).expect("the file is added");
        run(&other, &["commit", "-m", "the second one"]).expect("the commit is made");
        run(&other, &["push", "origin", "HEAD:refs/heads/main"]).expect("the commit is pushed");

        // The change is brought in here.
        pull(&directory).expect("the remote is brought in");
        let text = std::fs::read_to_string(directory.join("notes.txt")).expect("the file is read");
        assert!(text.contains("the second line"), "got {text}");

        let _ = std::fs::remove_dir_all(&directory);
        let _ = std::fs::remove_dir_all(&remote);
        let _ = std::fs::remove_dir_all(&other);
    }
}