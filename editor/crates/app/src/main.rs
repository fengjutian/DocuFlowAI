use std::path::PathBuf;

use anyhow::Result;
use codeweaver_dap::AdapterConfig;
use codeweaver_lsp::ServerConfig;
use codeweaver_platform::{HighlightPolicy, NativeApplication};

/// CodeWeaver's editor.
///
/// ```text
/// codeweaver-editor [file] [--find <text>] [--lsp <program>] [--complete]
///                    [--highlight-limit <bytes>] [--highlight-lines <lines>]
///                    [--dap <program>] [--dap-arg <arg>]
/// ```
///
/// With a path, the file is opened into the buffer; without one, the editor
/// starts on an empty document. With `--find`, the editor starts with the
/// find bar open and the text already searched for. With `--lsp`, the
/// language server is the program that is named, instead of the one that
/// belongs to the language of the file. With `--complete`, the editor asks
/// the language server what could come next, and shows the answer.
///
/// With `--dap`, the debug adapter is the program that is named: `F5` starts
/// a debug session with it, and `--dap-arg` says one of its arguments.
///
/// With `--highlight-limit` and `--highlight-lines`, the sizes at which the
/// editor stops colouring a file are set. A file that is over the limit is
/// still edited and saved; it only loses its colours.
fn main() -> Result<()> {
    let mut path: Option<PathBuf> = None;
    let mut query: Option<String> = None;
    let mut server: Option<ServerConfig> = None;
    let mut adapter: Option<String> = None;
    let mut adapter_args: Vec<String> = Vec::new();
    let mut complete = false;
    let mut limit_bytes: Option<usize> = None;
    let mut limit_lines: Option<usize> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--find" {
            query = args.next().and_then(|value| value.into_string().ok());
        } else if argument == "--lsp" {
            let program = args.next().and_then(|value| value.into_string().ok());
            server = program.map(|program| ServerConfig::new(program, Vec::new()));
        } else if argument == "--dap" {
            adapter = args.next().and_then(|value| value.into_string().ok());
        } else if argument == "--dap-arg" {
            if let Some(value) = args.next().and_then(|value| value.into_string().ok()) {
                adapter_args.push(value);
            }
        } else if argument == "--complete" {
            complete = true;
        } else if argument == "--highlight-limit" {
            limit_bytes = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<usize>().ok());
        } else if argument == "--highlight-lines" {
            limit_lines = args
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<usize>().ok());
        } else if path.is_none() {
            path = Some(PathBuf::from(argument));
        }
    }

    let mut application = match server {
        Some(server) => NativeApplication::with_language_server(path, server)?,
        None => NativeApplication::new(path)?,
    };
    if let Some(program) = adapter {
        application.set_debug_adapter(AdapterConfig::new(program, adapter_args));
    }
    if limit_bytes.is_some() || limit_lines.is_some() {
        let mut policy = HighlightPolicy::default();
        if let Some(bytes) = limit_bytes {
            policy.max_bytes = bytes;
        }
        if let Some(lines) = limit_lines {
            policy.max_lines = lines;
        }
        application.set_highlight_policy(policy);
    }
    if let Some(query) = query {
        application.open_find_with(&query);
    }
    if complete {
        application.open_completion();
    }
    application.run()
}
