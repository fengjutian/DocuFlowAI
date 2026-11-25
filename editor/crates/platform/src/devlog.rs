//! The log of a development run.
//!
//! What the editor says about itself goes to the terminal it was started
//! from, and a window that is closed takes it with it. That is enough while
//! the editor is in front of you, and not enough the moment something goes
//! wrong in a run nobody watched.
//!
//! In development mode the lines are therefore also written into a file, and
//! the last of them are kept in memory so that the developer panel can show
//! them without touching the disk again.
//!
//! The file belongs to one run: it is emptied when the editor starts, so
//! what is in it is what happened in *this* run and not a mixture of two.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// How many lines are kept in memory for the developer panel.
const TAIL: usize = 400;

/// The log of one run of the editor.
#[derive(Debug)]
pub struct DevLog {
    /// Where the log is written.
    path: PathBuf,
    /// The file the log is written to, when it could be opened.
    file: Option<File>,
    /// Why the file could not be opened, when it could not.
    problem: Option<String>,
    /// The lines that were written, the newest last.
    tail: VecDeque<String>,
    /// When the editor started, so that every line can say how long ago it
    /// happened.
    started: Instant,
}

impl DevLog {
    /// Opens the log at that path.
    ///
    /// The file is emptied: it belongs to this run. A file that cannot be
    /// opened is **not** a reason to refuse to start — the editor says so
    /// and goes on, because a log that is missing is better than an editor
    /// that does not open.
    pub fn open(path: PathBuf) -> Self {
        let started = Instant::now();
        let (file, problem) = match open_the_file(&path) {
            Ok(file) => (Some(file), None),
            Err(error) => (None, Some(error)),
        };
        DevLog {
            path,
            file,
            problem,
            tail: VecDeque::new(),
            started,
        }
    }

    /// Where the log is written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Why the log could not be written, when it could not.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Whether the log is really being written.
    pub fn is_writing(&self) -> bool {
        self.file.is_some()
    }

    /// Writes one line.
    ///
    /// The line says how long ago it happened, so that the order of the
    /// lines can be read against the order of the things that caused them.
    pub fn write(&mut self, line: &str) {
        let seconds = self.started.elapsed().as_secs_f64();
        let entry = format!("{seconds:8.2}s  {line}");
        self.tail.push_back(entry.clone());
        while self.tail.len() > TAIL {
            self.tail.pop_front();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if writeln!(file, "{entry}").is_err() {
            // The file was there a moment ago and is not there now: the log
            // stops here rather than pretending that the line was written.
            self.problem = Some("日志文件写不进去了".to_owned());
            self.file = None;
        }
    }

    /// The lines of the log, the newest last.
    pub fn lines(&self) -> Vec<&str> {
        self.tail.iter().map(|line| line.as_str()).collect()
    }

    /// The last lines of the log, the newest last.
    pub fn last(&self, count: usize) -> Vec<&str> {
        let lines = self.lines();
        let start = lines.len().saturating_sub(count);
        lines[start..].to_vec()
    }
}

/// Opens the file of the log, creating the folder it is in.
fn open_the_file(path: &Path) -> Result<File, String> {
    if let Some(folder) = path.parent()
        && !folder.exists()
    {
        std::fs::create_dir_all(folder).map_err(|error| {
            format!("日志文件夹 {} 建不出来：{error}", folder.display())
        })?;
    }
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|error| format!("日志文件 {} 开不了：{error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch_dir;

    /// A log in a folder of its own.
    fn log(name: &str) -> DevLog {
        let directory = scratch_dir(name);
        DevLog::open(directory.join("editor.log"))
    }

    #[test]
    fn the_log_is_written_to_its_file() {
        let mut log = log("devlog-write");
        assert!(log.is_writing());
        log.write("编辑器起来了");
        let text = std::fs::read_to_string(log.path()).expect("the log is read");
        assert!(text.contains("编辑器起来了"), "got {text}");
    }

    #[test]
    fn the_file_belongs_to_one_run() {
        let directory = scratch_dir("devlog-one-run");
        let path = directory.join("editor.log");
        std::fs::write(&path, "上一次运行写的话\n").unwrap();

        let mut log = DevLog::open(path.clone());
        log.write("这一次运行写的话");
        let text = std::fs::read_to_string(&path).expect("the log is read");
        assert!(!text.contains("上一次运行写的话"), "got {text}");
        assert!(text.contains("这一次运行写的话"), "got {text}");
    }

    #[test]
    fn a_log_that_cannot_be_written_says_why() {
        // A file cannot be opened when a folder is in the way of it.
        let directory = scratch_dir("devlog-broken");
        let path = directory.join("editor.log");
        std::fs::create_dir_all(&path).unwrap();

        let log = DevLog::open(path);
        assert!(!log.is_writing());
        let problem = log.problem().expect("the log says why");
        assert!(!problem.is_empty());
    }

    #[test]
    fn the_lines_are_kept_in_memory() {
        let mut log = log("devlog-tail");
        log.write("第一句");
        log.write("第二句");
        let lines = log.lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("第一句"), "got {}", lines[0]);
        assert!(lines[1].contains("第二句"), "got {}", lines[1]);
    }

    #[test]
    fn the_last_lines_are_the_ones_that_are_asked_for() {
        let mut log = log("devlog-last");
        for number in 0..5 {
            log.write(&format!("第 {number} 句"));
        }
        let last = log.last(2);
        assert_eq!(last.len(), 2);
        assert!(last[0].contains("第 3 句"), "got {}", last[0]);
        assert!(last[1].contains("第 4 句"), "got {}", last[1]);
    }

    #[test]
    fn every_line_says_how_long_ago_it_happened() {
        let mut log = log("devlog-time");
        log.write("一件事");
        let line = log.lines()[0].to_owned();
        assert!(line.contains('s'), "the line says the time: {line}");
    }
}
