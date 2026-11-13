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
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::rename::RenameBox;

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
    /// The line that is being typed into the task, when it takes input.
    pub input: Option<&'a RenameBox>,
    /// Whether the keyboard is in the line that is being typed.
    ///
    /// A line that is not being typed into is drawn dimmer, and without a
    /// caret: the person has to see that what he types goes somewhere else.
    pub focused: bool,
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
    /// The standard input of the task, when the task takes input.
    ///
    /// A terminal is a task whose input is a pipe: what the person types
    /// goes into it.
    input: Arc<Mutex<Option<ChildStdin>>>,
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
        Self::start_with(program, args, directory, false)
    }

    /// Starts a program that takes input, and watches what it says.
    ///
    /// The standard input of the program is a pipe: what the person types
    /// goes into it. This is what a terminal is.
    pub fn start_with_input(
        program: &str,
        args: &[String],
        directory: &Path,
    ) -> Result<Self, std::io::Error> {
        Self::start_with(program, args, directory, true)
    }

    /// Starts a program, and says whether it takes input.
    fn start_with(
        program: &str,
        args: &[String],
        directory: &Path,
        takes_input: bool,
    ) -> Result<Self, std::io::Error> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(directory)
            .stdin(if takes_input {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let input = child.stdin.take();

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

        Ok(Task {
            output,
            child,
            input: Arc::new(Mutex::new(input)),
        })
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

    /// Writes a line into the task.
    ///
    /// The task says nothing about it: a program that reads its input is
    /// the one that says something.
    pub fn write_input(&self, text: &str) -> Result<(), String> {
        let mut guard = self.input.lock().expect("the input lock is not poisoned");
        let Some(input) = guard.as_mut() else {
            return Err("这个任务不收输入".to_owned());
        };
        input
            .write_all(text.as_bytes())
            .and_then(|_| input.flush())
            .map_err(|error| format!("写不进去：{error}"))
    }

    /// Puts a line into what the task has said.
    ///
    /// A shell that reads its input from a pipe echoes nothing: the editor
    /// says the line itself, so that the panel shows what was typed.
    pub fn echo(&self, line: &str) {
        let mut output = self.output.lock().expect("the output lock is not poisoned");
        output.lines.push(line.to_owned());
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

/// The shell of the machine.
///
/// It is a shell and not a terminal: the program is started with its input
/// as a pipe, so that what the person types goes into it. A program that
/// wants the whole screen (an editor of its own) does not work in it.
pub fn shell_command() -> (&'static str, Vec<String>) {
    #[cfg(windows)]
    {
        ("cmd.exe", Vec::new())
    }
    #[cfg(not(windows))]
    {
        ("sh", Vec::new())
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

/// Splits a command line into the program and its arguments.
///
/// The rules are the ones of a shell: blanks separate the arguments, and
/// what is in quotes is **one** argument, whatever is in it.
pub fn split_command(line: &str) -> Result<(String, Vec<String>), String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    // Whether something was written: an empty pair of quotes is an
    // argument, even though it is empty.
    let mut written = false;
    // The quote that is open, if one is.
    let mut quote: Option<char> = None;

    for character in line.chars() {
        match quote {
            Some(mark) => {
                if character == mark {
                    quote = None;
                } else {
                    current.push(character);
                }
            }
            None => {
                if character == '"' || character == '\'' {
                    quote = Some(character);
                    written = true;
                } else if character.is_whitespace() {
                    if written {
                        parts.push(std::mem::take(&mut current));
                        written = false;
                    }
                } else {
                    current.push(character);
                    written = true;
                }
            }
        }
    }
    if let Some(mark) = quote {
        return Err(format!("引号 {mark} 没关上"));
    }
    if written {
        parts.push(current);
    }
    let Some(program) = parts.first().cloned() else {
        return Err("命令是空的".to_owned());
    };
    let args = parts[1..].to_vec();
    Ok((program, args))
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
    fn a_task_that_takes_input_reads_what_is_written_to_it() {
        let directory = scratch_dir("task-input");
        let task = Task::start_with_input(
            "cmd",
            &[
                "/v:on".to_owned(),
                "/c".to_owned(),
                "set /p answer= && echo you said !answer!".to_owned(),
            ],
            &directory,
        )
        .expect("the task starts");

        task.write_input("hello\n").expect("the input is written");

        let output = wait_for_output(&task, |output| {
            output.lines.iter().any(|line| line.contains("you said hello"))
        });
        assert!(
            output.lines.iter().any(|line| line.contains("you said hello")),
            "got {:?}",
            output.lines
        );
    }

    #[test]
    fn a_task_that_does_not_take_input_says_so() {
        let directory = scratch_dir("task-no-input");
        let task = Task::start("cmd", &["/c".to_owned(), "echo hi".to_owned()], &directory)
            .expect("the task starts");
        let error = task
            .write_input("hello\n")
            .expect_err("the task does not take input");
        assert!(error.contains("不收输入"), "got {error}");
    }

    #[test]
    fn what_the_editor_says_is_in_what_the_task_has_said() {
        let directory = scratch_dir("task-echo");
        let task = Task::start("cmd", &["/c".to_owned(), "echo hi".to_owned()], &directory)
            .expect("the task starts");
        task.echo("the line that was typed");
        let output = task.output();
        assert!(
            output.lines.contains(&"the line that was typed".to_owned()),
            "got {:?}",
            output.lines
        );
    }

    #[test]
    fn the_shell_of_the_machine_is_the_shell_of_the_machine() {
        let (program, args) = shell_command();
        assert!(!program.is_empty());
        assert!(args.is_empty(), "the shell takes no arguments");
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

    #[test]
    fn a_command_line_is_split_into_the_program_and_its_arguments() {
        let (program, args) = split_command("cargo build --release").expect("the line is split");
        assert_eq!(program, "cargo");
        assert_eq!(args, vec!["build".to_owned(), "--release".to_owned()]);
    }

    #[test]
    fn what_is_in_quotes_is_one_argument() {
        let (program, args) =
            split_command("\"C:\\Program Files\\tool.exe\" --flag").expect("the line is split");
        assert_eq!(program, "C:\\Program Files\\tool.exe");
        assert_eq!(args, vec!["--flag".to_owned()]);

        let (program, args) = split_command("echo \"one argument\"").expect("the line is split");
        assert_eq!(program, "echo");
        assert_eq!(args, vec!["one argument".to_owned()]);
    }

    #[test]
    fn a_quote_that_is_not_closed_is_said_out_loud() {
        let error = split_command("echo \"one argument").expect_err("the line is wrong");
        assert!(error.contains("没关上"), "got {error}");
    }

    #[test]
    fn an_empty_command_has_no_program() {
        let error = split_command("   ").expect_err("there is no command");
        assert!(error.contains("空"), "got {error}");
    }
}
