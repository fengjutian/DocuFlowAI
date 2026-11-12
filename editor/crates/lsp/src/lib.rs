//! The client for the Language Server Protocol.
//!
//! The client owns one child process — the language server. Requests are
//! written to its stdin, and a background thread reads its stdout, decoding
//! frames and dispatching them:
//!
//! - a response resolves the request that is waiting for it;
//! - `textDocument/publishDiagnostics` becomes an [`LspEvent::Diagnostics`];
//! - a request from the server is answered with an empty result, so the
//!   server carries on.
//!
//! The editor's event loop never blocks on the server: it drains
//! [`Client::drain_events`] once per frame, and the handshake happens in the
//! background too, so a server that takes a second to start does not freeze
//! the window.

pub mod framing;
pub mod positions;
pub mod protocol;

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
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
    framing::{FrameReader, serialize},
    protocol::{
        CompletionList, Diagnostic, Position, TextChange, TextDocumentIdentifier, TextDocumentItem,
        TextDocumentSyncKind, VersionedTextDocumentIdentifier,
    },
};

/// The version of the protocol the editor speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Something went wrong while talking to the server.
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("could not start the language server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("could not write to the language server: {0}")]
    Write(#[source] std::io::Error),
    #[error("the language server closed its output")]
    Closed,
    #[error("the language server reported an error: {message}")]
    Remote { code: i64, message: String },
    #[error("the language server sent a malformed message: {0}")]
    Protocol(String),
}

/// What the server says, in the editor's own words.
#[derive(Clone, Debug, PartialEq)]
pub enum LspEvent {
    /// The server found things to say about a document.
    Diagnostics {
        uri: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// The server said something.
    Message { text: String },
    /// The server went away.
    Disconnected { reason: String },
}

/// How to start the language server.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// The program to run.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
}

impl ServerConfig {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        ServerConfig {
            program: program.into(),
            args,
        }
    }
}

/// A request that is waiting for its answer.
enum Pending {
    /// The answer goes back to the caller that asked for it.
    Answer(Sender<Result<Value, LspError>>),
    /// A completion is not a JSON-RPC result but a list of items, so it has
    /// its own way in.
    Completion(Sender<CompletionList>),
}

/// The document that was opened before the handshake was done.
///
/// The server has not seen the document yet, so the text that is kept here
/// is the text it gets when the handshake ends.
struct Waiting {
    uri: String,
    language_id: String,
    version: i64,
    text: String,
}

/// A connection to a language server.
pub struct Client {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    next_id: Arc<Mutex<i64>>,
    pending: Arc<Mutex<HashMap<i64, Pending>>>,
    events: Receiver<LspEvent>,
    completions: Receiver<CompletionList>,
    /// Where the answer of a completion goes.
    completion_sender: Sender<CompletionList>,
    responses: Receiver<Result<Value, LspError>>,
    /// Where the answer of an ordinary request goes.
    response_sender: Sender<Result<Value, LspError>>,
    running: Arc<AtomicBool>,
    /// Whether the handshake is done.
    ready: Arc<AtomicBool>,
    /// How the server wants to be told about changes.
    sync_kind: Arc<Mutex<TextDocumentSyncKind>>,
    /// The document that was opened before the handshake was done.
    waiting: Arc<Mutex<Option<Waiting>>>,
    /// The last lines the server wrote to its error output.
    errors: Arc<Mutex<Vec<String>>>,
    reader: Option<JoinHandle<()>>,
    child: Option<Child>,
}

impl Client {
    /// Starts the language server.
    ///
    /// The handshake runs in the background, so this returns as soon as the
    /// process is up. A document that is opened before the handshake is done
    /// is sent as soon as it is.
    pub fn start(config: &ServerConfig, root: &Path) -> Result<Self, LspError> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(LspError::Spawn)?;
        let stdin = Arc::new(Mutex::new(Some(
            child.stdin.take().expect("stdin was piped"),
        )));
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let (event_sender, events) = channel();
        let (completion_sender, completions) = channel();
        let (response_sender, responses) = channel();
        let client_completions = completion_sender.clone();
        let client_responses = response_sender.clone();
        let pending: Arc<Mutex<HashMap<i64, Pending>>> = Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(Mutex::new(1));
        let running = Arc::new(AtomicBool::new(true));
        let ready = Arc::new(AtomicBool::new(false));
        let sync_kind = Arc::new(Mutex::new(TextDocumentSyncKind::Full));
        let waiting: Arc<Mutex<Option<Waiting>>> = Arc::new(Mutex::new(None));

        // The server writes what it thinks about the program to its error
        // output. Nobody is watching it, but when the server dies that text
        // says why, so it is kept.
        let errors = Arc::new(Mutex::new(Vec::new()));
        std::thread::spawn({
            let errors = Arc::clone(&errors);
            move || read_errors(stderr, errors)
        });
        let reader = {
            let pending = Arc::clone(&pending);
            let running = Arc::clone(&running);
            let stdin = Arc::clone(&stdin);
            let errors = Arc::clone(&errors);
            std::thread::spawn(move || {
                read_loop(stdout, stdin, pending, event_sender, running, errors)
            })
        };

        // The handshake runs in its own thread: a server that needs a second
        // to start must not hold the window.
        std::thread::spawn({
            let stdin = Arc::clone(&stdin);
            let pending = Arc::clone(&pending);
            let next_id = Arc::clone(&next_id);
            let ready = Arc::clone(&ready);
            let sync_kind = Arc::clone(&sync_kind);
            let waiting = Arc::clone(&waiting);
            let root = root.to_path_buf();
            move || {
                handshake(
                    &stdin, &pending, &next_id, &ready, &sync_kind, &waiting, &root,
                )
            }
        });

        Ok(Client {
            stdin,
            next_id,
            pending,
            events,
            completions,
            completion_sender: client_completions,
            responses,
            response_sender: client_responses,
            running,
            ready,
            sync_kind,
            waiting,
            errors,
            reader: Some(reader),
            child: Some(child),
        })
    }

    /// The last lines the server wrote to its error output.
    ///
    /// A server that does not start says why on its error output, and this
    /// is where that text is.
    pub fn stderr_tail(&self) -> String {
        let errors = self.errors.lock().expect("the error lock is not poisoned");
        errors.join("\n")
    }

    /// Whether the server has finished its handshake.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Whether the server is still there.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// How the server wants to be told about changes.
    pub fn sync_kind(&self) -> TextDocumentSyncKind {
        *self
            .sync_kind
            .lock()
            .expect("the sync kind lock is not poisoned")
    }

    /// Tells the server that a document was opened.
    pub fn did_open(
        &self,
        uri: &str,
        language_id: &str,
        version: i64,
        text: &str,
    ) -> Result<(), LspError> {
        if !self.is_ready() {
            // The handshake is not done yet; the document goes as soon as it
            // is, with the text it has then.
            let mut waiting = self
                .waiting
                .lock()
                .expect("the waiting lock is not poisoned");
            *waiting = Some(Waiting {
                uri: uri.to_owned(),
                language_id: language_id.to_owned(),
                version,
                text: text.to_owned(),
            });
            return Ok(());
        }
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": TextDocumentItem {
                    uri: uri.to_owned(),
                    language_id: language_id.to_owned(),
                    version,
                    text: text.to_owned(),
                },
            }),
        )
    }

    /// Tells the server that a document changed.
    ///
    /// `text` is the whole text of the document, and `changes` says what
    /// changed in it. A server that wants the whole document after every
    /// change gets `text`; a server that wants the range gets `changes`.
    pub fn did_change(
        &self,
        uri: &str,
        version: i64,
        text: &str,
        changes: &[TextChange],
    ) -> Result<(), LspError> {
        if !self.is_ready() {
            // The server has not seen the document yet, so it gets the whole
            // text when the handshake ends. The ranges of the changes are of
            // no use here: there is no text to put them in.
            let mut waiting = self
                .waiting
                .lock()
                .expect("the waiting lock is not poisoned");
            if let Some(document) = waiting.as_mut() {
                document.version = version;
                document.text = text.to_owned();
            }
            return Ok(());
        }

        let changes = if self.sync_kind() == TextDocumentSyncKind::Full {
            vec![TextChange::whole(text)]
        } else {
            changes.to_vec()
        };
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": VersionedTextDocumentIdentifier {
                    uri: uri.to_owned(),
                    version,
                },
                "contentChanges": changes,
            }),
        )
    }

    /// Tells the server that a document was closed.
    pub fn did_close(&self, uri: &str) -> Result<(), LspError> {
        self.notify(
            "textDocument/didClose",
            json!({
                "textDocument": TextDocumentIdentifier { uri: uri.to_owned() },
            }),
        )
    }

    /// Asks the server what could come next at a place in the document.
    ///
    /// The answer arrives later: it is in [`Client::drain_completions`].
    pub fn request_completion(&self, uri: &str, position: Position) -> Result<(), LspError> {
        let id = self.next_id();
        self.pending
            .lock()
            .expect("the pending lock is not poisoned")
            .insert(id, Pending::Completion(self.completion_sender.clone()));

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "textDocument/completion",
            "params": {
                "textDocument": TextDocumentIdentifier { uri: uri.to_owned() },
                "position": position,
            },
        });
        self.write(&message)
    }

    /// Asks the server something else.
    ///
    /// The answer arrives later: it is in [`Client::drain_responses`].
    pub fn request(&self, method: &str, params: Value) -> Result<(), LspError> {
        let id = self.next_id();
        self.pending
            .lock()
            .expect("the pending lock is not poisoned")
            .insert(id, Pending::Answer(self.response_sender.clone()));

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write(&message)
    }

    /// Returns every event that arrived since the last call.
    pub fn drain_events(&self) -> Vec<LspEvent> {
        self.events.try_iter().collect()
    }

    /// Returns the completions that arrived since the last call.
    pub fn drain_completions(&self) -> Vec<CompletionList> {
        self.completions.try_iter().collect()
    }

    /// Returns the answers of the ordinary requests that arrived since the
    /// last call.
    ///
    /// The answers that came back as an error are left out: the caller asked
    /// for something, and there is nothing to show.
    pub fn drain_responses(&self) -> Vec<Value> {
        self.responses
            .try_iter()
            .filter_map(|answer| answer.ok())
            .collect()
    }

    /// The next id of a request.
    fn next_id(&self) -> i64 {
        let mut next = self
            .next_id
            .lock()
            .expect("the request id lock is not poisoned");
        let id = *next;
        *next += 1;
        id
    }

    /// Sends a notification.
    fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write(&message)
    }

    fn write(&self, message: &Value) -> Result<(), LspError> {
        write_frame(&self.stdin, message)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Closing stdin lets the server shut down on its own.
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

/// Says hello to the server and waits for its answer.
fn handshake(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    pending: &Arc<Mutex<HashMap<i64, Pending>>>,
    next_id: &Arc<Mutex<i64>>,
    ready: &Arc<AtomicBool>,
    sync_kind: &Arc<Mutex<TextDocumentSyncKind>>,
    waiting: &Arc<Mutex<Option<Waiting>>>,
    root: &Path,
) {
    let id = {
        let mut next = next_id.lock().expect("the request id lock is not poisoned");
        let id = *next;
        *next += 1;
        id
    };
    let (sender, receiver) = channel();
    pending
        .lock()
        .expect("the pending lock is not poisoned")
        .insert(id, Pending::Answer(sender));

    let root_uri = crate::protocol::file_uri(root);
    let message = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "clientInfo": { "name": "CodeWeaver" },
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": { "relatedInformation": true },
                    "completion": { "completionItem": { "snippetSupport": false } }
                },
                "workspace": { "workspaceFolders": true }
            }
        },
    });
    if write_frame(stdin, &message).is_err() {
        return;
    }

    // The answer comes back through the reader thread, which resolves the
    // request.
    let Ok(Ok(answer)) = receiver.recv() else {
        return;
    };

    // The server says how it wants to be told about changes.
    if let Ok(result) = serde_json::from_value::<crate::protocol::InitializeResult>(answer) {
        *sync_kind
            .lock()
            .expect("the sync kind lock is not poisoned") = result.capabilities.text_document_sync;
    }

    let _ = write_frame(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {
                "workspaceFolders": [{
                    "uri": root_uri,
                    "name": root.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| root.to_string_lossy().to_string()),
                }],
            },
        }),
    );
    ready.store(true, Ordering::SeqCst);

    // A document that was opened before the handshake was done goes now, with
    // the text it has then: the server has not seen any text before.
    let document = waiting
        .lock()
        .expect("the waiting lock is not poisoned")
        .take();
    if let Some(document) = document {
        let _ = write_frame(
            stdin,
            &json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {
                    "textDocument": TextDocumentItem {
                        uri: document.uri,
                        language_id: document.language_id,
                        version: document.version,
                        text: document.text,
                    },
                },
            }),
        );
    }
}

/// Reads frames from the server until it closes its output.
fn read_loop(
    stdout: std::process::ChildStdout,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<i64, Pending>>>,
    events: Sender<LspEvent>,
    running: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
) {
    let mut reader = BufReader::new(stdout);
    let mut frames = FrameReader::new();
    let mut chunk = [0u8; 8192];

    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        for frame in frames.push(&chunk[..read]) {
            dispatch(&frame, &stdin, &pending, &events);
        }
    }

    running.store(false, Ordering::SeqCst);
    // Fail every request that is still waiting, so callers do not hang.
    let mut pending = pending.lock().expect("the pending lock is not poisoned");
    for (_, entry) in pending.drain() {
        if let Pending::Answer(sender) = entry {
            let _ = sender.send(Err(LspError::Closed));
        }
    }
    let reason = {
        let errors = errors.lock().expect("the error lock is not poisoned");
        let tail = errors.join("\n");
        if tail.is_empty() {
            "the language server closed its output".to_owned()
        } else {
            format!("the language server closed its output:\n{tail}")
        }
    };
    let _ = events.send(LspEvent::Disconnected { reason });
}

/// Keeps the last lines the server wrote to its error output.
fn read_errors(mut stderr: std::process::ChildStderr, errors: Arc<Mutex<Vec<String>>>) {
    let reader = BufReader::new(&mut stderr);
    for line in reader.lines().map_while(Result::ok) {
        let mut errors = errors.lock().expect("the error lock is not poisoned");
        errors.push(line);
        if errors.len() > 40 {
            errors.remove(0);
        }
    }
}

/// Handles one decoded frame.
fn dispatch(
    frame: &str,
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    pending: &Arc<Mutex<HashMap<i64, Pending>>>,
    events: &Sender<LspEvent>,
) {
    let Ok(message) = serde_json::from_str::<Value>(frame) else {
        let _ = events.send(LspEvent::Disconnected {
            reason: format!("the language server sent a malformed message: {frame}"),
        });
        return;
    };

    let id = message.get("id").and_then(|id| id.as_i64());
    let method = message.get("method").and_then(|method| method.as_str());

    match (id, method) {
        // A response to one of our requests.
        (Some(id), None) => {
            let entry = pending
                .lock()
                .expect("the pending lock is not poisoned")
                .remove(&id);
            let Some(entry) = entry else {
                return;
            };
            let outcome = if let Some(error) = message.get("error") {
                Err(LspError::Remote {
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
            // A completion is not a JSON-RPC result but a list of items, so
            // it goes into its own way in.
            match entry {
                Pending::Completion(completion) => {
                    if let Ok(value) = outcome {
                        let _ = completion.send(completion_list(value));
                    }
                }
                Pending::Answer(sender) => {
                    let _ = sender.send(outcome);
                }
            }
        }
        // A request from the server. It gets an empty answer, so that the
        // server carries on with what it was doing.
        (Some(id), Some(_)) => {
            let answer = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": [],
            });
            let _ = write_frame(stdin, &answer);
        }
        // A notification.
        (None, Some(method)) => {
            if method == "textDocument/publishDiagnostics" {
                let Some(params) = message.get("params") else {
                    return;
                };
                let Some(uri) = params.get("uri").and_then(|uri| uri.as_str()) else {
                    return;
                };
                let diagnostics = params
                    .get("diagnostics")
                    .and_then(|items| serde_json::from_value(items.clone()).ok())
                    .unwrap_or_default();
                let _ = events.send(LspEvent::Diagnostics {
                    uri: uri.to_owned(),
                    diagnostics,
                });
            } else if method == "window/showMessage" {
                let text = message
                    .get("params")
                    .and_then(|params| params.get("message"))
                    .and_then(|message| message.as_str())
                    .unwrap_or_default();
                let _ = events.send(LspEvent::Message {
                    text: text.to_owned(),
                });
            }
        }
        (None, None) => {}
    }
}

/// Writes a frame to the server.
fn write_frame(stdin: &Arc<Mutex<Option<ChildStdin>>>, message: &Value) -> Result<(), LspError> {
    let text =
        serde_json::to_string(message).map_err(|error| LspError::Protocol(error.to_string()))?;
    let mut guard = stdin.lock().expect("the stdin lock is not poisoned");
    let stdin = guard.as_mut().ok_or(LspError::Closed)?;
    stdin
        .write_all(&serialize(&text))
        .and_then(|()| stdin.flush())
        .map_err(LspError::Write)
}

/// The answer of a completion request, in the editor's own shape.
///
/// The server may answer with a list or with an array of items.
fn completion_list(value: Value) -> CompletionList {
    if let Some(items) = value.get("items").and_then(|items| items.as_array()) {
        return CompletionList {
            is_incomplete: value
                .get("isIncomplete")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            items: items
                .iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect(),
        };
    }
    CompletionList {
        is_incomplete: false,
        items: serde_json::from_value(value).unwrap_or_default(),
    }
}
