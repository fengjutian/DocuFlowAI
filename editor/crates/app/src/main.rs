use std::path::PathBuf;

use anyhow::Result;
use codeweaver_platform::NativeApplication;

/// CodeWeaver's editor.
///
/// ```text
/// codeweaver-editor [file] [--find <text>]
/// ```
///
/// With a path, the file is opened into the buffer; without one, the editor
/// starts on an empty document. With `--find`, the editor starts with the
/// find bar open and the text already searched for.
fn main() -> Result<()> {
    let mut path: Option<PathBuf> = None;
    let mut query: Option<String> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--find" {
            query = args.next().and_then(|value| value.into_string().ok());
        } else if path.is_none() {
            path = Some(PathBuf::from(argument));
        }
    }

    let mut application = NativeApplication::new(path)?;
    if let Some(query) = query {
        application.open_find_with(&query);
    }
    application.run()
}
