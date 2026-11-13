//! The client against a debug adapter that is not real.
//!
//! The fake adapter answers the handshake, keeps the breakpoints it is told
//! about, and runs a program that stops at the first one. These tests are
//! about the wire: the frames, the order of the messages and the text that
//! arrives.
//!
//! The bodies of the Debug Adapter Protocol keep the list they carry under a
//! name of its own — `breakpoints`, `stackFrames`, `scopes` and `variables` —
//! so the client has to dig the list out of the body of the answer.

use std::time::{Duration, Instant};

use codeweaver_dap::{AdapterConfig, Client, DapEvent};

/// Starts the fake adapter and says hello to it.
fn client() -> Client {
    let config = AdapterConfig::new(env!("CARGO_BIN_EXE_fake-dap-adapter"), Vec::new());
    let client = Client::start(&config).expect("the adapter starts");
    client.initialize().expect("the handshake goes through");
    client
}

/// Starts the fake adapter, sets a breakpoint, and starts the program.
fn running_client() -> Client {
    let client = client();
    client
        .set_breakpoints("file:///tmp/main.rs", &[7])
        .expect("the breakpoints go through");
    client
        .configuration_done()
        .expect("the setup goes through");
    client.launch("main", &[]).expect("the program starts");
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
            "the debug adapter did not answer in time"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The events of the client, until one of them is the one that is waited for.
fn events_until(client: &Client, wanted: impl Fn(&DapEvent) -> bool) -> Vec<DapEvent> {
    let mut found = Vec::new();
    wait_for(|| {
        for event in client.drain_events() {
            found.push(event);
        }
        found.iter().any(&wanted).then(|| found.clone())
    })
}

#[test]
fn the_client_says_hello_and_the_adapter_answers() {
    let client = client();
    wait_for(|| client.is_ready().then_some(()));

    assert!(client.is_ready());
    assert!(client.is_running());

    // The adapter says that it is ready for the breakpoints.
    let events = events_until(&client, |event| matches!(event, DapEvent::Initialized));
    assert!(events.contains(&DapEvent::Initialized));
}

#[test]
fn a_breakpoint_is_placed_where_it_was_asked_for() {
    let client = client();
    client
        .set_breakpoints("file:///tmp/main.rs", &[7])
        .expect("the breakpoints go through");

    let answer = wait_for(|| {
        let mut answers = client.drain_breakpoints();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let breakpoints = answer.expect("the adapter answered");
    assert_eq!(breakpoints.len(), 1);
    assert!(breakpoints[0].verified);
    assert_eq!(breakpoints[0].line, Some(7));
}

#[test]
fn the_program_stops_at_the_breakpoint() {
    let client = running_client();

    let events = events_until(&client, |event| {
        matches!(event, DapEvent::Stopped { .. })
    });
    let stopped = events
        .iter()
        .find_map(|event| match event {
            DapEvent::Stopped {
                reason, thread_id, ..
            } => Some((reason.clone(), *thread_id)),
            _ => None,
        })
        .expect("the program stopped");
    assert_eq!(stopped.0, "breakpoint");
    assert_eq!(stopped.1, 1);
}

#[test]
fn the_stack_of_the_stopped_program_is_known() {
    let client = running_client();
    events_until(&client, |event| matches!(event, DapEvent::Stopped { .. }));

    client.stack_trace(1).expect("the request goes through");
    let answer = wait_for(|| {
        let mut answers = client.drain_stack_traces();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let frames = answer.expect("the adapter answered");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].name, "main");
    // The frame is on the line of the breakpoint.
    assert_eq!(frames[0].line, Some(7));
}

#[test]
fn the_variables_of_the_frame_are_known() {
    let client = running_client();
    events_until(&client, |event| matches!(event, DapEvent::Stopped { .. }));

    client.scopes(1).expect("the request goes through");
    let answer = wait_for(|| {
        let mut answers = client.drain_scopes();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let scopes = answer.expect("the adapter answered");
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].variables_reference, 10);

    client
        .variables(scopes[0].variables_reference)
        .expect("the request goes through");
    let answer = wait_for(|| {
        let mut answers = client.drain_variables();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let variables = answer.expect("the adapter answered");
    assert_eq!(variables.len(), 2);
    assert_eq!(variables[0].name, "count");
    assert_eq!(variables[0].value, "3");
    assert_eq!(variables[0].type_.as_deref(), Some("i32"));
    assert_eq!(variables[1].name, "name");
    assert_eq!(variables[1].value, "greeting");
}

#[test]
fn the_output_of_the_program_arrives() {
    let client = running_client();

    let events = events_until(&client, |event| matches!(event, DapEvent::Output { .. }));
    let output = events
        .iter()
        .find_map(|event| match event {
            DapEvent::Output { category, text } => Some((category.clone(), text.clone())),
            _ => None,
        })
        .expect("the program said something");
    assert_eq!(output.0, "stdout");
    assert!(output.1.contains("the program started"), "{}", output.1);
}

#[test]
fn the_session_ends_when_the_program_is_terminated() {
    let client = running_client();
    client.terminate().expect("the session ends");

    let events = events_until(&client, |event| {
        matches!(event, DapEvent::Terminated)
    });
    assert!(events.contains(&DapEvent::Terminated));
}
