//! External change detection: what happened to a file outside the editor.
//!
//! While a file is open, somebody else may write it, or delete it. The
//! editor has to know, because saving over a change that nobody saw loses
//! work. [`ChangeDetector`] watches the files that are open and compares the
//! file on disk with the state the editor recorded when it opened the file.
//!
//! The detector is polled, not pushed: the platform layer calls
//! [`ChangeDetector::poll`] when it has time (between frames, for example) and
//! gets the changes that happened since the last call.
//!
//! Every change is reported once. The recorded state is updated when a change
//! is reported, so polling twice in a row does not report the same change
//! twice. A file that is gone is reported as removed once; the file staying
//! gone is not news. A file that comes back is reported as modified, because
//! its content is new.
//!
//! The detector only looks at the files it watches. It does not watch
//! directories, and it does not look for files that are new on disk.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use crate::Fingerprint;

/// A change that happened to a watched file outside the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalChange {
    /// The file is still there, but its content changed.
    Modified { path: PathBuf },
    /// The file is gone.
    Removed { path: PathBuf },
}

/// What the detector remembers about a watched file.
#[derive(Clone, Copy, Debug)]
struct Watched {
    /// The state of the file on disk the last time it was looked at.
    fingerprint: Fingerprint,
    /// Whether the removal of the file has already been reported.
    missing: bool,
}

/// Watches the files that are open and reports the changes made to them.
#[derive(Clone, Debug, Default)]
pub struct ChangeDetector {
    /// The watched files, keyed by path so that the reports come out in a
    /// stable order.
    watched: BTreeMap<PathBuf, Watched>,
}

impl ChangeDetector {
    /// A detector that watches nothing yet.
    pub fn new() -> Self {
        ChangeDetector {
            watched: BTreeMap::new(),
        }
    }

    /// Starts watching a file, with the state it had when it was opened.
    ///
    /// Watching a path that is already watched updates the recorded state
    /// instead of watching it twice.
    pub fn watch(&mut self, path: &Path, fingerprint: Fingerprint) {
        self.watched.insert(
            path.to_path_buf(),
            Watched {
                fingerprint,
                missing: false,
            },
        );
    }

    /// Stops watching a file.
    pub fn forget(&mut self, path: &Path) {
        self.watched.remove(path);
    }

    /// Looks at the disk and reports what changed since the last call.
    ///
    /// A change is reported once: the recorded fingerprint is updated when the
    /// change is reported, so the same change is not reported again. A file
    /// that is gone is reported once as removed; the file staying gone is not
    /// reported again. The reports come out sorted by path, so the same state
    /// of the disk always gives the same list.
    pub fn poll(&mut self) -> Vec<ExternalChange> {
        let mut changes = Vec::new();

        for (path, watched) in self.watched.iter_mut() {
            match fingerprint_on_disk(path) {
                Ok(current) => {
                    if watched.missing {
                        // The file was gone and is back: its content is new.
                        watched.missing = false;
                        watched.fingerprint = current;
                        changes.push(ExternalChange::Modified { path: path.clone() });
                    } else if current != watched.fingerprint {
                        watched.fingerprint = current;
                        changes.push(ExternalChange::Modified { path: path.clone() });
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if !watched.missing {
                        watched.missing = true;
                        changes.push(ExternalChange::Removed { path: path.clone() });
                    }
                }
                // The file cannot be read (a lock, a permission problem). That
                // is not a change of the file, so nothing is reported.
                Err(_) => {}
            }
        }

        changes
    }

    /// The files that are being watched.
    pub fn watched_paths(&self) -> Vec<PathBuf> {
        self.watched.keys().cloned().collect()
    }
}

/// The state of a file on disk.
///
/// This is the same state [`crate::Fingerprint::of`] records; it is built
/// here directly so that a file that is not there can be told apart from a
/// file that cannot be read.
fn fingerprint_on_disk(path: &Path) -> io::Result<Fingerprint> {
    let metadata = fs::metadata(path)?;
    Ok(Fingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory inside the build output, empty and unique per test.
    fn scratch(name: &str) -> PathBuf {
        let directory = scratch_root().join(name);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory
    }

    /// Where the scratch files live: the build output, which is ignored by git.
    fn scratch_root() -> PathBuf {
        match std::env::var("CARGO_TARGET_TMPDIR") {
            Ok(directory) => PathBuf::from(directory),
            Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp"),
        }
    }

    /// Writes a file and returns its fingerprint.
    fn write_file(path: &Path, text: &str) -> Fingerprint {
        fs::write(path, text).expect("the scratch file can be written");
        fingerprint_on_disk(path).expect("the scratch file can be read")
    }

    #[test]
    fn reports_a_file_that_changed_on_disk() {
        let root = scratch("watch-modified");
        let file = root.join("main.rs");
        let fingerprint = write_file(&file, "first\n");

        let mut detector = ChangeDetector::new();
        detector.watch(&file, fingerprint);

        // The file is written again with a different length, so the change
        // cannot be hidden by the clock's resolution.
        write_file(&file, "second, and longer\n");

        assert_eq!(
            detector.poll(),
            vec![ExternalChange::Modified { path: file.clone() }]
        );
    }

    #[test]
    fn says_nothing_about_an_unchanged_file() {
        let root = scratch("watch-unchanged");
        let file = root.join("main.rs");
        let fingerprint = write_file(&file, "first\n");

        let mut detector = ChangeDetector::new();
        detector.watch(&file, fingerprint);

        assert!(detector.poll().is_empty());
    }

    #[test]
    fn reports_the_same_change_only_once() {
        let root = scratch("watch-once");
        let file = root.join("main.rs");
        let fingerprint = write_file(&file, "first\n");

        let mut detector = ChangeDetector::new();
        detector.watch(&file, fingerprint);
        write_file(&file, "second, and longer\n");

        assert_eq!(detector.poll().len(), 1);
        assert!(
            detector.poll().is_empty(),
            "the change was already reported"
        );
    }

    #[test]
    fn reports_a_removed_file_only_once() {
        let root = scratch("watch-removed");
        let file = root.join("main.rs");
        let fingerprint = write_file(&file, "first\n");

        let mut detector = ChangeDetector::new();
        detector.watch(&file, fingerprint);
        fs::remove_file(&file).unwrap();

        assert_eq!(
            detector.poll(),
            vec![ExternalChange::Removed { path: file.clone() }]
        );
        assert!(
            detector.poll().is_empty(),
            "a file that stays gone is not news"
        );
    }

    #[test]
    fn reports_a_file_that_comes_back_as_modified() {
        let root = scratch("watch-comes-back");
        let file = root.join("main.rs");
        let fingerprint = write_file(&file, "first\n");

        let mut detector = ChangeDetector::new();
        detector.watch(&file, fingerprint);
        fs::remove_file(&file).unwrap();
        assert_eq!(detector.poll().len(), 1);

        write_file(&file, "first\n");
        assert_eq!(
            detector.poll(),
            vec![ExternalChange::Modified { path: file.clone() }]
        );
    }

    #[test]
    fn watching_a_path_twice_updates_the_fingerprint() {
        let root = scratch("watch-twice");
        let file = root.join("main.rs");
        let fingerprint = write_file(&file, "first\n");

        let mut detector = ChangeDetector::new();
        detector.watch(&file, fingerprint);
        write_file(&file, "second, and longer\n");
        let current = fingerprint_on_disk(&file).unwrap();
        detector.watch(&file, current);

        assert_eq!(detector.watched_paths(), vec![file.clone()]);
        assert!(
            detector.poll().is_empty(),
            "the new fingerprint is the one that counts"
        );
    }

    #[test]
    fn forgets_a_file() {
        let root = scratch("watch-forget");
        let file = root.join("main.rs");
        let fingerprint = write_file(&file, "first\n");

        let mut detector = ChangeDetector::new();
        detector.watch(&file, fingerprint);
        write_file(&file, "second, and longer\n");
        detector.forget(&file);

        assert!(detector.watched_paths().is_empty());
        assert!(detector.poll().is_empty());
    }

    #[test]
    fn reports_the_changes_sorted_by_path() {
        let root = scratch("watch-sorted");
        let first = root.join("a.txt");
        let second = root.join("b.txt");
        let first_fingerprint = write_file(&first, "first\n");
        let second_fingerprint = write_file(&second, "first\n");

        let mut detector = ChangeDetector::new();
        detector.watch(&second, second_fingerprint);
        detector.watch(&first, first_fingerprint);
        write_file(&first, "second, and longer\n");
        write_file(&second, "second, and longer\n");

        assert_eq!(
            detector.poll(),
            vec![
                ExternalChange::Modified {
                    path: first.clone()
                },
                ExternalChange::Modified {
                    path: second.clone()
                },
            ]
        );
        assert_eq!(detector.watched_paths(), vec![first, second]);
    }
}
