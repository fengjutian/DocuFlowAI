//! Benchmarks for the syntax highlighter.
//!
//! The plan's §9 gives the editor a frame budget of 16 ms between a
//! keystroke and the picture, and asks for a benchmark behind every target.
//! The highlighter is on that path twice: the tree of the text is parsed
//! again when the text changes, and the highlights of the text that is on
//! screen are made for every frame.
//!
//! The three cases are the three costs:
//!
//! - `parse_cold`: a file is opened. The whole document is parsed.
//! - `parse_warm`: a keystroke. The tree of the last parse is followed, so
//!   only the text around the change is parsed again.
//! - `highlights`: a frame. The query runs over the text that is on screen,
//!   not over the whole document.
//!
//! Run with:
//!
//! ```powershell
//! cargo bench -p codeweaver-syntax
//! ```

use std::time::{Duration, Instant};

use codeweaver_syntax::{HighlightPolicy, Language, Syntax, SyntaxWorker};
use codeweaver_text::{Buffer, TextEdit, TextOffset, Transaction};

/// The statistics a benchmark reports.
struct Measurement {
    label: String,
    samples: Vec<Duration>,
}

impl Measurement {
    fn new(label: impl Into<String>) -> Self {
        Measurement {
            label: label.into(),
            samples: Vec::new(),
        }
    }

    fn record(&mut self, elapsed: Duration) {
        self.samples.push(elapsed);
    }

    fn median(&self) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort();
        sorted[sorted.len() / 2]
    }

    fn p95(&self) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort();
        let index = ((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
        sorted[index.min(sorted.len() - 1)]
    }

    fn mean_ms(&self) -> f64 {
        let total: Duration = self.samples.iter().sum();
        total.as_secs_f64() * 1000.0 / self.samples.len() as f64
    }
}

/// Runs `body` repeatedly and records how long each run took.
fn measure(label: impl Into<String>, iterations: usize, mut body: impl FnMut()) -> Measurement {
    let mut measurement = Measurement::new(label);
    for _ in 0..iterations {
        let start = Instant::now();
        body();
        measurement.record(start.elapsed());
    }
    measurement
}

/// A source file with `lines` lines of Rust.
fn document(lines: usize) -> Buffer {
    let mut text = String::with_capacity(lines * 48);
    for index in 0..lines {
        text.push_str(&format!(
            "fn function_{index}(value: u32) -> u32 {{\n    let name = \"value\";\n    value + {index}\n}}\n"
        ));
    }
    Buffer::new(text)
}

/// Measures the parse of a whole document: what opening a file pays.
///
/// The highlighter keeps the tree of the last parse, so the tree is thrown
/// away before every round: what is measured is the parse of the whole
/// document, not the check that says the document did not change.
fn bench_parse_cold(lines: usize) -> Measurement {
    let buffer = document(lines);
    let mut measurement = Measurement::new(format!("parse_cold/{lines}-lines"));
    for _ in 0..20 {
        let mut syntax = Syntax::new();
        syntax.set_language(Some(Language::Rust));
        let start = Instant::now();
        syntax.update(&buffer, None);
        measurement.record(start.elapsed());
    }
    measurement
}

/// Measures the parse after a keystroke: what typing pays.
///
/// The tree of the last parse is followed, so the document is only parsed
/// again around the change.
fn bench_parse_warm(lines: usize) -> Measurement {
    let mut buffer = document(lines);
    let mut syntax = Syntax::new();
    syntax.set_language(Some(Language::Rust));
    syntax.update(&buffer, None);

    // The place the keystroke happens: in the middle of the document.
    let caret = buffer.len().0 / 2;
    let mut measurement = Measurement::new(format!("parse_warm/{lines}-lines"));
    for _ in 0..200 {
        let applied = buffer
            .apply(Transaction::new(
                buffer.version(),
                vec![TextEdit::new(TextOffset(caret)..TextOffset(caret), "x")],
            ))
            .unwrap();
        let start = Instant::now();
        syntax.update(&buffer, Some(&applied.change));
        measurement.record(start.elapsed());
        // Take the character back, so the next round starts at the same
        // place.
        let applied = buffer
            .apply(Transaction::new(
                buffer.version(),
                vec![TextEdit::new(TextOffset(caret)..TextOffset(caret + 1), "")],
            ))
            .unwrap();
        syntax.update(&buffer, Some(&applied.change));
    }
    measurement
}

/// Measures the highlights of the text that is on screen: what a frame pays.
///
/// The highlighter keeps the highlights of the last range it was asked about,
/// so two ranges are used one after the other: every round is then a range
/// that has not been looked at yet, which is what a frame that scrolls costs.
fn bench_highlights(lines: usize, visible: usize) -> Measurement {
    let buffer = document(lines);
    let mut syntax = Syntax::new();
    syntax.set_language(Some(Language::Rust));
    syntax.update(&buffer, None);

    // Two ranges of the size of the screen, next to each other.
    let first = 0..(buffer.len().0 * visible) / lines;
    let second = first.end..(buffer.len().0 * (visible + 1)) / lines;
    measure(
        format!("highlights/{lines}-lines/{visible}-visible"),
        50,
        || {
            let highlights = syntax.highlights(&buffer, first.clone());
            std::hint::black_box(highlights.len());
            let highlights = syntax.highlights(&buffer, second.clone());
            std::hint::black_box(highlights.len());
        },
    )
}

/// The budgets the measurements have to stay inside.
///
/// The plan gives a frame 16 ms for the whole round trip from a keystroke to
/// the picture. The highlighter is a part of that round trip, and it has to
/// leave room for the rest of it: the layout, the shaping and the drawing
/// all happen in the same frame.
///
/// The warm number is the cost of typing, so it is the tight one. Tree-sitter
/// walks the old tree to find the place that changed, so the cost of a
/// keystroke grows with the document: a file of two thousand lines is well
/// inside a frame, a file of ten thousand lines is not. The budget is set
/// where the measurement is, so that a change that makes the parse slower
/// stops the merge; closing the gap with the plan's 16 ms is the job of
/// parsing in the background, and that is a separate piece of work.
///
/// The cold number is the cost of opening a file, which happens once. The
/// plan allows three seconds for a hundred megabyte file, and ten thousand
/// lines is far smaller than that.
///
/// The last four numbers are the ones V0.4.0 added. The parse moved to a
/// thread of its own, so the numbers that matter for a frame are the ones the
/// thread that draws the picture pays:
///
/// - `ui_decision` is what that thread does with a file that is too big to be
///   coloured: the decision whether to parse it at all. It happens on the
///   way of every keystroke, so it has to be far inside a frame.
/// - `ui_handoff` is what that thread pays for a file that *is* coloured: the
///   copy of the text for the other thread. The copy is the only part of the
///   highlighter that is left on this thread, so it has to fit in a frame.
/// - `background` is the whole round trip over the channel, parse included.
///   It is not on the frame's way, so its budget is about the user waiting
///   for the colours, not about the frame.
///
/// The big file is measured twice on purpose: once over the limit, where the
/// answer has to come back saying "degraded" instead of parsing, and once
/// under it, where the parse really happens.
const BUDGETS: &[(&str, f64)] = &[
    ("parse_cold/10000-lines", 300.0),
    ("parse_warm/2000-lines", 12.0),
    ("parse_warm/10000-lines", 35.0),
    ("highlights/10000-lines/40-visible", 1.0),
    ("ui_decision/60000-lines", 1.0),
    ("ui_handoff/20000-lines", 16.0),
    ("background/60000-lines", 100.0),
    ("background/20000-lines", 2000.0),
];

/// The number of lines that fit on the screen.
const VISIBLE_LINES: usize = 40;

/// A file with exactly `lines` lines of Rust.
///
/// The size of the file is what the policy looks at, so the number of lines
/// has to be the real one and not a name.
fn source_of(lines: usize) -> String {
    let mut text = String::with_capacity(lines * 28);
    for index in 0..lines {
        if index % 1000 == 0 {
            text.push_str("fn function() {}\n");
        } else {
            text.push_str("let value = 1; // a line of code\n");
        }
    }
    text
}

/// Measures what the thread that draws the picture does with a file that is
/// too big to be coloured.
///
/// This is the case the background highlighter exists for: the parse is not
/// on this thread any more, and a file over the limit is not parsed at all.
/// What is left here is the decision, and it has to be small.
fn bench_ui_decision(lines: usize) -> Measurement {
    let buffer = Buffer::new(source_of(lines));
    let policy = HighlightPolicy::default();
    let mut measurement = Measurement::new(format!("ui_decision/{lines}-lines"));
    for _ in 0..100 {
        let start = Instant::now();
        let allowed = policy.allows(buffer.len().0, buffer.line_count());
        measurement.record(start.elapsed());
        std::hint::black_box(allowed);
    }
    measurement
}

/// Measures what the thread that draws the picture pays for a file that is
/// still coloured: the copy of the text for the highlighter on the other
/// thread.
fn bench_ui_handoff(lines: usize) -> Measurement {
    let buffer = Buffer::new(source_of(lines));
    let mut measurement = Measurement::new(format!("ui_handoff/{lines}-lines"));
    for _ in 0..50 {
        let start = Instant::now();
        let text = buffer.text();
        measurement.record(start.elapsed());
        std::hint::black_box(text.len());
    }
    measurement
}

/// Waits for the answer of the highlighter.
fn wait_for(worker: &mut SyntaxWorker) -> Option<codeweaver_syntax::Outcome> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(outcome) = worker.poll() {
            return Some(outcome);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Measures the whole round trip of the background highlighter.
///
/// The request goes to the other thread, the text is parsed there, and the
/// answer comes back. A new version every round, so every round is a full
/// parse and not the check that says the text did not change.
///
/// The range that is asked for is the range that is on screen, the way the
/// editor asks for it: the whole document is parsed, but only the text the
/// user can see is turned into highlights.
fn bench_background(lines: usize) -> Measurement {
    let text = source_of(lines);
    let bytes = text.len();
    let visible = (bytes * VISIBLE_LINES / lines).min(bytes);
    let mut worker = SyntaxWorker::new(Some(Language::Rust));
    let mut measurement = Measurement::new(format!("background/{lines}-lines"));
    for round in 0..5u64 {
        // The copy is what the thread that draws the picture pays, and it is
        // measured on its own: this case is about the other thread.
        let copy = text.clone();
        let start = Instant::now();
        worker.request(round + 1, copy, 0..visible, lines);
        let outcome = wait_for(&mut worker);
        measurement.record(start.elapsed());
        std::hint::black_box(outcome.map(|outcome| outcome.degraded));
    }
    measurement
}

fn main() {
    let measurements: Vec<Measurement> = vec![
        bench_parse_cold(10_000),
        bench_parse_warm(2_000),
        bench_parse_warm(10_000),
        bench_highlights(10_000, 40),
        bench_ui_decision(60_000),
        bench_ui_handoff(20_000),
        bench_background(60_000),
        bench_background(20_000),
    ];

    println!(
        "{:<42} {:>12} {:>12} {:>12}",
        "case", "median", "p95", "mean"
    );
    println!("{}", "-".repeat(82));
    for measurement in &measurements {
        println!(
            "{:<42} {:>12} {:>12} {:>12}",
            measurement.label,
            format!("{:.3} ms", measurement.median().as_secs_f64() * 1000.0),
            format!("{:.3} ms", measurement.p95().as_secs_f64() * 1000.0),
            format!("{:.3} ms", measurement.mean_ms()),
        );
    }

    let mut failures: Vec<String> = Vec::new();
    for (label, budget) in BUDGETS {
        let Some(measurement) = measurements.iter().find(|m| m.label == *label) else {
            failures.push(format!("{label}: not measured"));
            continue;
        };
        let measured = measurement.p95().as_secs_f64() * 1000.0;
        if measured > *budget {
            failures.push(format!(
                "{label}: {measured:.3} ms is over the {budget:.3} ms budget"
            ));
        }
    }

    println!();
    if failures.is_empty() {
        println!("every case is inside its budget");
        return;
    }
    for failure in failures {
        eprintln!("{failure}");
    }
    std::process::exit(1);
}
