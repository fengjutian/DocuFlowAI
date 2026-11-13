//! A debug adapter that is not real, for the tests of the client.
//!
//! It speaks the protocol over its standard input and output, keeps the
//! breakpoints it is told about, and answers with what a test needs:
//!
//! - `initialize` says what the adapter can do, and then that it is ready;
//! - `setBreakpoints` answers with the breakpoints that were asked for;
//! - `launch` starts the program, says what it printed and that it stopped;
//! - `stackTrace` answers with three frames, `scopes` with one group of
//!   variables, and `variables` with two variables; every frame has
//!   variables of its own, so that the frame the editor asks about can be
//!   told from the answer;
//! - `next`, `stepIn` and `stepOut` stop the program again, `continue` lets
//!   it run on, and `terminate` ends the session.

use std::collections::BTreeMap;
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
    /// The condition of every breakpoint, by the line it is on.
    ///
    /// The adapter does not work the condition out: it only says what it was
    /// told, so that the editor can be checked against the protocol.
    conditions: BTreeMap<u32, String>,
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
                self.conditions = breakpoint_conditions(message);
                let breakpoints: Vec<Value> = self
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(index, line)| {
                        let mut breakpoint = json!({ "id": index + 1, "verified": true, "line": line });
                        // The condition is said back, so that the editor can
                        // be checked against the protocol.
                        if let Some(condition) = self.conditions.get(line) {
                            breakpoint["message"] = json!(format!("条件：{condition}"));
                        }
                        breakpoint
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
                // The stack has more than one frame: the editor shows the
                // whole stack, and the person may pick any frame of it.
                //
                // The second frame is in another file of the same project:
                // the editor has to be able to open the file of a frame.
                let caller = caller_path(&path);
                let frames = [
                    (1, "main", line, path.clone()),
                    (2, "helper", line.saturating_sub(1).max(1), caller),
                    (3, "outer", line.saturating_sub(2).max(1), path.clone()),
                ];
                let stack: Vec<Value> = frames
                    .iter()
                    .map(|(id, name, line, path)| {
                        json!({
                            "id": id,
                            "name": name,
                            "line": line,
                            "column": 1,
                            "source": { "path": path },
                        })
                    })
                    .collect();
                let body = json!({
                    "stackFrames": stack,
                    "totalFrames": frames.len(),
                });
                self.answer(output, seq, body);
            }
            "scopes" => {
                // The number of the group of variables says which frame the
                // variables belong to: the editor asks for the group of the
                // frame it is showing.
                let frame_id = message
                    .get("arguments")
                    .and_then(|arguments| arguments.get("frameId"))
                    .and_then(Value::as_i64)
                    .unwrap_or(1);
                let body = json!({
                    "scopes": [{
                        "name": "Locals",
                        "variablesReference": 100 + frame_id,
                        "expensive": false,
                    }],
                });
                self.answer(output, seq, body);
            }
            "variables" => {
                // Every frame has variables of its own: the number of the
                // group says which frame they belong to.
                let reference = message
                    .get("arguments")
                    .and_then(|arguments| arguments.get("variablesReference"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let (first, second) = match reference {
                    102 => ("total", "6"),
                    103 => ("depth", "2"),
                    _ => ("count", "3"),
                };
                let body = json!({
                    "variables": [
                        {
                            "name": first,
                            "value": second,
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
            "evaluate" => {
                // The adapter does not really work the expression out: it
                // says a worth that is always the same. What it does look
                // at is the expression: an empty one is refused, so that the
                // contract can be checked — the editor has to send the
                // expression the person wrote.
                let expression = message
                    .get("arguments")
                    .and_then(|arguments| arguments.get("expression"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if expression.trim().is_empty() {
                    self.failure(output, seq, "the expression is empty");
                    return;
                }
                let body = json!({
                    "result": "42",
                    "type": "i32",
                    "variablesReference": 0,
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

/// The condition of every breakpoint of a request, by the line it is on.
fn breakpoint_conditions(message: &Value) -> BTreeMap<u32, String> {
    let Some(arguments) = message.get("arguments") else {
        return BTreeMap::new();
    };
    arguments
        .get("breakpoints")
        .and_then(Value::as_array)
        .map(|breakpoints| {
            breakpoints
                .iter()
                .filter_map(|breakpoint| {
                    let line = breakpoint.get("line").and_then(Value::as_u64)? as u32;
                    let condition = breakpoint
                        .get("condition")
                        .and_then(Value::as_str)?
                        .to_owned();
                    Some((line, condition))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The file of the caller of a frame.
///
/// The caller is in another file of the same project: the editor has to be
/// able to open the file of a frame, and a file that is not there says
/// nothing about the stack.
fn caller_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/helper.rs"),
        None => "helper.rs".to_owned(),
    }
}

/// Writes one message to the output.
fn write(output: &mut impl Write, message: &Value) {
    let Ok(text) = serde_json::to_string(message) else {
        return;
    };
    let _ = output.write_all(&serialize(&text));
    let _ = output.flush();
}
