//! The files the checks write to.
//!
//! They live inside the crate's own build directory, because the binary of
//! the checks runs with the low integrity label of the workspace, and a low
//! integrity process cannot write to the folders of the user (the
//! `%LOCALAPPDATA%` of the machine, for one).
//!
//! A directory of scratch files is **not** a project of its own: the editor
//! looks for the project a file belongs to by walking up from the file until
//! it finds a manifest. A file inside the workspace therefore belongs to the
//! crate these checks live in, and a check that looks for text would look in
//! the whole crate instead of in the files of the check. A check that needs
//! a project of its own asks for `scratch_project`, which has a manifest of
//! its own.

use std::fs;
use std::path::PathBuf;

/// The directory the scratch files of the checks live in.
fn root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch");
    fs::create_dir_all(&root).expect("the scratch directory can be created");
    root
}

/// A file of its own, for a check that needs one.
pub(crate) fn scratch(name: &str) -> PathBuf {
    let path = root().join(name);
    let _ = fs::remove_file(&path);
    path
}

/// A directory of its own, for a check that needs more than one file.
pub(crate) fn scratch_dir(name: &str) -> PathBuf {
    let directory = root().join(name);
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("the scratch directory can be created");
    directory
}

/// A project of its own, for a check that needs the files of a project.
///
/// The manifest is an empty workspace, the same as the projects the
/// acceptance scripts use: it says "this folder is a project", and nothing
/// more.
pub(crate) fn scratch_project(name: &str) -> PathBuf {
    let directory = scratch_dir(name);
    fs::write(directory.join("Cargo.toml"), "[workspace]\n")
        .expect("the manifest of the scratch project can be written");
    directory
}
