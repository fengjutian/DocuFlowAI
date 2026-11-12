use std::path::PathBuf;

use anyhow::Result;
use codeweaver_lsp::ServerConfig;
use codeweaver_platform::NativeApplication;

/// CodeWeaver's editor.
///
/// ```text
/// codeweaver-editor [file] [--find <text>] [--lsp <program>] [--complete]
/// ```
///
/// With a path, the file is opened into the buffer; without one, the editor
/// starts on an empty document. With `--find`, the editor starts with the
/// find bar open and the text already searched for. With `--lsp`, the
/// language server is the program that is named, instead of the one that
/// belongs to the language of the file. With `--complete`, the editor asks
/// the language server what could come next, and shows the answer.
fn main() -> Result<()> {
    let mut path: Option<PathBuf> = None;
    let mut query: Option<String> = None;
    let mut server: Option<ServerConfig> = None;
    let mut complete = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--find" {
            query = args.next().and_then(|value| value.into_string().ok());
        } else if argument == "--lsp" {
            let program = args.next().and_then(|value| value.into_string().ok());
            server = program.map(|program| ServerConfig::new(program, Vec::new()));
        } else if argument == "--complete" {
            complete = true;
        } else if path.is_none() {
            path = Some(PathBuf::from(argument));
        }
    }

    let mut application = match server {
        Some(server) => NativeApplication::with_language_server(path, server)?,
        None => NativeApplication::new(path)?,
    };
    if let Some(query) = query {
        application.open_find_with(&query);
    }
    if complete {
        application.open_completion();
    }
    application.run()
}
