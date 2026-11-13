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
fn the_condition_of_a_breakpoint_reaches_the_adapter() {
    let client = client();
    client
        .set_breakpoints_with_conditions(
            "file:///tmp/main.rs",
            &[(7, Some("count > 3".to_owned())), (9, None)],
        )
        .expect("the breakpoints go through");

    let answer = wait_for(|| {
        let mut answers = client.drain_breakpoints();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let breakpoints = answer.expect("the adapter answered");
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0].line, Some(7));
    assert!(
        breakpoints[0]
            .message
            .as_deref()
            .is_some_and(|message| message.contains("count > 3")),
        "the adapter was told the condition: {:?}",
        breakpoints[0].message
    );
    assert!(
        breakpoints[1].message.is_none(),
        "a breakpoint without a condition has none: {:?}",
        breakpoints[1].message
    );
}

#[test]
fn an_expression_is_worth_what_the_adapter_says() {
    let client = client();
    client
        .evaluate("count + 1", 1)
        .expect("the expression goes through");

    let answer = wait_for(|| {
        let mut answers = client.drain_evaluations();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let value = answer.expect("the adapter answered");
    assert_eq!(value.result, "42");
    assert_eq!(value.type_.as_deref(), Some("i32"));
    assert_eq!(value.line("count + 1"), "count + 1: i32 = 42");
}

#[test]
fn an_expression_the_adapter_refuses_is_an_error() {
    let client = client();
    // The adapter looks at the expression: an empty one is refused. That is
    // how the contract proves that the expression was sent at all.
    client.evaluate("", 1).expect("the request goes through");

    let answer = wait_for(|| {
        let mut answers = client.drain_evaluations();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let error = answer.expect_err("the adapter refused");
    assert!(
        error.to_string().contains("empty"),
        "the reason has to be said out loud: {error}"
    );
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
    // The stack has more than one frame: the editor shows the whole stack.
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].name, "main");
    // The frame is on the line of the breakpoint.
    assert_eq!(frames[0].line, Some(7));
    assert_eq!(frames[1].name, "helper");
    assert_eq!(frames[2].name, "outer");
    // The caller is in another file: the editor has to be able to open the
    // file of a frame.
    assert_eq!(
        frames[1]
            .source
            .as_ref()
            .and_then(|source| source.path.as_deref()),
        Some("file:///tmp/helper.rs")
    );
    // Every frame has a number of its own: the variables are asked for by
    // it.
    assert_ne!(frames[0].id, frames[1].id);
    assert_ne!(frames[1].id, frames[2].id);
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
    let (frame_id, scopes) = answer.expect("the adapter answered");
    // The answer says the frame the groups belong to.
    assert_eq!(frame_id, 1);
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].variables_reference, 101);

    client
        .variables(1, scopes[0].variables_reference)
        .expect("the request goes through");
    let answer = wait_for(|| {
        let mut answers = client.drain_variables();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let (frame_id, variables) = answer.expect("the adapter answered");
    // The answer says the frame the variables belong to.
    assert_eq!(frame_id, 1);
    assert_eq!(variables.len(), 2);
    assert_eq!(variables[0].name, "count");
    assert_eq!(variables[0].value, "3");
    assert_eq!(variables[0].type_.as_deref(), Some("i32"));
    assert_eq!(variables[1].name, "name");
    assert_eq!(variables[1].value, "greeting");
}

#[test]
fn the_variables_of_another_frame_are_known() {
    let client = running_client();
    events_until(&client, |event| matches!(event, DapEvent::Stopped { .. }));

    // The second frame of the stack has variables of its own.
    client.scopes(2).expect("the request goes through");
    let answer = wait_for(|| {
        let mut answers = client.drain_scopes();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let (frame_id, scopes) = answer.expect("the adapter answered");
    assert_eq!(frame_id, 2);
    assert_eq!(scopes[0].variables_reference, 102);

    client
        .variables(2, scopes[0].variables_reference)
        .expect("the request goes through");
    let answer = wait_for(|| {
        let mut answers = client.drain_variables();
        (!answers.is_empty()).then(|| answers.remove(0))
    });
    let (frame_id, variables) = answer.expect("the adapter answered");
    assert_eq!(frame_id, 2);
    assert_eq!(variables[0].name, "total");
    assert_eq!(variables[0].value, "6");
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
