//! Running a task and watching what it says.
//!
//! A task is a program that is started on its own: the editor does not wait
//! for it and does not do its work in its own process. The output of the task
//! is read on a thread of its own and put into a buffer that the window reads
//! once per frame, the way the output of the agent is read.
//!
//! The task is stopped when the editor goes away: a task that keeps running
//! after the window is closed is a task nobody is watching.

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

/// What a task is doing now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TaskState {
    /// The task is still running.
    #[default]
    Running,
    /// The task is done, and this is the code it finished with.
    ///
    /// A code of zero is a task that went well.
    Finished(i32),
    /// The task could not be run at all.
    Failed(String),
}

/// What the task has said so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskOutput {
    /// The lines the task has said, the oldest first.
    pub lines: Vec<String>,
    /// What the task is doing now.
    pub state: TaskState,
}

/// The output panel of a task, as the window draws it.
pub struct TaskPanel<'a> {
    /// The lines the task has said.
    pub lines: &'a [String],
    /// What the task is doing now.
    pub state: &'a TaskState,
    /// What the task was asked to do.
    pub title: &'a str,
}

impl TaskPanel<'_> {
    /// What the task is doing, in words.
    ///
    /// A task that fails says how it failed: the code it finished with is
    /// the first thing a person wants to know.
    pub fn state_text(state: &TaskState) -> String {
        match state {
            TaskState::Running => "正在跑…".to_owned(),
            TaskState::Finished(0) => "跑完了".to_owned(),
            TaskState::Finished(code) => format!("失败了，退出码 {code}"),
            TaskState::Failed(reason) => format!("跑不起来：{reason}"),
        }
    }

    /// Whether the task went well.
    pub fn is_failure(state: &TaskState) -> bool {
        match state {
            TaskState::Running => false,
            TaskState::Finished(0) => false,
            TaskState::Finished(_) | TaskState::Failed(_) => true,
        }
    }
}

/// A task that is running.
pub struct Task {
    /// What the task has said.
    output: Arc<Mutex<TaskOutput>>,
    /// The process of the task, so that it can be stopped.
    child: Arc<Mutex<Option<Child>>>,
}

impl Task {
    /// Starts a program and watches what it says.
    ///
    /// The program is started in `directory`. The standard output and the
    /// standard error of the task are read together, so that the order of
    /// the lines is the order the task said them in.
    pub fn start(
        program: &str,
        args: &[String],
        directory: &Path,
    ) -> Result<Self, std::io::Error> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let output = Arc::new(Mutex::new(TaskOutput::default()));
        // The output is read on threads of its own: a task that says a lot
        // must not hold the window.
        for stream in [
            Box::new(stdout) as Box<dyn std::io::Read + Send>,
            Box::new(stderr),
        ] {
            let output = Arc::clone(&output);
            std::thread::spawn(move || {
                let reader = BufReader::new(stream);
                for line in reader.lines().map_while(Result::ok) {
                    let mut output = output.lock().expect("the output lock is not poisoned");
                    output.lines.push(line);
                }
            });
        }

        // The child is shared with the thread that waits for it: the thread
        // asks whether it is done, and the editor may stop it.
        let child = Arc::new(Mutex::new(Some(child)));
        let waiter = Arc::clone(&child);
        let finished = Arc::clone(&output);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(100));
            let mut guard = waiter.lock().expect("the child lock is not poisoned");
            let Some(child) = guard.as_mut() else {
                return;
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code().unwrap_or(-1);
                    let mut output = finished.lock().expect("the output lock is not poisoned");
                    output.state = TaskState::Finished(code);
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    let mut output = finished.lock().expect("the output lock is not poisoned");
                    output.state = TaskState::Failed(error.to_string());
                    return;
                }
            }
        });

        Ok(Task { output, child })
    }

    /// What the task has said so far.
    pub fn output(&self) -> TaskOutput {
        self.output
            .lock()
            .expect("the output lock is not poisoned")
            .clone()
    }

    /// Whether the task is still running.
    pub fn is_running(&self) -> bool {
        self.output().state == TaskState::Running
    }

    /// Stops the task.
    pub fn stop(&self) {
        let mut guard = self.child.lock().expect("the child lock is not poisoned");
        if let Some(child) = guard.as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        // A task that keeps running after the editor is gone is a task
        // nobody is watching.
        self.stop();
    }
}

/// The command that builds the project the given folder belongs to.
///
/// The command is the one the manifest of the project says: a Rust project
/// is built with cargo, a Node one with npm, a Go one with go. A folder that
/// has none of them has nothing to run.
pub fn build_command(root: &Path) -> Option<(String, Vec<String>)> {
    if root.join("Cargo.toml").is_file() {
        return Some(("cargo".to_owned(), vec!["build".to_owned()]));
    }
    if root.join("package.json").is_file() {
        return Some((
            "npm".to_owned(),
            vec!["run".to_owned(), "build".to_owned()],
        ));
    }
    if root.join("go.mod").is_file() {
        return Some(("go".to_owned(), vec!["build".to_owned()]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of scratch files for the checks that need one.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory
    }

    /// Waits for the task to say something, and gives up after a while.
    fn wait_for_output(task: &Task, wanted: impl Fn(&TaskOutput) -> bool) -> TaskOutput {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let output = task.output();
            if wanted(&output) {
                return output;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the task did not say what it was asked to say"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_task_says_what_it_has_to_say() {
        let directory = scratch_dir("task-output");
        let task = Task::start(
            "cmd",
            &[
                "/c".to_owned(),
                "echo hello from the task".to_owned(),
            ],
            &directory,
        )
        .expect("the task starts");

        let output = wait_for_output(&task, |output| {
            output.lines.iter().any(|line| line.contains("hello from the task"))
        });
        assert!(
            output.lines.iter().any(|line| line.contains("hello from the task")),
            "got {:?}",
            output.lines
        );
    }

    #[test]
    fn a_task_that_fails_says_so() {
        let directory = scratch_dir("task-failure");
        let task = Task::start(
            "cmd",
            &["/c".to_owned(), "exit 3".to_owned()],
            &directory,
        )
        .expect("the task starts");

        let output = wait_for_output(&task, |output| {
            output.state != TaskState::Running
        });
        assert_eq!(output.state, TaskState::Finished(3));
    }

    #[test]
    fn a_task_that_cannot_be_started_is_refused() {
        let directory = scratch_dir("task-missing");
        let outcome = Task::start("this-program-is-not-there", &[], &directory);
        assert!(outcome.is_err(), "the editor has to say why it cannot run it");
    }

    #[test]
    fn the_build_command_is_the_one_the_manifest_says() {
        let directory = scratch_dir("task-build");
        assert!(build_command(&directory).is_none());

        std::fs::write(directory.join("Cargo.toml"), "[package]\n").unwrap();
        let (program, args) = build_command(&directory).expect("there is a build command");
        assert_eq!(program, "cargo");
        assert_eq!(args, vec!["build".to_owned()]);

        let directory = scratch_dir("task-build-node");
        std::fs::write(directory.join("package.json"), "{}\n").unwrap();
        let (program, args) = build_command(&directory).expect("there is a build command");
        assert_eq!(program, "npm");
        assert_eq!(args, vec!["run".to_owned(), "build".to_owned()]);
    }
}
