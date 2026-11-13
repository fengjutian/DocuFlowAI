//! A debug adapter that is not real, for the tests of the client.
//!
//! It speaks the protocol over its standard input and output, keeps the
//! breakpoints it is told about, and answers with what a test needs:
//!
//! - `initialize` says what the adapter can do, and then that it is ready;
//! - `setBreakpoints` answers with the breakpoints that were asked for;
//! - `launch` starts the program, says what it printed and that it stopped;
//! - `stackTrace` answers with one frame, `scopes` with one group of
//!   variables, and `variables` with two variables;
//! - `next`, `stepIn` and `stepOut` stop the program again, `continue` lets
//!   it run on, and `terminate` ends the session.

use std::io::{Read, Write};

use codeweaver_dap::framing::{FrameReader, serialize};
use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout();
    let mut frames = FrameReader::new();
    let mut chunk = [0u8; 8192];
    let mut adapter = Adapter::default();

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
            adapter.handle(&message, &mut output);
        }
    }
}

/// The adapter, and what it knows about the program that is being debugged.
#[derive(Default)]
struct Adapter {
    /// The number of the next message the adapter sends.
    seq: i64,
    /// The file the breakpoints are in.
    path: Option<String>,
    /// The lines the breakpoints are on.
    lines: Vec<u32>,
}

impl Adapter {
    /// Answers one message of the editor.
    fn handle(&mut self, message: &Value, output: &mut impl Write) {
        let command = message
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if command.is_empty() {
            // Not a request of the editor: nothing to answer.
            return;
        }
        let seq = message.get("seq").and_then(Value::as_i64).unwrap_or_default();

        match command {
            "initialize" => {
                self.answer(
                    output,
                    seq,
                    json!({ "capabilities": { "supportsConfigurationDoneRequest": true } }),
                );
                // The adapter is ready for the breakpoints.
                self.event(output, "initialized", json!({}));
            }
            "setBreakpoints" => {
                // The adapter keeps the breakpoints, so that it knows where
                // the program is to stop.
                self.path = source_path(message);
                self.lines = breakpoint_lines(message);
                let breakpoints: Vec<Value> = self
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(index, line)| {
                        json!({ "id": index + 1, "verified": true, "line": line })
                    })
                    .collect();
                let body = json!({ "breakpoints": breakpoints });
                self.answer(output, seq, body);
            }
            "configurationDone" => {
                self.answer(output, seq, json!({}));
            }
            "launch" => {
                self.answer(output, seq, json!({}));
                self.event(
                    output,
                    "output",
                    json!({ "category": "stdout", "output": "the program started\n" }),
                );
                self.event(
                    output,
                    "stopped",
                    json!({
                        "reason": "breakpoint",
                        "threadId": 1,
                        "allThreadsStopped": true,
                    }),
                );
            }
            "stackTrace" => {
                // The program stopped at the first breakpoint that was set.
                let line = self.lines.first().copied().unwrap_or(1);
                let path = self.path.clone().unwrap_or_else(|| "main.rs".to_owned());
                let body = json!({
                    "stackFrames": [{
                        "id": 1,
                        "name": "main",
                        "line": line,
                        "column": 1,
                        "source": { "path": path },
                    }],
                    "totalFrames": 1,
                });
                self.answer(output, seq, body);
            }
            "scopes" => {
                let body = json!({
                    "scopes": [{
                        "name": "Locals",
                        "variablesReference": 10,
                        "expensive": false,
                    }],
                });
                self.answer(output, seq, body);
            }
            "variables" => {
                let body = json!({
                    "variables": [
                        {
                            "name": "count",
                            "value": "3",
                            "type": "i32",
                            "variablesReference": 0,
                        },
                        {
                            "name": "name",
                            "value": "greeting",
                            "type": "&str",
                            "variablesReference": 0,
                        },
                    ],
                });
                self.answer(output, seq, body);
            }
            "next" | "stepIn" | "stepOut" => {
                self.answer(output, seq, json!({}));
                self.event(
                    output,
                    "stopped",
                    json!({
                        "reason": "step",
                        "threadId": 1,
                        "allThreadsStopped": true,
                    }),
                );
            }
            "continue" => {
                self.answer(output, seq, json!({}));
                self.event(output, "continued", json!({ "threadId": 1 }));
            }
            "terminate" => {
                self.answer(output, seq, json!({}));
                self.event(output, "terminated", json!({}));
            }
            _ => {
                // Every other request fails, and says why.
                self.failure(
                    output,
                    seq,
                    &format!("the adapter does not know the command {command}"),
                );
            }
        }
    }

    /// Answers a request of the editor.
    fn answer(&mut self, output: &mut impl Write, request_seq: i64, body: Value) {
        let message = json!({
            "seq": self.next_seq(),
            "type": "response",
            "request_seq": request_seq,
            "success": true,
            "body": body,
        });
        write(output, &message);
    }

    /// Says that a request of the editor could not be done.
    fn failure(&mut self, output: &mut impl Write, request_seq: i64, message: &str) {
        let message = json!({
            "seq": self.next_seq(),
            "type": "response",
            "request_seq": request_seq,
            "success": false,
            "message": message,
        });
        write(output, &message);
    }

    /// Says that something happened.
    fn event(&mut self, output: &mut impl Write, name: &str, body: Value) {
        let message = json!({
            "seq": self.next_seq(),
            "type": "event",
            "event": name,
            "body": body,
        });
        write(output, &message);
    }

    /// The number of the next message the adapter sends.
    fn next_seq(&mut self) -> i64 {
        self.seq += 1;
        self.seq
    }
}

/// The file the breakpoints of a request are in.
fn source_path(message: &Value) -> Option<String> {
    message
        .get("arguments")
        .and_then(|arguments| arguments.get("source"))
        .and_then(|source| source.get("path"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The lines the breakpoints of a request are on.
fn breakpoint_lines(message: &Value) -> Vec<u32> {
    let Some(arguments) = message.get("arguments") else {
        return Vec::new();
    };
    let lines: Vec<u32> = arguments
        .get("breakpoints")
        .and_then(Value::as_array)
        .map(|breakpoints| {
            breakpoints
                .iter()
                .filter_map(|breakpoint| breakpoint.get("line").and_then(Value::as_u64))
                .map(|line| line as u32)
                .collect()
        })
        .unwrap_or_default();
    if !lines.is_empty() {
        return lines;
    }
    // The editor may send the lines on their own.
    arguments
        .get("lines")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(Value::as_u64)
                .map(|line| line as u32)
                .collect()
        })
        .unwrap_or_default()
}

/// Writes one message to the output.
fn write(output: &mut impl Write, message: &Value) {
    let Ok(text) = serde_json::to_string(message) else {
        return;
    };
    let _ = output.write_all(&serialize(&text));
    let _ = output.flush();
}
