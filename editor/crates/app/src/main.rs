use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;
use codeweaver_dap::AdapterConfig;
use codeweaver_lsp::ServerConfig;
use codeweaver_platform::{dev_log_path, HighlightPolicy, NativeApplication};

/// CodeWeaver's editor.
///
/// ```text
/// codeweaver-editor [file] [--find <text>] [--lsp <program>] [--complete]
///                    [--highlight-limit <bytes>] [--highlight-lines <lines>]
///                    [--dap <program>] [--dap-arg <arg>] [--dev]
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
///
/// With `--dev`, the editor is in development mode: what it says about
/// itself is written into `.dev-logs/editor.log` in the folder the editor
/// was started in, and `Ctrl+Shift+D` opens the developer panel, which shows
/// that log, the commands of the editor and the state of the language
/// server.
fn main() -> Result<()> {
    let options = Options::parse(std::env::args_os().skip(1));

    let mut application = build_application(&options)?;
    // A window that is opened without a file is the window of the last run:
    // the files, the columns and the workspace are put back the way they
    // were. A file on the command line is the person saying what to open, so
    // the session stays out of it — but the window it makes is the one the
    // next run gets back.
    if options.path.is_none() {
        application.resume_the_session();
    } else {
        application.record_the_session();
    }
    if let Some(program) = options.adapter.clone() {
        application.set_debug_adapter(AdapterConfig::new(program, options.adapter_args.clone()));
    }
    if options.limit_bytes.is_some() || options.limit_lines.is_some() {
        let mut policy = HighlightPolicy::default();
        if let Some(bytes) = options.limit_bytes {
            policy.max_bytes = bytes;
        }
        if let Some(lines) = options.limit_lines {
            policy.max_lines = lines;
        }
        application.set_highlight_policy(policy);
    }
    if let Some(query) = options.query.clone() {
        application.open_find_with(&query);
    }
    if options.complete {
        application.open_completion();
    }
    application.run()
}

/// What the editor was asked to do on the command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Options {
    /// The file that is opened.
    path: Option<PathBuf>,
    /// The text the find bar starts with.
    query: Option<String>,
    /// The language server to use, when one is named.
    server: Option<String>,
    /// The debug adapter to use, when one is named.
    adapter: Option<String>,
    /// The arguments of the debug adapter.
    adapter_args: Vec<String>,
    /// Whether the editor asks the language server what could come next.
    complete: bool,
    /// Whether the editor is in development mode.
    dev: bool,
    /// The size at which the highlighter gives up, in bytes.
    limit_bytes: Option<usize>,
    /// The number of lines at which the highlighter gives up.
    limit_lines: Option<usize>,
}

impl Options {
    /// Reads the options from the arguments of the command line.
    ///
    /// A word that is not a known option is the file to open. An option that
    /// needs a value but has none is **said to be wrong**: the editor starts
    /// without it and says so, rather than quietly doing something else.
    fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Self {
        let mut options = Options::default();
        let mut args = args.into_iter();
        while let Some(argument) = args.next() {
            let word = argument.to_string_lossy().to_string();
            match word.as_str() {
                "--find" => {
                    options.query = args.next().and_then(|value| value.into_string().ok());
                }
                "--lsp" => {
                    options.server = args.next().and_then(|value| value.into_string().ok());
                }
                "--dap" => {
                    options.adapter = args.next().and_then(|value| value.into_string().ok());
                }
                "--dap-arg" => {
                    if let Some(value) = args.next().and_then(|value| value.into_string().ok()) {
                        options.adapter_args.push(value);
                    }
                }
                "--complete" => options.complete = true,
                "--dev" => options.dev = true,
                "--highlight-limit" => {
                    options.limit_bytes = args
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<usize>().ok());
                }
                "--highlight-lines" => {
                    options.limit_lines = args
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<usize>().ok());
                }
                _ => {
                    if options.path.is_none() {
                        options.path = Some(PathBuf::from(argument));
                    }
                }
            }
        }
        options
    }
}

/// Builds the editor the options ask for.
fn build_application(options: &Options) -> Result<NativeApplication> {
    let server = options
        .server
        .as_ref()
        .map(|program| ServerConfig::new(program.clone(), Vec::new()));
    Ok(match (options.dev, server) {
        (true, Some(server)) => NativeApplication::in_development_mode_with_language_server(
            options.path.clone(),
            server,
            dev_log_path(),
        )?,
        (true, None) => {
            NativeApplication::in_development_mode(options.path.clone(), dev_log_path())?
        }
        (false, Some(server)) => NativeApplication::with_language_server(options.path.clone(), server)?,
        (false, None) => NativeApplication::new(options.path.clone())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Options {
        Options::parse(words.iter().map(OsString::from))
    }

    #[test]
    fn a_file_is_opened_without_any_option() {
        let options = parse(&["main.rs"]);
        assert_eq!(options.path, Some(PathBuf::from("main.rs")));
        assert!(!options.dev);
    }

    #[test]
    fn the_development_mode_is_asked_for_with_dev() {
        assert!(parse(&["--dev"]).dev);
        assert!(parse(&["--dev", "main.rs"]).dev);
        assert!(!parse(&["main.rs"]).dev);
    }

    #[test]
    fn the_development_mode_reaches_the_editor() {
        let options = parse(&["--dev"]);
        let application = build_application(&options).expect("the editor starts");
        assert!(application.is_development_mode(), "the editor knows the mode");
    }

    #[test]
    fn an_editor_without_the_mode_is_not_in_development_mode() {
        let options = parse(&[]);
        let application = build_application(&options).expect("the editor starts");
        assert!(!application.is_development_mode());
    }

    #[test]
    fn the_language_server_is_named_on_the_command_line() {
        assert_eq!(
            parse(&["--lsp", "rust-analyzer.exe"]).server.as_deref(),
            Some("rust-analyzer.exe")
        );
    }

    #[test]
    fn the_debug_adapter_and_its_arguments_are_named() {
        let options = parse(&["--dap", "fake.py", "--dap-arg", "--one"]);
        assert_eq!(options.adapter.as_deref(), Some("fake.py"));
        assert_eq!(options.adapter_args, vec!["--one".to_owned()]);
    }

    #[test]
    fn the_sizes_of_the_highlighter_are_named() {
        let options = parse(&["--highlight-limit", "1024", "--highlight-lines", "20"]);
        assert_eq!(options.limit_bytes, Some(1024));
        assert_eq!(options.limit_lines, Some(20));
    }

    #[test]
    fn the_find_bar_starts_with_the_text_that_was_asked_for() {
        let options = parse(&["--find", "needle"]);
        assert_eq!(options.query.as_deref(), Some("needle"));
        assert!(parse(&["--complete"]).complete);
    }

    #[test]
    fn an_option_without_its_value_leaves_it_empty() {
        let options = parse(&["--find"]);
        assert_eq!(options.query, None);
    }
}
