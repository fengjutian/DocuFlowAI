//! Workspace services: files on disk.
//!
//! The editor's UI never touches the file system directly — it calls this
//! crate's API, which keeps the "UI only depends on service interfaces" rule
//! from the architecture document true.
//!
//! A [`Document`] is a file plus the state needed to write it back safely:
//! the text it was loaded with, and a fingerprint of the file as it was on
//! disk. The fingerprint is what stops the editor from silently overwriting a
//! file that something else changed in the meantime.
//!
//! Two models sit next to it: [`FileTree`], the tree of the workspace that
//! the file panel draws, and [`ChangeDetector`], which reports the files that
//! changed on disk while they were open.
//!
//! A third service lives beside them: [`search`], which finds text in the
//! files of the project.

pub mod search;
pub mod tree;
pub mod watch;

pub use search::{SearchHit, SearchOptions, SearchResults};
pub use tree::{FileTree, NodeKind, TreeNode};
pub use watch::{ChangeDetector, ExternalChange};

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// What went wrong while reading or writing a file.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid UTF-8 text")]
    NotUtf8 { path: PathBuf },
    #[error("{path} was changed on disk since it was opened; saving would overwrite those changes")]
    ChangedOnDisk { path: PathBuf },
    #[error("the file system is not available: {0}")]
    Io(#[from] std::io::Error),
}

/// The state of a file on disk, used to detect outside changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    /// The file's length in bytes.
    pub len: u64,
    /// The file's modification time.
    pub modified: Option<SystemTime>,
}

impl Fingerprint {
    /// Reads the fingerprint of a file.
    pub fn of(path: &Path) -> Result<Self, WorkspaceError> {
        let metadata = fs::metadata(path).map_err(|source| WorkspaceError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Fingerprint {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

/// A file opened in the editor.
#[derive(Clone, Debug)]
pub struct Document {
    path: PathBuf,
    /// The text the file had when it was opened.
    saved_text: String,
    /// The file's state on disk when it was opened.
    fingerprint: Fingerprint,
}

impl Document {
    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's path, ready to show to a person.
    ///
    /// Windows' verbatim prefix (`\\?\`) is an implementation detail of the
    /// path API; showing it in a status line helps nobody.
    pub fn display_path(&self) -> String {
        display_path(&self.path)
    }

    /// The text the file had when it was opened.
    pub fn saved_text(&self) -> &str {
        &self.saved_text
    }

    /// The file's state on disk when it was opened.
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    /// Whether `text` differs from what is on disk.
    pub fn is_dirty(&self, text: &str) -> bool {
        text != self.saved_text
    }
}

/// Reads and writes the files of a workspace.
#[derive(Clone, Debug, Default)]
pub struct Workspace {
    root: Option<PathBuf>,
}

impl Workspace {
    /// A workspace rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Workspace {
            root: Some(root.into()),
        }
    }

    /// A workspace with no root, used for single files.
    pub fn detached() -> Self {
        Workspace { root: None }
    }

    /// The workspace root, if it has one.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Reads a file.
    pub fn open(&self, path: &Path) -> Result<Document, WorkspaceError> {
        let fingerprint = Fingerprint::of(path)?;
        let bytes = read_bytes(path)?;
        let text = decode_utf8(path, &bytes)?;
        Ok(Document {
            path: path.to_path_buf(),
            saved_text: text,
            fingerprint,
        })
    }

    /// Writes `text` to the document's file.
    ///
    /// The write is refused when the file changed on disk since it was
    /// opened: overwriting somebody else's change is worse than failing.
    pub fn save(&self, document: &mut Document, text: &str) -> Result<(), WorkspaceError> {
        let current = Fingerprint::of(&document.path).ok();
        if let Some(current) = current
            && current != document.fingerprint
        {
            return Err(WorkspaceError::ChangedOnDisk {
                path: document.path.clone(),
            });
        }

        write_text(&document.path, text)?;

        document.saved_text = text.to_owned();
        document.fingerprint = Fingerprint::of(&document.path)?;
        Ok(())
    }

    /// Writes `text` to a different path and points the document at it.
    pub fn save_as(
        &self,
        document: &mut Document,
        path: &Path,
        text: &str,
    ) -> Result<(), WorkspaceError> {
        write_text(path, text)?;
        document.path = path.to_path_buf();
        document.saved_text = text.to_owned();
        document.fingerprint = Fingerprint::of(path)?;
        Ok(())
    }
}

/// Renders a path for a person to read.
///
/// Windows' verbatim prefix (`\\?\`) is an implementation detail of the path
/// API; showing it in a status line helps nobody.
pub fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(restored) => restored.to_owned(),
        None => text,
    }
}

/// Reads a file's bytes.
fn read_bytes(path: &Path) -> Result<Vec<u8>, WorkspaceError> {
    let mut file = fs::File::open(path).map_err(|source| WorkspaceError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| WorkspaceError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(bytes)
}

/// Decodes bytes as UTF-8 text, dropping a byte-order mark.
fn decode_utf8(path: &Path, bytes: &[u8]) -> Result<String, WorkspaceError> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(bytes.to_vec()).map_err(|_| WorkspaceError::NotUtf8 {
        path: path.to_path_buf(),
    })
}

/// Writes text to a file.
fn write_text(path: &Path, text: &str) -> Result<(), WorkspaceError> {
    fs::write(path, text).map_err(|source| WorkspaceError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch file inside the workspace's own directory.
    fn scratch(name: &str) -> PathBuf {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
        fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory.join(name)
    }

    #[test]
    fn opens_and_saves_a_file() {
        let path = scratch("open-save.txt");
        fs::write(&path, "hello\n").unwrap();
        let workspace = Workspace::detached();

        let mut document = workspace.open(&path).unwrap();
        assert_eq!(document.saved_text(), "hello\n");
        assert!(!document.is_dirty("hello\n"));
        assert!(document.is_dirty("hello world\n"));

        workspace.save(&mut document, "hello world\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world\n");
        assert!(!document.is_dirty("hello world\n"));
    }

    #[test]
    fn strips_a_byte_order_mark() {
        let path = scratch("bom.txt");
        fs::write(&path, [0xEF, 0xBB, 0xBF, b'h', b'i']).unwrap();
        let document = Workspace::detached().open(&path).unwrap();
        assert_eq!(document.saved_text(), "hi");
    }

    #[test]
    fn rejects_a_file_that_is_not_utf8() {
        let path = scratch("binary.bin");
        fs::write(&path, [0xFF, 0xFE, 0x00]).unwrap();
        let error = Workspace::detached().open(&path).unwrap_err();
        assert!(matches!(error, WorkspaceError::NotUtf8 { .. }));
    }

    #[test]
    fn refuses_to_overwrite_a_file_that_changed_on_disk() {
        let path = scratch("changed.txt");
        fs::write(&path, "first\n").unwrap();
        let workspace = Workspace::detached();
        let mut document = workspace.open(&path).unwrap();

        // Something else writes the file after the editor opened it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, "second, and longer\n").unwrap();

        let error = workspace.save(&mut document, "mine\n").unwrap_err();
        assert!(matches!(error, WorkspaceError::ChangedOnDisk { .. }));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "second, and longer\n",
            "the outside change must survive"
        );
    }

    #[test]
    fn reports_a_missing_file() {
        let error = Workspace::detached()
            .open(&scratch("does-not-exist.txt"))
            .unwrap_err();
        assert!(matches!(error, WorkspaceError::Read { .. }));
    }

    #[test]
    fn the_display_path_drops_the_windows_verbatim_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\repo\main.rs")),
            r"C:\repo\main.rs"
        );
        assert_eq!(
            display_path(Path::new(r"C:\repo\main.rs")),
            r"C:\repo\main.rs"
        );
    }
}
