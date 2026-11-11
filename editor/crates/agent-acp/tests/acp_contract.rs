//! Contract tests against the real ACP agent endpoint.
//!
//! The endpoint under test is the compiled `AcpAgentServer` from
//! `codeweaver-hermes-lab`, with only its backend replaced by a scripted one.
//! The framing, the dispatch and the `session/update` shapes are therefore the
//! production ones.
//!
//! The tests are skipped when Node or the compiled endpoint is unavailable, so
//! the suite still runs on a machine that only has the Rust toolchain.

use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use codeweaver_agent_acp::{
    Client, EndpointConfig, PermissionOutcome, SessionEvent, StopReason, ToolStatus,
};

/// The repository root, derived from this file's location.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repository root exists")
}

/// The mock endpoint script.
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock-acp-agent.mjs")
}

/// Whether the contract tests can run on this machine.
fn endpoint_available() -> bool {
    if Command::new("node").arg("--version").output().is_err() {
        return false;
    }
    repo_root()
        .join("codeweaver-hermes-lab/dist/acp/server.js")
        .exists()
}

/// Starts the mock endpoint with the given scenario.
fn start(scenario: &str) -> Option<Client> {
    if !endpoint_available() {
        return None;
    }
    let config = EndpointConfig::new(
        "node",
        vec![fixture().to_string_lossy().to_string(), scenario.to_owned()],
    )
    .with_env(
        "CODEWEAVER_REPO_ROOT",
        repo_root().to_string_lossy().to_string(),
    );
    match Client::start(&config) {
        Ok(client) => Some(client),
        Err(error) => panic!("could not start the mock endpoint: {error}"),
    }
}

/// The events collected so far.
fn collect(client: &Client) -> Vec<SessionEvent> {
    client.drain_events()
}

/// Waits until the predicate holds, polling the event stream.
fn wait_for<F>(client: &Client, timeout: Duration, mut condition: F) -> Vec<SessionEvent>
where
    F: FnMut(&[SessionEvent]) -> bool,
{
    let deadline = Instant::now() + timeout;
    let mut collected: Vec<SessionEvent> = Vec::new();
    while Instant::now() < deadline {
        collected.extend(collect(client));
        if condition(&collected) {
            return collected;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    collected.extend(collect(client));
    collected
}

#[test]
fn initializes_against_the_real_endpoint() {
    let Some(client) = start("echo") else {
        eprintln!("skipped: the ACP endpoint is not available on this machine");
        return;
    };
    // `Client::start` already ran `initialize`; the endpoint answered it.
    // A second initialize proves the round trip works on demand too.
    let result = client.initialize().expect("initialize is answered");
    assert!(result.agent_info.is_some());
}

#[test]
fn opens_a_session_and_streams_a_reply() {
    let Some(client) = start("echo") else {
        eprintln!("skipped: the ACP endpoint is not available on this machine");
        return;
    };

    let session = client
        .new_session(std::path::Path::new("C:\\repo"))
        .expect("session/new is answered");
    assert_eq!(session.session_id, "sess-mock-1");

    client
        .start_prompt(&session.session_id, "say hello")
        .expect("session/prompt is written");

    let events = wait_for(&client, Duration::from_secs(20), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnCompleted { .. }))
    });

    let messages: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Message { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages,
        vec!["echo: say hello".to_string()],
        "the streamed reply does not match what the endpoint sent"
    );

    let completed = events.iter().find_map(|event| match event {
        SessionEvent::TurnCompleted { stop_reason } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(completed, Some(StopReason::EndTurn));
}

#[test]
fn maps_tool_calls_onto_editor_events() {
    let Some(client) = start("tool") else {
        eprintln!("skipped: the ACP endpoint is not available on this machine");
        return;
    };

    let session = client
        .new_session(std::path::Path::new("C:\\repo"))
        .expect("session/new is answered");
    client
        .start_prompt(&session.session_id, "read something")
        .expect("session/prompt is written");

    let events = wait_for(&client, Duration::from_secs(20), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnCompleted { .. }))
    });

    let started = events.iter().find_map(|event| match event {
        SessionEvent::ToolCall {
            tool_call_id,
            title,
            status,
        } if *status == ToolStatus::InProgress => Some((tool_call_id.clone(), title.clone())),
        _ => None,
    });
    assert_eq!(
        started,
        Some(("call-1".to_string(), "read_file".to_string())),
        "the started tool call was not mapped onto an editor event"
    );

    let completed = events.iter().any(|event| {
        matches!(
            event,
            SessionEvent::ToolCall { status, .. } if *status == ToolStatus::Completed
        )
    });
    assert!(completed, "the tool completion was not mapped");
}

#[test]
fn answers_a_permission_request() {
    let Some(client) = start("permission") else {
        eprintln!("skipped: the ACP endpoint is not available on this machine");
        return;
    };

    let session = client
        .new_session(std::path::Path::new("C:\\repo"))
        .expect("session/new is answered");
    client
        .start_prompt(&session.session_id, "apply the diff")
        .expect("session/prompt is written");

    // The endpoint asks the client before the turn finishes, so the answer has
    // to come from the event stream.
    let events = wait_for(&client, Duration::from_secs(20), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::PermissionRequested { .. }))
    });
    let request = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::PermissionRequested {
                request_id,
                request,
            } => Some((*request_id, request.clone())),
            _ => None,
        })
        .expect("the permission request was seen");
    assert_eq!(request.1.tool_call_id, "call-9");
    assert_eq!(request.1.title, "apply_diff");
    assert_eq!(request.1.options.len(), 2);
    assert_eq!(request.1.options[0].option_id, "allow");

    client
        .respond_to_permission(request.0, PermissionOutcome::selected("allow"))
        .expect("the permission answer is written");

    let events = wait_for(&client, Duration::from_secs(20), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::TurnCompleted { .. }))
    });
    let reported = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Message { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert!(
        reported.contains("decision: allow"),
        "the endpoint did not receive the approval, got: {reported}"
    );
}

#[test]
fn cancels_a_running_turn() {
    let Some(client) = start("slow") else {
        eprintln!("skipped: the ACP endpoint is not available on this machine");
        return;
    };

    let session = client
        .new_session(std::path::Path::new("C:\\repo"))
        .expect("session/new is answered");
    client
        .start_prompt(&session.session_id, "take your time")
        .expect("session/prompt is written");

    // The turn streams its first chunk and then waits.
    let events = wait_for(&client, Duration::from_secs(20), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::Message { .. }))
    });
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::Message { .. })),
        "the turn never started"
    );

    client
        .cancel(&session.session_id)
        .expect("session/cancel is written");
}
