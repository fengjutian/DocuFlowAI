//! ACP client: spawns the agent endpoint and speaks JSON-RPC to it.
//!
//! The client owns one child process. Requests are written to its stdin, and
//! a background thread reads its stdout, decoding frames and dispatching them:
//!
//! - a response resolves the request that is waiting for it;
//! - a `session/update` notification becomes a [`SessionEvent`];
//! - a `session/request_permission` request becomes a
//!   [`SessionEvent::PermissionRequested`], and the editor answers it later.
//!
//! The editor's event loop never blocks on the agent: it polls
//! [`Client::drain_events`] once per frame and hands the events to the UI.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
};

use serde_json::{Value, json};

use crate::{
    framing::{FrameReader, Framing, serialize},
    protocol::{
        InitializeResult, PROTOCOL_VERSION, PermissionOption, PermissionOutcome, PermissionRequest,
        PromptBlock, PromptCompleted, SessionCreated, SessionUpdate, StopReason, permission_result,
    },
};

/// An event produced by the agent, in the editor's own vocabulary.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    /// A chunk of the agent's reply.
    Message { text: String },
    /// A tool call started or was updated.
    ToolCall {
        tool_call_id: String,
        title: String,
        status: crate::protocol::ToolStatus,
        /// What kind of tool this is.
        kind: String,
        /// The content the tool produced, such as a proposed diff.
        content: Vec<crate::protocol::ToolCallContent>,
    },
    /// The agent is asking for permission.
    PermissionRequested {
        /// The id of the agent's request, used to answer it.
        request_id: crate::protocol::RequestId,
        request: PermissionRequest,
    },
    /// A turn finished.
    TurnCompleted { stop_reason: StopReason },
    /// The agent endpoint failed or went away.
    Disconnected { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("could not start the agent endpoint: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("could not write to the agent endpoint: {0}")]
    Write(#[source] std::io::Error),
    #[error("the agent endpoint closed its output")]
    Closed,
    #[error("the agent endpoint reported an error: {message}")]
    Remote { code: i64, message: String },
    #[error("the agent endpoint sent a malformed message: {0}")]
    Protocol(String),
    #[error("the agent endpoint did not answer in time")]
    Timeout,
}

/// A pending request waiting for its response.
struct Pending {
    sender: Sender<Result<Value, AcpError>>,
}

/// Configuration for the agent endpoint process.
#[derive(Clone, Debug)]
pub struct EndpointConfig {
    /// The program to run.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Environment variables for the child process.
    pub env: Vec<(String, String)>,
}

impl EndpointConfig {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        EndpointConfig {
            program: program.into(),
            args,
            env: Vec::new(),
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// A connection to an ACP agent endpoint.
pub struct Client {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<Mutex<crate::protocol::RequestId>>,
    pending: Arc<Mutex<HashMap<crate::protocol::RequestId, Pending>>>,
    /// The turns started with [`Client::start_prompt`], by request id.
    turns: Arc<Mutex<HashMap<crate::protocol::RequestId, String>>>,
    events: Receiver<SessionEvent>,
    running: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    child: Option<Child>,
}

impl Client {
    /// Starts the agent endpoint and waits for it to be ready.
    pub fn start(config: &EndpointConfig) -> Result<Self, AcpError> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (key, value) in &config.env {
            command.env(key, value);
        }

        let mut child = command.spawn().map_err(AcpError::Spawn)?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        let (event_sender, events) = channel();
        let pending: Arc<Mutex<HashMap<crate::protocol::RequestId, Pending>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let turns: Arc<Mutex<HashMap<crate::protocol::RequestId, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(Mutex::new(1));
        let running = Arc::new(AtomicBool::new(true));

        let reader = {
            let pending = Arc::clone(&pending);
            let turns = Arc::clone(&turns);
            let running = Arc::clone(&running);
            std::thread::spawn(move || read_loop(stdout, pending, turns, event_sender, running))
        };

        let client = Client {
            stdin: Arc::new(Mutex::new(Some(stdin))),
            next_id: Arc::clone(&next_id),
            pending,
            turns,
            events,
            running,
            reader: Some(reader),
            child: Some(child),
        };

        client.initialize()?;
        Ok(client)
    }

    /// Sends `initialize` and returns the agent's capabilities.
    pub fn initialize(&self) -> Result<InitializeResult, AcpError> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {},
            }),
        )?;
        serde_json::from_value(result).map_err(|error| AcpError::Protocol(error.to_string()))
    }

    /// Opens a session rooted at `cwd`.
    pub fn new_session(&self, cwd: &std::path::Path) -> Result<SessionCreated, AcpError> {
        let result = self.request(
            "session/new",
            json!({
                "cwd": cwd.to_string_lossy(),
                "mcpServers": [],
            }),
        )?;
        serde_json::from_value(result).map_err(|error| AcpError::Protocol(error.to_string()))
    }

    /// Starts a turn and returns as soon as the request is written.
    ///
    /// The turn runs in the background. Its output arrives as
    /// [`SessionEvent`]s on the event stream, and it ends with
    /// [`SessionEvent::TurnCompleted`]. The editor never blocks on a turn:
    /// the event loop drains the stream once per frame.
    pub fn start_prompt(&self, session_id: &str, text: &str) -> Result<(), AcpError> {
        self.start_prompt_blocks(session_id, &[PromptBlock::text(text)])
    }

    /// Starts a turn made of several blocks.
    ///
    /// The instruction and the attached material travel together but stay
    /// apart: the endpoint tells the model which is which.
    pub fn start_prompt_blocks(
        &self,
        session_id: &str,
        blocks: &[PromptBlock],
    ) -> Result<(), AcpError> {
        let id = {
            let mut next = self
                .next_id
                .lock()
                .expect("the request id lock is not poisoned");
            let id = *next;
            *next += 1;
            id
        };

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": blocks,
            },
        });
        self.write(&message)?;
        self.turns
            .lock()
            .expect("the turn lock is not poisoned")
            .insert(id, session_id.to_owned());
        Ok(())
    }

    /// Waits for a turn to finish, draining the event stream meanwhile.
    ///
    /// This is the blocking form, for callers that are not the UI thread —
    /// tests and the CLI. The editor uses [`Client::start_prompt`] instead.
    pub fn prompt(&self, session_id: &str, text: &str) -> Result<PromptCompleted, AcpError> {
        let id = {
            let mut next = self
                .next_id
                .lock()
                .expect("the request id lock is not poisoned");
            let id = *next;
            *next += 1;
            id
        };

        let (sender, receiver) = channel();
        self.pending
            .lock()
            .expect("the pending lock is not poisoned")
            .insert(id, Pending { sender });

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [PromptBlock::Text { text: text.to_owned() }],
            },
        });
        if let Err(error) = self.write(&message) {
            self.pending
                .lock()
                .expect("the pending lock is not poisoned")
                .remove(&id);
            return Err(error);
        }

        match receiver.recv() {
            Ok(Ok(result)) => serde_json::from_value(result)
                .map_err(|error| AcpError::Protocol(error.to_string())),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(AcpError::Closed),
        }
    }

    /// Asks the agent to cancel the turn in `session_id`.
    ///
    /// Cancellation is a notification, so it is fire-and-forget: the turn
    /// still ends with `cancelled`, and the endpoint reports whether the
    /// runtime honoured it.
    pub fn cancel(&self, session_id: &str) -> Result<(), AcpError> {
        self.notify(
            "session/cancel",
            json!({
                "sessionId": session_id,
            }),
        )
    }

    /// Answers a permission request.
    pub fn respond_to_permission(
        &self,
        request_id: crate::protocol::RequestId,
        outcome: PermissionOutcome,
    ) -> Result<(), AcpError> {
        self.respond(request_id, permission_result(outcome))
    }

    /// Returns every event that has arrived since the last call.
    pub fn drain_events(&self) -> Vec<SessionEvent> {
        self.events.try_iter().collect()
    }

    /// Sends a request and waits for its response.
    fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let id = {
            let mut next = self
                .next_id
                .lock()
                .expect("the request id lock is not poisoned");
            let id = *next;
            *next += 1;
            id
        };

        let (sender, receiver) = channel();
        self.pending
            .lock()
            .expect("the pending lock is not poisoned")
            .insert(id, Pending { sender });

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write(&message) {
            self.pending
                .lock()
                .expect("the pending lock is not poisoned")
                .remove(&id);
            return Err(error);
        }

        match receiver.recv() {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(error),
            // The reader thread ended without answering.
            Err(_) => Err(AcpError::Closed),
        }
    }

    /// Sends a response to a request the agent made.
    fn respond(&self, id: crate::protocol::RequestId, result: Value) -> Result<(), AcpError> {
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        self.write(&message)
    }

    /// Sends a notification.
    fn notify(&self, method: &str, params: Value) -> Result<(), AcpError> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write(&message)
    }

    fn write(&self, message: &Value) -> Result<(), AcpError> {
        let text = serde_json::to_string(message)
            .map_err(|error| AcpError::Protocol(error.to_string()))?;
        let mut guard = self.stdin.lock().expect("the stdin lock is not poisoned");
        let stdin = guard.as_mut().ok_or(AcpError::Closed)?;
        // The client always writes ndjson; the endpoint answers in the
        // framing it detected, and it detects ndjson from this.
        stdin
            .write_all(&serialize(&text, Framing::Ndjson))
            .and_then(|()| stdin.flush())
            .map_err(AcpError::Write)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Closing stdin lets the endpoint shut down on its own.
        if let Ok(mut guard) = self.stdin.lock() {
            guard.take();
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Reads frames from the agent endpoint until it closes its output.
fn read_loop(
    stdout: std::process::ChildStdout,
    pending: Arc<Mutex<HashMap<crate::protocol::RequestId, Pending>>>,
    turns: Arc<Mutex<HashMap<crate::protocol::RequestId, String>>>,
    events: Sender<SessionEvent>,
    running: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stdout);
    let mut frames = FrameReader::new();
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                for frame in frames.push(line.as_bytes()) {
                    dispatch(&frame, &pending, &turns, &events);
                }
            }
            Err(_) => break,
        }
    }

    running.store(false, Ordering::SeqCst);
    // Fail every request that is still waiting, so callers do not hang.
    let mut pending = pending.lock().expect("the pending lock is not poisoned");
    for (_, entry) in pending.drain() {
        let _ = entry.sender.send(Err(AcpError::Closed));
    }
    let _ = events.send(SessionEvent::Disconnected {
        reason: "the agent endpoint closed its output".to_owned(),
    });
}

/// Handles one decoded frame.
fn dispatch(
    frame: &str,
    pending: &Arc<Mutex<HashMap<crate::protocol::RequestId, Pending>>>,
    turns: &Arc<Mutex<HashMap<crate::protocol::RequestId, String>>>,
    events: &Sender<SessionEvent>,
) {
    let Ok(message) = serde_json::from_str::<Value>(frame) else {
        let _ = events.send(SessionEvent::Disconnected {
            reason: format!("the agent endpoint sent a malformed message: {frame}"),
        });
        return;
    };

    let has_id = message.get("id").is_some_and(|id| !id.is_null());
    let method = message.get("method").and_then(|method| method.as_str());

    match (has_id, method) {
        // A response to one of our requests.
        (true, None) => {
            let Some(id) = message.get("id").and_then(|id| id.as_i64()) else {
                return;
            };

            // A turn started with `start_prompt` ends here.
            let is_turn = turns
                .lock()
                .expect("the turn lock is not poisoned")
                .remove(&id)
                .is_some();
            if is_turn {
                // A turn the endpoint refused ends with an error, not with a result. The reason in
                // that error is the only thing that says what went wrong, so it is carried over:
                // a turn that failed as "unknown" is a failure nobody can act on.
                if let Some(error) = message.get("error") {
                    let reason = error
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("the agent endpoint refused the turn")
                        .to_owned();
                    let _ = events.send(SessionEvent::Disconnected { reason });
                    return;
                }
                let reason = message
                    .get("result")
                    .and_then(|result| result.get("stopReason"))
                    .and_then(|reason| reason.as_str())
                    .map(StopReason::from_wire)
                    .unwrap_or(StopReason::Unknown);
                let _ = events.send(SessionEvent::TurnCompleted {
                    stop_reason: reason,
                });
                return;
            }

            let entry = pending
                .lock()
                .expect("the pending lock is not poisoned")
                .remove(&id);
            let Some(entry) = entry else {
                return;
            };
            let outcome = if let Some(error) = message.get("error") {
                Err(AcpError::Remote {
                    code: error
                        .get("code")
                        .and_then(|code| code.as_i64())
                        .unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("unknown error")
                        .to_owned(),
                })
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = entry.sender.send(outcome);
        }
        // A request from the agent: the only one it makes is a permission
        // request, which the editor has to answer.
        (true, Some(method)) => {
            let Some(id) = message.get("id").and_then(|id| id.as_i64()) else {
                return;
            };
            if method == "session/request_permission"
                && let Some(request) = parse_permission_request(&message)
            {
                let _ = events.send(SessionEvent::PermissionRequested {
                    request_id: id,
                    request,
                });
            }
        }
        // A notification.
        (false, Some(method)) => {
            if method == "session/update" {
                let Some(params) = message.get("params") else {
                    return;
                };
                let Some(update) = params.get("update") else {
                    return;
                };
                let Some(update) = SessionUpdate::parse(update) else {
                    return;
                };
                match update {
                    SessionUpdate::Message(chunk) => {
                        let _ = events.send(SessionEvent::Message { text: chunk.text });
                    }
                    SessionUpdate::ToolCall(call) => {
                        let _ = events.send(SessionEvent::ToolCall {
                            tool_call_id: call.tool_call_id,
                            title: call.title,
                            status: call.status,
                            kind: call.kind,
                            content: call.content,
                        });
                    }
                    SessionUpdate::Unknown(_) => {}
                }
            }
        }
        (false, None) => {}
    }
}

/// Parses a `session/request_permission` request.
fn parse_permission_request(message: &Value) -> Option<PermissionRequest> {
    let params = message.get("params")?;
    let tool_call = params.get("toolCall");
    Some(PermissionRequest {
        session_id: params
            .get("sessionId")
            .and_then(|id| id.as_str())
            .unwrap_or_default()
            .to_owned(),
        tool_call_id: tool_call
            .and_then(|call| call.get("toolCallId"))
            .and_then(|id| id.as_str())
            .unwrap_or_default()
            .to_owned(),
        title: tool_call
            .and_then(|call| call.get("title"))
            .and_then(|title| title.as_str())
            .unwrap_or_default()
            .to_owned(),
        options: params
            .get("options")
            .and_then(|options| options.as_array())
            .map(|options| {
                options
                    .iter()
                    .map(|option| PermissionOption {
                        option_id: option
                            .get("optionId")
                            .and_then(|id| id.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        name: option
                            .get("name")
                            .and_then(|name| name.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        kind: option
                            .get("kind")
                            .and_then(|kind| kind.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds one frame through the dispatcher and returns the events it produced.
    fn events_for(frame: &str) -> Vec<SessionEvent> {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let turns = Arc::new(Mutex::new(HashMap::new()));
        let (sender, events) = channel();
        turns
            .lock()
            .expect("the turn lock is not poisoned")
            .insert(7, "sess-1".to_owned());
        dispatch(frame, &pending, &turns, &sender);
        events.try_iter().collect()
    }

    #[test]
    fn a_refused_turn_carries_the_endpoints_reason() {
        let events = events_for(
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"Unknown runtime no-such-runtime"}}"#,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionEvent::Disconnected { reason } => {
                assert_eq!(reason, "Unknown runtime no-such-runtime");
            }
            other => panic!("expected the refusal to be reported, got {other:?}"),
        }
    }

    #[test]
    fn a_finished_turn_reports_its_stop_reason() {
        let events = events_for(r#"{"jsonrpc":"2.0","id":7,"result":{"stopReason":"end_turn"}}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            SessionEvent::TurnCompleted {
                stop_reason: StopReason::EndTurn
            }
        ));
    }
}
