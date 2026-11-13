//! The client for the Debug Adapter Protocol.
//!
//! The client owns one child process — the debug adapter. Requests are
//! written to its stdin, and a background thread reads its stdout, decoding
//! frames and dispatching them:
//!
//! - an event becomes a [`DapEvent`];
//! - a response resolves the request that is waiting for it;
//! - a request from the adapter is answered with an empty result, so the
//!   adapter carries on.
//!
//! The editor's event loop never blocks on the adapter: it drains
//! [`Client::drain_events`] once per frame, and the steps of a debug session
//! are driven from there.

pub mod framing;
pub mod protocol;

use std::{
    collections::HashMap,
    io::{BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
};

use serde_json::{Value, json};

use crate::framing::{FrameReader, serialize};
use crate::protocol::{Breakpoint, Scope, StackFrame, Variable};

/// Something went wrong while talking to the adapter.
#[derive(Debug, thiserror::Error)]
pub enum DapError {
    #[error("could not start the debug adapter: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("could not write to the debug adapter: {0}")]
    Write(#[source] std::io::Error),
    #[error("the debug adapter closed its output")]
    Closed,
    #[error("the debug adapter reported an error: {message}")]
    Remote { message: String },
    #[error("the debug adapter sent a malformed message: {0}")]
    Protocol(String),
}

/// What the adapter says, in the editor's own words.
#[derive(Clone, Debug, PartialEq)]
pub enum DapEvent {
    /// The adapter is ready for the breakpoints.
    Initialized,
    /// The program stopped.
    Stopped {
        reason: String,
        thread_id: i64,
        all_threads_stopped: bool,
    },
    /// The program is running again.
    Continued { thread_id: i64 },
    /// The adapter said something.
    Output { category: String, text: String },
    /// A breakpoint of the editor was placed.
    Breakpoint {
        verified: bool,
        line: Option<u32>,
        message: Option<String>,
    },
    /// The debug session is over.
    Terminated,
    /// The program itself is over.
    Exited { code: i64 },
    /// The adapter went away.
    Disconnected { reason: String },
}

/// How to start the debug adapter.
#[derive(Clone, Debug)]
pub struct AdapterConfig {
    /// The program to run.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
}

impl AdapterConfig {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        AdapterConfig {
            program: program.into(),
            args,
        }
    }
}

/// A request that is waiting for its answer.
enum Pending {
    /// The answer goes back to the caller that asked for it.
    Answer(Sender<Result<Value, DapError>>),
    /// The answer of `setBreakpoints` is the list of the breakpoints that
    /// were placed.
    Breakpoints(Sender<Result<Vec<Breakpoint>, DapError>>),
    /// The answer of `stackTrace` is the list of the frames.
    StackTrace(Sender<Result<Vec<StackFrame>, DapError>>),
    /// The answer of `scopes` is the list of the groups of variables.
    Scopes(Sender<Result<Vec<Scope>, DapError>>),
    /// The answer of `variables` is the list of the variables.
    Variables(Sender<Result<Vec<Variable>, DapError>>),
}

/// What a message of the adapter says.
#[derive(Clone, Debug, PartialEq)]
enum Message {
    /// Something happened.
    Event(DapEvent),
    /// The answer of a request of the editor.
    Response {
        seq: i64,
        success: bool,
        body: Value,
        message: Option<String>,
    },
    /// The adapter asks the editor to do something.
    Request { seq: i64, command: String },
}

/// Reads a message of the adapter.
///
/// A message that cannot be read is nothing: the adapter said something the
/// editor does not understand, and that must not stop the session.
fn parse(text: &str) -> Option<Message> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value.get("type")?.as_str()? {
        "event" => parse_event(&value).map(Message::Event),
        "response" => Some(Message::Response {
            seq: value
                .get("request_seq")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            success: value
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            body: value.get("body").cloned().unwrap_or(Value::Null),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "request" => Some(Message::Request {
            seq: value.get("seq").and_then(Value::as_i64).unwrap_or_default(),
            command: value
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        _ => None,
    }
}

/// Reads an event of the adapter.
fn parse_event(value: &Value) -> Option<DapEvent> {
    let body = value.get("body");
    let number = |name: &str| {
        body.and_then(|body| body.get(name))
            .and_then(Value::as_i64)
            .unwrap_or_default()
    };
    let text = |name: &str| {
        body.and_then(|body| body.get(name))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match value.get("event")?.as_str()? {
        "initialized" => Some(DapEvent::Initialized),
        "stopped" => Some(DapEvent::Stopped {
            reason: text("reason"),
            thread_id: number("threadId"),
            all_threads_stopped: body
                .and_then(|body| body.get("allThreadsStopped"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }),
        "continued" => Some(DapEvent::Continued {
            thread_id: number("threadId"),
        }),
        "output" => Some(DapEvent::Output {
            category: text("category"),
            text: text("output"),
        }),
        "breakpoint" => Some(DapEvent::Breakpoint {
            verified: body
                .and_then(|body| body.get("verified"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            line: body
                .and_then(|body| body.get("line"))
                .and_then(Value::as_u64)
                .map(|line| line as u32),
            message: Some(text("message")),
        }),
        "terminated" => Some(DapEvent::Terminated),
        "exited" => Some(DapEvent::Exited { code: number("exitCode") }),
        _ => None,
    }
}

/// A connection to a debug adapter.
pub struct Client {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_seq: Arc<Mutex<i64>>,
    pending: Arc<Mutex<HashMap<i64, Pending>>>,
    /// The sending halves of the channels.
    ///
    /// Every request needs a channel of its own kind to answer on, so the
    /// sending half has to be within reach: the reader thread has a copy,
    /// and the client keeps one for the requests it sends.
    senders: Senders,
    events: Receiver<DapEvent>,
    responses: Receiver<Result<Value, DapError>>,
    breakpoints: Receiver<Result<Vec<Breakpoint>, DapError>>,
    stack_traces: Receiver<Result<Vec<StackFrame>, DapError>>,
    scopes: Receiver<Result<Vec<Scope>, DapError>>,
    variables: Receiver<Result<Vec<Variable>, DapError>>,
    /// Whether the adapter has said that it is ready.
    ready: Arc<AtomicBool>,
    /// Whether the adapter is still there.
    running: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    child: Option<Child>,
}

/// The channels the reader thread writes to.
#[derive(Clone)]
struct Senders {
    events: Sender<DapEvent>,
    responses: Sender<Result<Value, DapError>>,
    breakpoints: Sender<Result<Vec<Breakpoint>, DapError>>,
    stack_traces: Sender<Result<Vec<StackFrame>, DapError>>,
    scopes: Sender<Result<Vec<Scope>, DapError>>,
    variables: Sender<Result<Vec<Variable>, DapError>>,
}

impl Client {
    /// Starts the debug adapter.
    ///
    /// This returns as soon as the process is up: the session itself is
    /// driven by the editor, one frame at a time.
    pub fn start(config: &AdapterConfig) -> Result<Self, DapError> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = command.spawn().map_err(DapError::Spawn)?;
        let stdin = Arc::new(Mutex::new(Some(
            child.stdin.take().expect("stdin was piped"),
        )));
        let stdout = child.stdout.take().expect("stdout was piped");

        let (event_sender, events) = channel();
        let (response_sender, responses) = channel();
        let (breakpoint_sender, breakpoints) = channel();
        let (stack_trace_sender, stack_traces) = channel();
        let (scope_sender, scopes) = channel();
        let (variable_sender, variables) = channel();
        let senders = Senders {
            events: event_sender,
            responses: response_sender,
            breakpoints: breakpoint_sender,
            stack_traces: stack_trace_sender,
            scopes: scope_sender,
            variables: variable_sender,
        };
        let pending: Arc<Mutex<HashMap<i64, Pending>>> = Arc::new(Mutex::new(HashMap::new()));
        let next_seq = Arc::new(Mutex::new(1));
        let ready = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));

        let reader = {
            let pending = Arc::clone(&pending);
            let ready = Arc::clone(&ready);
            let running = Arc::clone(&running);
            let stdin = Arc::clone(&stdin);
            let thread_senders = senders.clone();
            std::thread::spawn(move || {
                read_loop(stdout, stdin, pending, thread_senders, ready, running)
            })
        };

        Ok(Client {
            stdin,
            next_seq,
            pending,
            senders,
            events,
            responses,
            breakpoints,
            stack_traces,
            scopes,
            variables,
            ready,
            running,
            reader: Some(reader),
            child: Some(child),
        })
    }

    /// Whether the adapter has said that it is ready for the breakpoints.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Whether the adapter is still there.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Starts the handshake of the protocol.
    ///
    /// The adapter answers with what it can do, and then says that it is
    /// ready: the breakpoints go after that.
    pub fn initialize(&self) -> Result<(), DapError> {
        self.request(
            "initialize",
            json!({
                "clientID": "codeweaver",
                "adapterID": "codeweaver",
                "pathFormat": "path",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "supportsVariableType": true,
            }),
            Some(Pending::Answer(self.responses_sender())),
        )
    }

    /// Tells the adapter where the breakpoints are.
    pub fn set_breakpoints(
        &self,
        uri: &str,
        lines: &[u32],
    ) -> Result<(), DapError> {
        let breakpoints: Vec<crate::protocol::SourceBreakpoint> = lines
            .iter()
            .map(|line| crate::protocol::SourceBreakpoint {
                line: *line,
                source: Some(crate::protocol::Source {
                    path: Some(uri.to_owned()),
                    name: None,
                }),
            })
            .collect();
        self.request(
            "setBreakpoints",
            json!({
                "source": { "path": uri },
                "breakpoints": breakpoints,
                "lines": lines,
            }),
            Some(Pending::Breakpoints(self.breakpoints_sender())),
        )
    }

    /// Tells the adapter that everything is set up.
    pub fn configuration_done(&self) -> Result<(), DapError> {
        self.request("configurationDone", json!({}), None)
    }

    /// Starts the program.
    pub fn launch(&self, program: &str, args: &[String]) -> Result<(), DapError> {
        self.request(
            "launch",
            json!({
                "program": program,
                "args": args,
                "stopOnEntry": false,
            }),
            None,
        )
    }

    /// Runs one line and does not go into what it calls.
    pub fn next(&self, thread_id: i64) -> Result<(), DapError> {
        self.request("next", json!({ "threadId": thread_id }), None)
    }

    /// Runs one line and goes into what it calls.
    pub fn step_in(&self, thread_id: i64) -> Result<(), DapError> {
        self.request("stepIn", json!({ "threadId": thread_id }), None)
    }

    /// Runs until the frame it is in is over.
    pub fn step_out(&self, thread_id: i64) -> Result<(), DapError> {
        self.request("stepOut", json!({ "threadId": thread_id }), None)
    }

    /// Lets the program run on.
    pub fn continue_(&self, thread_id: i64) -> Result<(), DapError> {
        self.request("continue", json!({ "threadId": thread_id }), None)
    }

    /// Ends the session.
    pub fn terminate(&self) -> Result<(), DapError> {
        self.request("terminate", json!({}), None)
    }

    /// Asks for the frames of the stack.
    pub fn stack_trace(&self, thread_id: i64) -> Result<(), DapError> {
        self.request(
            "stackTrace",
            json!({ "threadId": thread_id, "startFrame": 0, "levels": 20 }),
            Some(Pending::StackTrace(self.stack_trace_sender())),
        )
    }

    /// Asks for the groups of variables of a frame.
    pub fn scopes(&self, frame_id: i64) -> Result<(), DapError> {
        self.request(
            "scopes",
            json!({ "frameId": frame_id }),
            Some(Pending::Scopes(self.scopes_sender())),
        )
    }

    /// Asks for the variables of a group.
    pub fn variables(&self, reference: i64) -> Result<(), DapError> {
        self.request(
            "variables",
            json!({ "variablesReference": reference }),
            Some(Pending::Variables(self.variables_sender())),
        )
    }

    /// Everything the adapter has said since the last time.
    pub fn drain_events(&self) -> Vec<DapEvent> {
        drain(&self.events)
    }

    /// The answers of the requests that are not about breakpoints or frames.
    pub fn drain_responses(&self) -> Vec<Result<Value, DapError>> {
        drain(&self.responses)
    }

    /// The answers of the requests for the breakpoints.
    pub fn drain_breakpoints(&self) -> Vec<Result<Vec<Breakpoint>, DapError>> {
        drain(&self.breakpoints)
    }

    /// The answers of the requests for the frames.
    pub fn drain_stack_traces(&self) -> Vec<Result<Vec<StackFrame>, DapError>> {
        drain(&self.stack_traces)
    }

    /// The answers of the requests for the groups of variables.
    pub fn drain_scopes(&self) -> Vec<Result<Vec<Scope>, DapError>> {
        drain(&self.scopes)
    }

    /// The answers of the requests for the variables.
    pub fn drain_variables(&self) -> Vec<Result<Vec<Variable>, DapError>> {
        drain(&self.variables)
    }

    /// Stops the adapter and waits for it.
    pub fn shutdown(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }

    /// Sends a request to the adapter.
    fn request(
        &self,
        command: &str,
        arguments: Value,
        pending: Option<Pending>,
    ) -> Result<(), DapError> {
        let seq = {
            let mut next = self
                .next_seq
                .lock()
                .expect("the sequence lock is not poisoned");
            let seq = *next;
            *next += 1;
            seq
        };
        let message = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        let text = serde_json::to_string(&message)
            .map_err(|error| DapError::Protocol(error.to_string()))?;

        if let Some(pending) = pending {
            self.pending
                .lock()
                .expect("the pending lock is not poisoned")
                .insert(seq, pending);
        }

        let mut guard = self.stdin.lock().expect("the stdin lock is not poisoned");
        let Some(stdin) = guard.as_mut() else {
            self.pending
                .lock()
                .expect("the pending lock is not poisoned")
                .remove(&seq);
            return Err(DapError::Closed);
        };
        if let Err(error) = stdin
            .write_all(&serialize(&text))
            .and_then(|_| stdin.flush())
        {
            self.pending
                .lock()
                .expect("the pending lock is not poisoned")
                .remove(&seq);
            return Err(DapError::Write(error));
        }
        Ok(())
    }

    /// A channel for the answers of the requests that are not about
    /// breakpoints or frames.
    fn responses_sender(&self) -> Sender<Result<Value, DapError>> {
        self.senders.responses.clone()
    }

    /// A channel for the answers of the requests for the breakpoints.
    fn breakpoints_sender(&self) -> Sender<Result<Vec<Breakpoint>, DapError>> {
        self.senders.breakpoints.clone()
    }

    /// A channel for the answers of the requests for the frames.
    fn stack_trace_sender(&self) -> Sender<Result<Vec<StackFrame>, DapError>> {
        self.senders.stack_traces.clone()
    }

    /// A channel for the answers of the requests for the groups of variables.
    fn scopes_sender(&self) -> Sender<Result<Vec<Scope>, DapError>> {
        self.senders.scopes.clone()
    }

    /// A channel for the answers of the requests for the variables.
    fn variables_sender(&self) -> Sender<Result<Vec<Variable>, DapError>> {
        self.senders.variables.clone()
    }
}

/// Takes everything that is in a channel.
fn drain<T>(receiver: &Receiver<T>) -> Vec<T> {
    let mut items = Vec::new();
    while let Ok(item) = receiver.try_recv() {
        items.push(item);
    }
    items
}

/// Reads the frames of the adapter and dispatches them.
fn read_loop(
    stdout: ChildStdout,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<i64, Pending>>>,
    senders: Senders,
    ready: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stdout);
    let mut frames = FrameReader::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        for text in frames.push(&buffer[..read]) {
            dispatch(&text, &pending, &senders, &ready, &stdin);
        }
    }
    running.store(false, Ordering::SeqCst);
    let _ = senders.events.send(DapEvent::Disconnected {
        reason: "调试适配器退出了".to_owned(),
    });
}

/// Gives one message of the adapter to whoever is waiting for it.
fn dispatch(
    text: &str,
    pending: &Arc<Mutex<HashMap<i64, Pending>>>,
    senders: &Senders,
    ready: &Arc<AtomicBool>,
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
) {
    let Some(message) = parse(text) else {
        return;
    };
    match message {
        Message::Event(event) => {
            if event == DapEvent::Initialized {
                ready.store(true, Ordering::SeqCst);
            }
            let _ = senders.events.send(event);
        }
        Message::Response {
            seq,
            success,
            body,
            message,
        } => {
            let waiting = pending
                .lock()
                .expect("the pending lock is not poisoned")
                .remove(&seq);
            let Some(waiting) = waiting else {
                return;
            };
            let error = if success {
                None
            } else {
                Some(DapError::Remote {
                    message: message.unwrap_or_else(|| "适配器没有说为什么失败".to_owned()),
                })
            };
            match waiting {
                Pending::Answer(sender) => {
                    let _ = sender.send(match error {
                        Some(error) => Err(error),
                        None => Ok(body),
                    });
                }
                Pending::Breakpoints(sender) => {
                    let _ = sender.send(match error {
                        Some(error) => Err(error),
                        None => Ok(serde_json::from_value(body).unwrap_or_default()),
                    });
                }
                Pending::StackTrace(sender) => {
                    let _ = sender.send(match error {
                        Some(error) => Err(error),
                        None => Ok(serde_json::from_value(body).unwrap_or_default()),
                    });
                }
                Pending::Scopes(sender) => {
                    let _ = sender.send(match error {
                        Some(error) => Err(error),
                        None => Ok(serde_json::from_value(body).unwrap_or_default()),
                    });
                }
                Pending::Variables(sender) => {
                    let _ = sender.send(match error {
                        Some(error) => Err(error),
                        None => Ok(serde_json::from_value(body).unwrap_or_default()),
                    });
                }
            }
        }
        Message::Request { seq, .. } => {
            // The adapter asks the editor to do something. The editor does
            // not do anything yet, but the adapter must not wait for an
            // answer that never comes.
            let message = json!({
                "seq": seq,
                "type": "response",
                "request_seq": seq,
                "success": true,
                "command": "unsupported",
                "body": {},
            });
            if let Ok(text) = serde_json::to_string(&message) {
                let mut guard = stdin.lock().expect("the stdin lock is not poisoned");
                if let Some(stdin) = guard.as_mut() {
                    let _ = stdin.write_all(&serialize(&text));
                    let _ = stdin.flush();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_of_the_adapter_is_read() {
        let message = parse(
            r#"{"seq":2,"type":"event","event":"stopped","body":{"reason":"breakpoint","threadId":3,"allThreadsStopped":true}}"#,
        )
        .expect("the message is read");
        assert_eq!(
            message,
            Message::Event(DapEvent::Stopped {
                reason: "breakpoint".to_owned(),
                thread_id: 3,
                all_threads_stopped: true,
            })
        );
    }

    #[test]
    fn the_output_of_the_adapter_is_read() {
        let message = parse(
            r#"{"seq":3,"type":"event","event":"output","body":{"category":"stdout","output":"hello\n"}}"#,
        )
        .expect("the message is read");
        assert_eq!(
            message,
            Message::Event(DapEvent::Output {
                category: "stdout".to_owned(),
                text: "hello\n".to_owned(),
            })
        );
    }

    #[test]
    fn a_response_is_read_with_the_request_it_answers() {
        let message = parse(
            r#"{"seq":9,"type":"response","request_seq":4,"success":true,"body":{"id":7}}"#,
        )
        .expect("the message is read");
        match message {
            Message::Response {
                seq,
                success,
                body,
                message,
            } => {
                assert_eq!(seq, 4);
                assert!(success);
                assert_eq!(body["id"], 7);
                assert!(message.is_none());
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_response_that_failed_says_why() {
        let message = parse(
            r#"{"seq":9,"type":"response","request_seq":4,"success":false,"message":"no launch configuration"}"#,
        )
        .expect("the message is read");
        match message {
            Message::Response { success, message, .. } => {
                assert!(!success);
                assert_eq!(message.as_deref(), Some("no launch configuration"));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn an_event_the_editor_does_not_know_is_not_a_problem() {
        assert!(parse(r#"{"seq":1,"type":"event","event":"thread"}"#).is_none());
    }

    #[test]
    fn a_message_that_is_not_json_is_not_a_problem() {
        assert!(parse("not json at all").is_none());
    }

    #[test]
    fn a_request_of_the_adapter_is_read() {
        let message = parse(r#"{"seq":8,"type":"request","command":"runInTerminal"}"#)
            .expect("the message is read");
        assert_eq!(
            message,
            Message::Request {
                seq: 8,
                command: "runInTerminal".to_owned()
            }
        );
    }
}
