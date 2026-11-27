//! The debugger against an adapter that is real: debugpy.
//!
//! Every other check of the debugger is against the adapter of the tests,
//! and that adapter is written by the same hand as the client: it can only
//! ever agree with it. This check is against debugpy, the adapter that is
//! really used for Python, and it is the one that shows whether the editor
//! speaks the protocol the way an adapter expects.
//!
//! Two things had to be right for this to work, and both of them are things
//! the adapter of the tests could never have shown:
//!
//! - the order of the requests: the program starts before the breakpoints
//!   go, because debugpy says that it is ready only after it knows how the
//!   program is to be started;
//! - the path of the file: the adapter reads the program from the disk, and
//!   the address that the editor keeps a file under (`file:///...`) is of no
//!   use to it.
//!
//! The check needs a Python that has debugpy in it. It is not run when there
//! is none: an answer that was not looked for is not an answer.
//!
//! ```text
//! set CODEWEAVER_DEBUGPY_PYTHON=C:\...\python.exe
//! cargo test -p codeweaver-platform --test real_adapter
//! ```
//!
//! The check is run from outside the workspace: the workspace carries a low
//! integrity label, and a low integrity process cannot start the Python of
//! the machine. See `tools/dap-real-check.ps1`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use codeweaver_dap::AdapterConfig;
use codeweaver_platform::debug::{DebugState, Debugger};

/// The program that is debugged.
///
/// The breakpoint is on the third line: the program has worked the answer out
/// and is about to return it.
const PROGRAM: &str = "\
def work(value):
    total = value * 2
    return total
answer = work(21)
print(\"the program ran\", answer)
";

/// The line the program stops at.
const LINE: u32 = 3;

/// The Python that has debugpy in it, when the machine has one.
fn python() -> Option<String> {
    match std::env::var("CODEWEAVER_DEBUGPY_PYTHON") {
        Ok(path) if !path.trim().is_empty() => Some(path),
        _ => {
            eprintln!(
                "BLOCKED: CODEWEAVER_DEBUGPY_PYTHON is not set; the check of the real adapter is \
                 not run at all. Run it from a copy of the test binary outside the workspace: \
                 tools\\dap-real-check.ps1"
            );
            None
        }
    }
}

/// The program that is debugged, in the folder of the crate.
///
/// The file goes into the `target` of the crate and not into the temporary
/// folder of the machine: the temporary folder is of the ordinary level, and
/// a check that runs at the low level cannot write into it.
fn write_the_program() -> PathBuf {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("dap-real");
    std::fs::create_dir_all(&folder).expect("the folder is made");
    let path = folder.join("program.py");
    std::fs::write(&path, PROGRAM).expect("the program is written");
    path
}

/// The breakpoints of the editor, by the address of the file they are in.
type Breakpoints = Vec<(String, Vec<(u32, Option<String>)>)>;

/// Pumps the session until something is true, and gives up after a while.
fn until(
    debugger: &mut Debugger,
    breakpoints: &Breakpoints,
    what: &str,
    mut ready: impl FnMut(&Debugger) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !ready(debugger) {
        debugger.pump(breakpoints);
        assert!(
            Instant::now() < deadline,
            "the {what} did not come:\n{}",
            debugger.output.join("\n")
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the adapter said, without the spaces.
///
/// The adapter may say a thing in pieces: what is said is put together, and
/// the spaces are of no use when what is looked for is a word.
fn said_without_spaces(debugger: &Debugger) -> String {
    debugger
        .output
        .iter()
        .flat_map(|line| line.chars())
        .filter(|character| !character.is_whitespace())
        .collect()
}

#[test]
fn the_program_stops_at_the_breakpoint_of_a_real_adapter() {
    let Some(python) = python() else {
        return;
    };
    let program = write_the_program();
    let config = AdapterConfig::new(
        python,
        vec!["-m".to_owned(), "debugpy.adapter".to_owned()],
    );
    let mut debugger = Debugger::start(&config, &program).expect("the session starts");

    // The breakpoints are given as the editor gives them: by the address of
    // the file. The session turns them into paths for the adapter.
    let breakpoints = vec![(
        codeweaver_lsp::protocol::file_uri(&program),
        vec![(LINE, None)],
    )];

    until(&mut debugger, &breakpoints, "stop", |debugger| {
        debugger.state == DebugState::Stopped
    });
    until(&mut debugger, &breakpoints, "stack", |debugger| {
        !debugger.frames.is_empty()
    });
    until(&mut debugger, &breakpoints, "variables", |debugger| {
        !debugger.variables.is_empty()
    });

    // The program stopped in the file that is debugged, on the line of the
    // breakpoint.
    let frame = debugger.frames.first().expect("the stack is known");
    assert_eq!(frame.line, Some(LINE), "the frame is on the breakpoint");
    let source = frame.source.as_ref().expect("the frame is in a file");
    let path = source.path.as_deref().unwrap_or_default();
    assert!(
        path.ends_with("program.py"),
        "the frame is in the program that is debugged: {path}"
    );

    // The variable of the frame is what the program had worked out.
    let total = debugger
        .variables
        .iter()
        .find(|variable| variable.name == "total")
        .expect("the variable is there");
    assert_eq!(total.value, "42", "the program worked the answer out");

    // The program runs on, and says what it has to say.
    //
    // The adapter may say it in pieces: what is said is put together, and
    // the spaces are of no use here.
    debugger
        .client
        .continue_(debugger.thread_id)
        .expect("the program runs on");
    until(&mut debugger, &breakpoints, "output", |debugger| {
        said_without_spaces(debugger).contains("theprogramran42")
    });
    until(&mut debugger, &breakpoints, "end", |debugger| {
        debugger.state == DebugState::Finished
    });

    // The talk of the adapter about itself is not for the person: the panel
    // of the debugger is not the place for the telemetry of debugpy.
    assert!(
        !debugger
            .output
            .iter()
            .any(|line| line.contains("ptvsd") || line.contains("debugpy")),
        "the panel shows only what is for the person: {:?}",
        debugger.output
    );

    debugger.client.shutdown();
}
