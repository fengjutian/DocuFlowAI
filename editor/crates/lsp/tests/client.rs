//! The client against a server that is not real.
//!
//! The fake server answers the handshake, keeps the document it is told
//! about and publishes what is wrong with it. These tests are about the
//! wire: the frames, the order of the messages and the text that arrives.

use std::path::Path;
use std::time::{Duration, Instant};

use codeweaver_lsp::protocol::{Position, Range, TextChange};
use codeweaver_lsp::{Client, LspEvent, ServerConfig};

/// Starts the fake server.
fn client() -> Client {
    let config = ServerConfig::new(env!("CARGO_BIN_EXE_fake-lsp-server"), Vec::new());
    let client = Client::start(&config, Path::new(".")).expect("the server starts");
    wait_for(|| client.is_ready().then_some(()));
    client
}

/// Waits for something to happen, and gives up after a while.
fn wait_for<T>(mut action: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = action() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "the language server did not answer in time"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The diagnostics the server published, once there are any.
fn diagnostics(client: &Client) -> Vec<codeweaver_lsp::protocol::Diagnostic> {
    let mut found = Vec::new();
    wait_for(|| {
        for event in client.drain_events() {
            if let LspEvent::Diagnostics { diagnostics, .. } = event {
                found = diagnostics;
            }
        }
        (!found.is_empty()).then_some(found.clone())
    })
}

#[test]
fn the_client_says_hello_and_the_server_answers() {
    let client = client();
    assert!(client.is_ready());
    assert!(client.is_running());
}

#[test]
fn an_opened_document_produces_diagnostics() {
    let client = client();
    client
        .did_open("file:///tmp/main.rs", "rust", 1, "fn main() {}\n")
        .expect("the document opens");

    let found = diagnostics(&client);
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].severity,
        Some(codeweaver_lsp::protocol::DiagnosticSeverity::Error)
    );
    assert!(
        found[0].message.contains("13 bytes"),
        "{}",
        found[0].message
    );
}

#[test]
fn a_change_of_one_place_reaches_the_server() {
    let client = client();
    client
        .did_open("file:///tmp/main.rs", "rust", 1, "fn main() {}\n")
        .expect("the document opens");
    let _ = diagnostics(&client);

    // The range is the one of the text *before* the change: the word "main"
    // is replaced by "run".
    client
        .did_change(
            "file:///tmp/main.rs",
            2,
            "fn run() {}\n",
            &[TextChange::in_range(
                Range {
                    start: Position::new(0, 3),
                    end: Position::new(0, 7),
                },
                "run",
            )],
        )
        .expect("the change goes through");

    let found = wait_for(|| {
        let mut found = Vec::new();
        for event in client.drain_events() {
            if let LspEvent::Diagnostics { diagnostics, .. } = event {
                found = diagnostics;
            }
        }
        found
            .iter()
            .any(|diagnostic| diagnostic.message.contains("12 bytes"))
            .then_some(found)
    });
    assert!(
        found[0].message.contains("12 bytes"),
        "{}",
        found[0].message
    );
}

#[test]
fn the_completions_of_the_server_arrive() {
    let client = client();
    client
        .did_open("file:///tmp/main.rs", "rust", 1, "fn main() {}\n")
        .expect("the document opens");

    client
        .request_completion("file:///tmp/main.rs", Position::new(0, 3))
        .expect("the request goes through");

    let list = wait_for(|| {
        let mut lists = client.drain_completions();
        (!lists.is_empty()).then(|| lists.remove(0))
    });
    assert_eq!(list.items.len(), 2);
    assert_eq!(list.items[0].label, "alpha");
    assert_eq!(list.items[0].insert_text(), "alpha");
    assert_eq!(list.items[1].label, "beta");
}

#[test]
fn the_code_actions_of_the_server_arrive() {
    let client = client();
    client
        .did_open("file:///tmp/main.rs", "rust", 1, "fn main() {}\n")
        .expect("the document opens");

    client
        .request_code_action(
            "file:///tmp/main.rs",
            Range {
                start: Position::new(0, 0),
                end: Position::new(0, 12),
            },
            &[],
        )
        .expect("the request goes through");

    let answer = wait_for(|| {
        let mut answers = client.drain_code_actions();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let actions = answer.expect("the server answered");
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].title, "Rename the function");
    // The first action brings edits of its own.
    let edit = actions[0].edit.as_ref().expect("the action has an edit");
    let edits = edit.edits_of("file:///tmp/main.rs");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "run");
    // The second one runs a command and has no edit.
    assert!(actions[1].edit.is_none());
    let command = actions[1].command.as_ref().expect("the action has a command");
    assert_eq!(command.command, "cargo.test");
}

#[test]
fn the_edits_of_a_rename_request_arrive() {
    let client = client();
    client
        .did_open("file:///tmp/main.rs", "rust", 1, "fn main() {}\n")
        .expect("the document opens");

    client
        .request_rename("file:///tmp/main.rs", Position::new(0, 3), "run")
        .expect("the request goes through");

    let answer = wait_for(|| {
        let mut answers = client.drain_renames();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let edit = answer.expect("the server answered");
    // The rename reaches more than the file it was asked about.
    assert_eq!(edit.files().len(), 2);
    let edits = edit.edits_of("file:///tmp/main.rs");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "run");
    assert_eq!(edits[0].range.start, Position::new(0, 3));
    assert_eq!(edits[0].range.end, Position::new(0, 7));
    let others = edit.edits_of("file:///tmp/other.rs");
    assert_eq!(others.len(), 1);
    assert_eq!(others[0].new_text, "run");
}

#[test]
fn the_edits_of_a_formatting_request_arrive() {
    let client = client();
    client
        .did_open("file:///tmp/main.rs", "rust", 1, "fn main() {}\n")
        .expect("the document opens");

    client
        .request_formatting("file:///tmp/main.rs", 4, true)
        .expect("the request goes through");

    let answer = wait_for(|| {
        let mut answers = client.drain_formattings();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let edits = answer.expect("the server answered");
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].new_text, ") {");
    assert_eq!(edits[0].range.start, Position::new(0, 8));
    assert_eq!(edits[0].range.end, Position::new(0, 9));
    assert_eq!(edits[1].new_text, "\n");
    assert_eq!(edits[1].range.start, Position::new(0, 12));
}
