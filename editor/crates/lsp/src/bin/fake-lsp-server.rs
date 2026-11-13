//! A language server that is not real, for the tests of the client.
//!
//! It speaks the protocol over its standard input and output, keeps the
//! document it is told about, and answers with what a test needs:
//!
//! - `initialize` says the server wants the ranges that changed;
//! - `textDocument/didOpen` and `textDocument/didChange` publish a
//!   diagnostic that says how much text the server has;
//! - `textDocument/completion` answers with two items;
//! - `textDocument/formatting` answers with two edits of the document.

use std::collections::HashMap;
use std::io::{Read, Write};

use codeweaver_lsp::framing::{FrameReader, serialize};
use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout();
    let mut frames = FrameReader::new();
    let mut chunk = [0u8; 8192];
    // The documents the server knows about, by URI.
    let mut documents: HashMap<String, String> = HashMap::new();

    loop {
        let read = match input.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        for frame in frames.push(&chunk[..read]) {
            let Ok(message) = serde_json::from_str::<Value>(&frame) else {
                continue;
            };
            handle(&message, &mut output, &mut documents);
        }
    }
}

/// The document the server has, and the URI it belongs to.
struct Document {
    uri: String,
    text: String,
}

/// Answers one message.
fn handle(message: &Value, output: &mut impl Write, documents: &mut HashMap<String, String>) {
    let method = message
        .get("method")
        .and_then(|method| method.as_str())
        .unwrap_or_default();
    let id = message.get("id").and_then(|id| id.as_i64());

    match (id, method) {
        (Some(id), "initialize") => {
            let answer = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "capabilities": {
                        "textDocumentSync": 2,
                        "completionProvider": { "triggerCharacters": ["."] }
                    }
                }
            });
            write(output, &answer);
        }
        (Some(id), "textDocument/completion") => {
            let answer = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "isIncomplete": false,
                    "items": [
                        { "label": "alpha", "kind": 3, "detail": "fn alpha()" },
                        { "label": "beta", "kind": 3, "detail": "fn beta()" }
                    ]
                }
            });
            write(output, &answer);
        }
        (Some(id), "textDocument/formatting") => {
            // Two edits, the way a formatter answers: one that replaces a
            // piece of text and one that puts something in.
            let answer = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": [
                    {
                        "range": {
                            "start": { "line": 0, "character": 8 },
                            "end": { "line": 0, "character": 9 }
                        },
                        "newText": ") {"
                    },
                    {
                        "range": {
                            "start": { "line": 0, "character": 12 },
                            "end": { "line": 0, "character": 12 }
                        },
                        "newText": "\n"
                    }
                ]
            });
            write(output, &answer);
        }
        (Some(id), "textDocument/rename") => {
            // A rename says which files change: the file that was asked
            // about and another one, the way the rename of a function
            // reaches the file that calls it.
            let answer = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "changes": {
                        "file:///tmp/main.rs": [
                            {
                                "range": {
                                    "start": { "line": 0, "character": 3 },
                                    "end": { "line": 0, "character": 7 }
                                },
                                "newText": "run"
                            }
                        ],
                        "file:///tmp/other.rs": [
                            {
                                "range": {
                                    "start": { "line": 0, "character": 0 },
                                    "end": { "line": 0, "character": 4 }
                                },
                                "newText": "run"
                            }
                        ]
                    }
                }
            });
            write(output, &answer);
        }
        (Some(id), "textDocument/codeAction") => {
            // Two actions: one that changes the text of the document and
            // one that runs a command.
            let answer = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": [
                    {
                        "title": "Rename the function",
                        "kind": "quickfix",
                        "edit": {
                            "changes": {
                                "file:///tmp/main.rs": [
                                    {
                                        "range": {
                                            "start": { "line": 0, "character": 3 },
                                            "end": { "line": 0, "character": 7 }
                                        },
                                        "newText": "run"
                                    }
                                ]
                            }
                        }
                    },
                    {
                        "title": "Run the tests",
                        "command": "cargo.test"
                    }
                ]
            });
            write(output, &answer);
        }
        (Some(id), _) => {
            // Every other request gets an empty answer, so that the server
            // carries on with what it was doing.
            let answer = json!({ "jsonrpc": "2.0", "id": id, "result": null });
            write(output, &answer);
        }
        (None, "textDocument/didOpen") => {
            let uri = document_uri(message);
            let text = message
                .get("params")
                .and_then(|params| params.get("textDocument"))
                .and_then(|document| document.get("text"))
                .and_then(|text| text.as_str())
                .unwrap_or_default()
                .to_owned();
            documents.insert(uri.clone(), text.clone());
            publish(output, &Document { uri, text });
        }
        (None, "textDocument/didChange") => {
            let uri = document_uri(message);
            let mut text = documents.get(&uri).cloned().unwrap_or_default();
            if let Some(changes) = message
                .get("params")
                .and_then(|params| params.get("contentChanges"))
                .and_then(|changes| changes.as_array())
            {
                for change in changes {
                    let new = change
                        .get("text")
                        .and_then(|text| text.as_str())
                        .unwrap_or_default();
                    match change.get("range") {
                        Some(range) => {
                            let start = offset_of(&text, range.get("start"));
                            let end = offset_of(&text, range.get("end"));
                            text.replace_range(start..end, new);
                        }
                        None => text = new.to_owned(),
                    }
                }
            }
            documents.insert(uri.clone(), text.clone());
            publish(output, &Document { uri, text });
        }
        _ => {}
    }
}

/// The URI of the document a message is about.
fn document_uri(message: &Value) -> String {
    message
        .get("params")
        .and_then(|params| params.get("textDocument"))
        .and_then(|document| document.get("uri"))
        .and_then(|uri| uri.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// The byte offset of a place in the text.
///
/// The protocol counts columns in UTF-16 code units; the text of the tests
/// is plain ASCII, so the two are the same here.
fn offset_of(text: &str, position: Option<&Value>) -> usize {
    let Some(position) = position else {
        return 0;
    };
    let line = position
        .get("line")
        .and_then(|line| line.as_u64())
        .unwrap_or(0) as usize;
    let character = position
        .get("character")
        .and_then(|character| character.as_u64())
        .unwrap_or(0) as usize;
    let mut offset = 0;
    for _ in 0..line {
        let Some(rest) = text.get(offset..) else {
            return text.len();
        };
        match rest.find('\n') {
            Some(index) => offset += index + 1,
            None => return text.len(),
        }
    }
    offset + character.min(text.get(offset..).map(str::len).unwrap_or(0))
}

/// Tells the client what is wrong with the document.
///
/// The message says how much text the server has, so that a test can see
/// that the changes arrived the way they were meant to.
fn publish(output: &mut impl Write, document: &Document) {
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": document.uri,
            "diagnostics": [{
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 3 }
                },
                "severity": 1,
                "message": format!("the server has {} bytes", document.text.len())
            }]
        }
    });
    write(output, &notification);
}

/// Writes one message to the output.
fn write(output: &mut impl Write, message: &Value) {
    let Ok(text) = serde_json::to_string(message) else {
        return;
    };
    let _ = output.write_all(&serialize(&text));
    let _ = output.flush();
}
