//! Benchmarks for the text model.
//!
//! The plan's stage 0 exit criteria name 10 MB and 100 MB files as the
//! reference points, so the benchmarks below measure those sizes directly
//! rather than extrapolating from small inputs.
//!
//! The harness is hand-written rather than pulled from a benchmarking crate:
//! the editor workspace keeps its dependency surface small, and the report
//! this produces is the one the exit criteria are read against.
//!
//! Run with:
//!
//! ```powershell
//! cargo bench -p codeweaver-text
//! ```

use std::time::{Duration, Instant};

use codeweaver_text::{Bias, Buffer, TextEdit, TextOffset, Transaction};

/// Builds a source file of roughly `megabytes` size.
fn source_file(megabytes: usize) -> String {
    let line = "fn main() {\n    println!(\"CodeWeaver\");\n}\n";
    let target = megabytes * 1024 * 1024;
    let mut text = String::with_capacity(target + line.len());
    while text.len() < target {
        text.push_str(line);
    }
    text
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

/// Measures how long it takes to open a file into a buffer.
fn bench_open(megabytes: usize) -> Measurement {
    let text = source_file(megabytes);
    measure(format!("open/{megabytes}mb"), 5, || {
        let buffer = Buffer::new(text.as_str());
        std::hint::black_box(buffer.len());
    })
}

/// Measures the cost of a single-character insertion at three positions.
///
/// The buffer is built once, outside the measured region. Building it inside
/// the loop would measure the cost of opening the file, not the cost of the
/// edit.
fn bench_insert(megabytes: usize) -> Vec<Measurement> {
    let text = source_file(megabytes);
    let length = text.len();
    [("start", 0usize), ("middle", length / 2), ("end", length)]
        .into_iter()
        .map(|(label, position)| {
            let mut buffer = Buffer::new(text.as_str());
            measure(format!("insert/{megabytes}mb/{label}"), 200, move || {
                let version = buffer.version();
                let edit = TextEdit::new(TextOffset(position)..TextOffset(position), "x");
                let applied = buffer
                    .apply(Transaction::new(version, vec![edit]))
                    .expect("bench edit applies");
                std::hint::black_box(applied.version);
                // Undo so the next iteration starts from the same state.
                buffer.replay(applied.inverse).expect("bench undo applies");
            })
        })
        .collect()
}

/// Measures how long it takes to read the text of a large file.
fn bench_read(megabytes: usize) -> Vec<Measurement> {
    let text = source_file(megabytes);
    let buffer = Buffer::new(text.as_str());
    let full = measure(format!("read/{megabytes}mb/full"), 5, || {
        std::hint::black_box(buffer.text());
    });
    let slice = measure(format!("read/{megabytes}mb/slice"), 200, || {
        let range = TextOffset(0)..TextOffset(4096);
        std::hint::black_box(buffer.slice(range));
    });
    vec![full, slice]
}

/// Measures the cost of resolving an anchor, and the cost of an edit that
/// forces the anchor to move.
fn bench_anchor(megabytes: usize) -> Vec<Measurement> {
    let text = source_file(megabytes);
    let length = text.len();
    let mut buffer = Buffer::new(text.as_str());
    let anchor = buffer.anchor(TextOffset(length / 2), Bias::Right);

    let resolve = measure(format!("anchor/{megabytes}mb/resolve"), 1000, || {
        std::hint::black_box(buffer.resolve_anchor(&anchor));
    });

    let mut movable = Buffer::new(text.as_str());
    let tracked = movable.anchor(TextOffset(length / 2), Bias::Right);
    let update = measure(format!("anchor/{megabytes}mb/update"), 200, move || {
        let version = movable.version();
        let edit = TextEdit::new(TextOffset(0)..TextOffset(0), "x");
        let applied = movable
            .apply(Transaction::new(version, vec![edit]))
            .expect("bench edit applies");
        std::hint::black_box(movable.resolve_anchor(&tracked));
        movable.replay(applied.inverse).expect("bench undo applies");
    });

    vec![resolve, update]
}

fn main() {
    let mut measurements: Vec<Measurement> = Vec::new();

    for megabytes in [10usize, 100] {
        measurements.push(bench_open(megabytes));
        measurements.extend(bench_insert(megabytes));
        measurements.extend(bench_read(megabytes));
        measurements.extend(bench_anchor(megabytes));
    }

    println!(
        "{:<28} {:>12} {:>12} {:>12}",
        "case", "median", "p95", "mean"
    );
    println!("{}", "-".repeat(68));
    for measurement in &measurements {
        println!(
            "{:<28} {:>12} {:>12} {:>12}",
            measurement.label,
            format!("{:.3} ms", measurement.median().as_secs_f64() * 1000.0),
            format!("{:.3} ms", measurement.p95().as_secs_f64() * 1000.0),
            format!("{:.3} ms", measurement.mean_ms()),
        );
    }
}
