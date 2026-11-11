use std::path::PathBuf;

use anyhow::Result;
use codeweaver_platform::NativeApplication;

/// CodeWeaver's editor.
///
/// ```text
/// codeweaver-editor [file]
/// ```
///
/// With a path, the file is opened into the buffer; without one, the editor
/// starts on an empty document.
fn main() -> Result<()> {
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    NativeApplication::new(path)?.run()
}
