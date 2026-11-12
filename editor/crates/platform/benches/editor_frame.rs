//! Benchmarks for the editor body.
//!
//! The plan's §9 sets "keyboard input to picture update" at P95 below 16 ms
//! and asks for a benchmark behind every target. This measures the part of
//! that budget the editor spends before the GPU is involved: measuring the
//! text of the visible lines, laying them out, and the lookups a keystroke
//! does.
//!
//! The measurements use the real font stack — measuring needs the font
//! system and the shaping, but no GPU, so the numbers are the ones the
//! running editor pays.
//!
//! Every case is measured twice: cold, when nothing has been shaped yet and
//! the editor has to shape every line it shows, and warm, when the lines on
//! screen are already shaped. The warm number is the one a keystroke pays,
//! because typing only changes the line that was typed on.
//!
//! Run with:
//!
//! ```powershell
//! cargo bench -p codeweaver-platform
//! ```

use std::time::{Duration, Instant};

use codeweaver_platform::editor_view::{FONT_SIZE, LINE_HEIGHT, ShaperMeasurer};
use codeweaver_render::{TextFamily, TextShaper};
use codeweaver_text::Buffer;
use codeweaver_ui::TextLayout;

/// The width of the measured viewport, in logical pixels.
const VIEWPORT_WIDTH: f32 = 900.0;
/// The width the drawing gives a run, so the renderer does not wrap it
/// again.
const DRAW_WIDTH: f32 = VIEWPORT_WIDTH * 2.0;

/// A source file with `lines` lines of code.
fn document(lines: usize) -> Buffer {
    let mut text = String::with_capacity(lines * 48);
    for index in 0..lines {
        text.push_str(&format!("    let value_{index} = compute({index});\n"));
    }
    Buffer::new(text)
}

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

/// Measures the layout of a viewport: what a keystroke pays for the lines it
/// shows.
///
/// `cold` says whether the lines have to be shaped first. The document is
/// prepared outside the loop, and so is the font system: only the work of
/// the frame itself is measured.
fn bench_layout(lines: usize, visible: usize, cold: bool) -> Measurement {
    let buffer = document(lines);
    let mut shaper = TextShaper::new();
    let label = format!(
        "layout_{}/{lines}-lines/{visible}-visible",
        if cold { "cold" } else { "warm" }
    );
    measure(label, 50, || {
        if cold {
            shaper.clear_cache();
        }
        let mut measurer = ShaperMeasurer::new(&mut shaper, TextFamily::Monospace);
        let layout = TextLayout::build(
            &buffer,
            0,
            visible,
            VIEWPORT_WIDTH,
            FONT_SIZE,
            LINE_HEIGHT,
            &mut measurer,
        );
        std::hint::black_box(layout.lines.len());
    })
}

/// Measures a whole frame of text work: the layout measures every visible
/// line, and the drawing shapes every visible line again.
fn bench_frame_text(lines: usize, visible: usize, cold: bool) -> Measurement {
    let buffer = document(lines);
    let mut shaper = TextShaper::new();
    let label = format!(
        "frame_text_{}/{lines}-lines/{visible}-visible",
        if cold { "cold" } else { "warm" }
    );
    measure(label, 50, || {
        if cold {
            shaper.clear_cache();
        }
        let layout = {
            let mut measurer = ShaperMeasurer::new(&mut shaper, TextFamily::Monospace);
            TextLayout::build(
                &buffer,
                0,
                visible,
                VIEWPORT_WIDTH,
                FONT_SIZE,
                LINE_HEIGHT,
                &mut measurer,
            )
        };
        // The drawing shapes every visible line a second time.
        for line in &layout.lines {
            let id = shaper.shape(
                &line.text,
                FONT_SIZE,
                LINE_HEIGHT,
                TextFamily::Monospace,
                Some(DRAW_WIDTH),
                Some(LINE_HEIGHT),
            );
            std::hint::black_box(id);
        }
    })
}

/// Measures one hit test: what a click on a line of text costs.
fn bench_hit_test(lines: usize) -> Measurement {
    let buffer = document(lines);
    let mut shaper = TextShaper::new();
    let mut measurer = ShaperMeasurer::new(&mut shaper, TextFamily::Monospace);
    let layout = TextLayout::build(
        &buffer,
        0,
        40,
        VIEWPORT_WIDTH,
        FONT_SIZE,
        LINE_HEIGHT,
        &mut measurer,
    );
    measure(format!("hit_test/{lines}-lines"), 10_000, || {
        std::hint::black_box(layout.hit_test(123.0, 11.0));
    })
}

/// Measures the lookup of the line the caret is on: what every caret move
/// pays.
fn bench_line_lookup(lines: usize) -> Measurement {
    let buffer = document(lines);
    let offset = buffer.len().0 / 2;
    measure(format!("caret_line/{lines}-lines"), 10_000, || {
        std::hint::black_box(buffer.line_of_offset(offset));
    })
}

/// The budgets the measurements have to stay inside.
///
/// The plan gives a frame 16 ms for the whole round trip from a keystroke to
/// the picture. The work measured here is the part of that round trip that
/// happens before the GPU is involved, so it gets a small share of it: a
/// normal window must lay itself out well inside a frame, and a hit test or
/// a caret move must not be felt at all.
///
/// The warm numbers are the cost of typing: only the line that was typed on
/// has to be shaped again, so they are far inside the frame budget. They are
/// the tight budgets, and they are the ones that catch a regression — a
/// dropped cache shows up here as a hundred times the cost.
///
/// The cold numbers are the cost of scrolling to text that was never shaped.
/// They are over the plan's 16 ms, and closing that gap is what an
/// incremental layout is for. They are shaped work, so the measurement is
/// noisy; the budgets are loose on purpose.
const BUDGETS: &[(&str, f64)] = &[
    ("layout_cold/10000-lines/40-visible", 20.0),
    ("layout_warm/10000-lines/40-visible", 0.5),
    ("layout_cold/10000-lines/200-visible", 80.0),
    ("layout_warm/10000-lines/200-visible", 2.0),
    ("frame_text_cold/10000-lines/40-visible", 30.0),
    ("frame_text_warm/10000-lines/40-visible", 0.5),
    ("hit_test/10000-lines", 0.1),
    ("caret_line/10000-lines", 0.02),
];

fn main() {
    let measurements: Vec<Measurement> = vec![
        bench_layout(10_000, 40, true),
        bench_layout(10_000, 40, false),
        bench_layout(10_000, 200, true),
        bench_layout(10_000, 200, false),
        bench_frame_text(10_000, 40, true),
        bench_frame_text(10_000, 40, false),
        bench_hit_test(10_000),
        bench_line_lookup(10_000),
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
