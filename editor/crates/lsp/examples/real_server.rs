//! The client against a real language server.
//!
//! This is not a test but a probe: it starts the server of the machine,
//! opens a file, and says what the server answered. Run it with the path of
//! a file that belongs to a crate:
//!
//! ```text
//! cargo run -p codeweaver-lsp --example real_server -- crates/lsp/src/lib.rs
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use codeweaver_lsp::protocol::Position;
use codeweaver_lsp::{Client, LspEvent, ServerConfig};

fn main() {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("crates/lsp/src/lib.rs"));
    // The path is made absolute by hand: the canonical one on Windows begins
    // with `\\?\`, which the server does not understand.
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .expect("the working directory is there")
            .join(path)
    };
    let root = path
        .parent()
        .and_then(|parent| parent.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    println!("opening {} in {}", path.display(), root.display());
    let program = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "rust-analyzer".to_owned());
    let config = ServerConfig::new(program, Vec::new());
    let client = Client::start(&config, &root).expect("the server starts");

    let deadline = Instant::now() + Duration::from_secs(120);
    while !client.is_ready() {
        assert!(
            Instant::now() < deadline,
            "the server did not say hello:\n{}",
            client.stderr_tail()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("the server is ready");

    let uri = codeweaver_lsp::protocol::file_uri(&path);
    let text = std::fs::read_to_string(&path).expect("the file is read");
    client
        .did_open(&uri, "rust", 1, &text)
        .expect("the document opens");

    // The diagnostics of the file the server was given.
    let mut diagnostics = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    while diagnostics.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the server did not publish any diagnostics"
        );
        for event in client.drain_events() {
            match event {
                LspEvent::Diagnostics { uri, diagnostics: found } => {
                    println!("diagnostics for {uri}: {} of them", found.len());
                    for diagnostic in &found {
                        println!(
                            "  {:?} {:?} {}",
                            diagnostic.severity, diagnostic.range, diagnostic.message
                        );
                    }
                    diagnostics = found;
                }
                LspEvent::Disconnected { reason } => {
                    panic!("the server went away: {reason}");
                }
                LspEvent::Message { text } => println!("the server says: {text}"),
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // The completions at the first place of the file.
    client
        .request_completion(&uri, Position::new(0, 0))
        .expect("the request goes through");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "the server did not answer");
        let lists = client.drain_completions();
        if let Some(list) = lists.first() {
            println!("completions: {} of them", list.items.len());
            for item in list.items.iter().take(10) {
                println!("  {} {:?}", item.label, item.detail);
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    println!("the sync kind is {:?}", client.sync_kind());
}
