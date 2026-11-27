//! The order in which the requests of a session go to the adapter.
//!
//! The order is not the one the document of the protocol writes down, and the
//! difference matters. The document says that the adapter says that it is
//! ready, and that the breakpoints go then. debugpy says that it is ready
//! only after it knows how the program is to be started, so the program has
//! to be started *before* the breakpoints go. VS Code does the same: it
//! starts the program right after the hello, and puts the breakpoints in
//! when the adapter says that it is ready.
//!
//! This module is that order, and nothing else. It is a small state machine
//! that says which requests are due now; the sending of them is the job of
//! the client. That way the order can be checked without an adapter, and the
//! rules of it are in one place instead of spread over the window.
//!
//! The order is:
//!
//! 1. `initialize` — the hello.
//! 2. `launch` — the program starts, once the adapter has answered the hello.
//! 3. `setBreakpoints` — the breakpoints go, once the adapter has said that
//!    it is ready for them.
//! 4. `configurationDone` — everything is set up, and the program runs.

use crate::protocol::{BreakpointRequest, LaunchRequest};

/// One request that goes to the adapter.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Say hello.
    Initialize,
    /// Start the program.
    Launch(LaunchRequest),
    /// Tell the adapter where the breakpoints of one file are.
    SetBreakpoints {
        /// The path of the file, as the adapter is to read it.
        ///
        /// It is a path and not an address of the editor: an adapter reads
        /// the file of the program from the disk, and a `file://` address is
        /// of no use to it.
        path: String,
        /// The breakpoints of the file.
        breakpoints: Vec<BreakpointRequest>,
    },
    /// Everything is set up.
    ConfigurationDone,
}

/// The order of the requests of a session.
///
/// The state machine keeps what the adapter has already said, and says which
/// requests are due now. Every method gives the steps that go out at that
/// moment; a step that is already gone is never given twice.
#[derive(Clone, Debug, Default)]
pub struct Handshake {
    /// Whether the adapter answered the hello.
    answered: bool,
    /// Whether the adapter said that it is ready for the breakpoints.
    ready: bool,
    /// Whether the breakpoints went and the setup is done.
    configured: bool,
}

impl Handshake {
    /// A handshake that has not begun yet.
    pub fn new() -> Self {
        Handshake::default()
    }

    /// The step that goes out first.
    pub fn first(&self) -> Step {
        Step::Initialize
    }

    /// The steps that go out when the adapter answered the hello.
    ///
    /// The program starts now: an adapter that is to be told where the
    /// breakpoints are needs to know how the program is started first.
    pub fn after_the_hello(
        &mut self,
        launch: &LaunchRequest,
        breakpoints: &[(String, Vec<BreakpointRequest>)],
    ) -> Vec<Step> {
        if self.answered {
            // The adapter answered once; the hello is not asked again.
            return Vec::new();
        }
        self.answered = true;
        let mut steps = vec![Step::Launch(launch.clone())];
        steps.extend(self.configure(breakpoints));
        steps
    }

    /// The steps that go out when the adapter said that it is ready.
    ///
    /// The breakpoints go first, and then the setup is done.
    pub fn when_it_is_ready(
        &mut self,
        breakpoints: &[(String, Vec<BreakpointRequest>)],
    ) -> Vec<Step> {
        if self.ready {
            // The adapter said it is ready once; the breakpoints are not
            // sent again because of it.
            return Vec::new();
        }
        self.ready = true;
        self.configure(breakpoints)
    }

    /// The breakpoints and the end of the setup, when both are due.
    ///
    /// Both are due when the adapter has answered the hello and has said
    /// that it is ready. An adapter that says that it is ready before it
    /// answers the hello gets its breakpoints right after the launch.
    fn configure(&mut self, breakpoints: &[(String, Vec<BreakpointRequest>)]) -> Vec<Step> {
        if self.configured || !self.ready || !self.answered {
            return Vec::new();
        }
        self.configured = true;
        let mut steps: Vec<Step> = breakpoints
            .iter()
            .map(|(path, list)| Step::SetBreakpoints {
                path: path.clone(),
                breakpoints: list.clone(),
            })
            .collect();
        steps.push(Step::ConfigurationDone);
        steps
    }

    /// Whether the setup is done.
    pub fn is_configured(&self) -> bool {
        self.configured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A launch of a program.
    fn launch() -> LaunchRequest {
        LaunchRequest::new("C:\\work\\main.py")
    }

    /// The breakpoints of one file.
    fn breakpoints() -> Vec<(String, Vec<BreakpointRequest>)> {
        vec![(
            "C:\\work\\main.py".to_owned(),
            vec![(4, None), (7, Some("count > 3".to_owned()))],
        )]
    }

    /// The names of the steps, so that the order can be said in one line.
    fn names(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .map(|step| match step {
                Step::Initialize => "initialize".to_owned(),
                Step::Launch(_) => "launch".to_owned(),
                Step::SetBreakpoints { path, .. } => format!("setBreakpoints {path}"),
                Step::ConfigurationDone => "configurationDone".to_owned(),
            })
            .collect()
    }

    #[test]
    fn the_hello_goes_first() {
        let handshake = Handshake::new();
        assert_eq!(handshake.first(), Step::Initialize);
    }

    #[test]
    fn the_program_starts_after_the_hello() {
        let mut handshake = Handshake::new();
        let steps = handshake.after_the_hello(&launch(), &breakpoints());
        assert_eq!(names(&steps), vec!["launch"]);
    }

    #[test]
    fn the_breakpoints_go_when_the_adapter_is_ready() {
        let mut handshake = Handshake::new();
        handshake.after_the_hello(&launch(), &breakpoints());

        let steps = handshake.when_it_is_ready(&breakpoints());
        assert_eq!(
            names(&steps),
            vec![
                "setBreakpoints C:\\work\\main.py",
                "configurationDone"
            ]
        );
    }

    #[test]
    fn the_whole_order_is_the_one_of_vscode() {
        let mut handshake = Handshake::new();
        let mut order = names(&[handshake.first()]);
        order.extend(names(&handshake.after_the_hello(&launch(), &breakpoints())));
        order.extend(names(&handshake.when_it_is_ready(&breakpoints())));

        assert_eq!(
            order,
            vec![
                "initialize",
                "launch",
                "setBreakpoints C:\\work\\main.py",
                "configurationDone",
            ]
        );
    }

    #[test]
    fn an_adapter_that_is_ready_before_it_answers_gets_the_breakpoints_at_once() {
        let mut handshake = Handshake::new();
        // The adapter says that it is ready before it answers the hello.
        let steps = handshake.when_it_is_ready(&breakpoints());
        assert!(steps.is_empty(), "the program has not started yet: {steps:?}");

        let steps = handshake.after_the_hello(&launch(), &breakpoints());
        assert_eq!(
            names(&steps),
            vec![
                "launch",
                "setBreakpoints C:\\work\\main.py",
                "configurationDone",
            ]
        );
    }

    #[test]
    fn the_breakpoints_are_not_sent_twice() {
        let mut handshake = Handshake::new();
        handshake.after_the_hello(&launch(), &breakpoints());
        handshake.when_it_is_ready(&breakpoints());

        let steps = handshake.when_it_is_ready(&breakpoints());
        assert!(steps.is_empty(), "the setup is done already: {steps:?}");
    }

    #[test]
    fn the_hello_is_not_answered_twice() {
        let mut handshake = Handshake::new();
        handshake.after_the_hello(&launch(), &breakpoints());
        let steps = handshake.after_the_hello(&launch(), &breakpoints());
        assert!(steps.is_empty(), "the program starts once: {steps:?}");
    }

    #[test]
    fn a_file_without_breakpoints_is_not_mentioned() {
        let mut handshake = Handshake::new();
        handshake.after_the_hello(&launch(), &[]);
        let steps = handshake.when_it_is_ready(&[]);
        assert_eq!(names(&steps), vec!["configurationDone"]);
    }

    #[test]
    fn the_setup_is_done_when_the_breakpoints_went() {
        let mut handshake = Handshake::new();
        assert!(!handshake.is_configured());
        handshake.after_the_hello(&launch(), &breakpoints());
        handshake.when_it_is_ready(&breakpoints());
        assert!(handshake.is_configured());
    }

    #[test]
    fn the_launch_that_goes_out_is_the_one_that_was_asked_for() {
        let mut handshake = Handshake::new();
        let steps = handshake.after_the_hello(&launch(), &breakpoints());
        let step = steps.first().expect("the program starts");
        match step {
            Step::Launch(request) => assert_eq!(request.program, "C:\\work\\main.py"),
            other => panic!("the first step is not the launch: {other:?}"),
        }
    }

    #[test]
    fn the_breakpoints_that_go_out_carry_their_conditions() {
        let mut handshake = Handshake::new();
        handshake.after_the_hello(&launch(), &breakpoints());
        let steps = handshake.when_it_is_ready(&breakpoints());
        let step = steps.first().expect("the breakpoints go");
        match step {
            Step::SetBreakpoints {
                path, breakpoints, ..
            } => {
                assert_eq!(path, "C:\\work\\main.py");
                assert_eq!(breakpoints.len(), 2);
                assert_eq!(breakpoints[0], (4, None));
                assert_eq!(breakpoints[1].1.as_deref(), Some("count > 3"));
            }
            other => panic!("the first step is not the breakpoints: {other:?}"),
        }
    }
}
