//! A probe for where the time of shaping a line goes.
//!
//! This is the measurement behind
//! [ADR-0006](../../docs/adr/0006-shape-cache-and-perf-baseline.md): shaping a
//! line costs about five to nine microseconds per character, and the editor
//! used to shape every visible line twice per frame. That is why the shaped
//! runs are cached.
//!
//! Run with:
//!
//! ```powershell
//! cargo run -p codeweaver-render --example shaper_probe --release
//! ```

use std::time::{Duration, Instant};

use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};

const LINE: &str = "    let doubled = value + 1234;";
const MIXED: &str = "    let 名字 = \"值\";";
const ITERATIONS: usize = 20_000;

fn per_microseconds(total: Duration, iterations: usize) -> f64 {
    total.as_secs_f64() * 1_000_000.0 / iterations as f64
}

/// Shapes `text` `iterations` times and reports the time per shape.
fn shape_cost(text: &str, shaping: Shaping, iterations: usize) -> f64 {
    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(16.0, 22.0);
    let start = Instant::now();
    for _ in 0..iterations {
        let mut buffer = Buffer::new(&mut font_system, metrics);
        buffer.set_text(
            &mut font_system,
            text,
            &Attrs::new().family(Family::Monospace),
            shaping,
        );
        let runs = buffer.layout_runs();
        std::hint::black_box(runs.count());
    }
    per_microseconds(start.elapsed(), iterations)
}

/// Prints the runs of a shaped line: how many there are and how wide each
/// one says the line is.
fn report_runs(text: &str) {
    let mut font_system = FontSystem::new();
    let mut buffer = Buffer::new(&mut font_system, Metrics::new(16.0, 22.0));
    buffer.set_text(
        &mut font_system,
        text,
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
    );
    let runs: Vec<_> = buffer
        .layout_runs()
        .map(|run| (run.text.to_owned(), run.line_w, run.glyphs.len()))
        .collect();
    println!("{text:?}");
    let mut sum = 0.0;
    for (run_text, line_w, glyphs) in &runs {
        println!("  run {run_text:?}: line_w = {line_w}, glyphs = {glyphs}");
        sum += line_w;
    }
    println!("  sum of the line widths: {sum}");
}

fn main() {
    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(16.0, 22.0);

    // 1. Allocating a buffer and nothing else.
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let buffer = Buffer::new(&mut font_system, metrics);
        std::hint::black_box(&buffer);
    }
    let allocate = per_microseconds(start.elapsed(), ITERATIONS);

    // 2. Allocating a buffer and shaping the text into it.
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let mut buffer = Buffer::new(&mut font_system, metrics);
        buffer.set_text(
            &mut font_system,
            LINE,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
        std::hint::black_box(&buffer);
    }
    let shape = per_microseconds(start.elapsed(), ITERATIONS);

    // 3. Allocating a buffer, shaping the text, and laying it out into runs.
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let mut buffer = Buffer::new(&mut font_system, metrics);
        buffer.set_text(
            &mut font_system,
            LINE,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
        let runs = buffer.layout_runs();
        std::hint::black_box(runs.count());
    }
    let layout = per_microseconds(start.elapsed(), ITERATIONS);

    // 4. The same text shaped again and again into one reused buffer.
    let mut reused = Buffer::new(&mut font_system, metrics);
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        reused.set_text(
            &mut font_system,
            LINE,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
        let runs = reused.layout_runs();
        std::hint::black_box(runs.count());
    }
    let reuse = per_microseconds(start.elapsed(), ITERATIONS);

    println!(
        "line: {LINE:?} ({} bytes), {ITERATIONS} iterations",
        LINE.len()
    );
    println!("  allocate a buffer        {allocate:>8.2} us");
    println!("  + shape the text         {shape:>8.2} us");
    println!("  + lay out the runs       {layout:>8.2} us");
    println!("  reused buffer, laid out  {reuse:>8.2} us");

    // 5. Is the cost per call or per character?
    println!();
    println!("cost per character (advanced shaping):");
    for (label, text) in [
        ("1 char", "x"),
        ("8 chars", "abcdefgh"),
        ("31 chars", LINE),
        ("96 chars", &LINE.repeat(4)[..93]),
    ] {
        let cost = shape_cost(text, Shaping::Advanced, ITERATIONS);
        println!(
            "  {label:>8}: {cost:>8.2} us  ({:>6.2} us per char)",
            cost / text.chars().count() as f64
        );
    }

    // 6. Does the advanced shaping pass cost more than the simple one?
    println!();
    let advanced = shape_cost(LINE, Shaping::Advanced, ITERATIONS);
    let basic = shape_cost(LINE, Shaping::Basic, ITERATIONS);
    println!("  advanced shaping: {advanced:>8.2} us");
    println!("  basic shaping:    {basic:>8.2} us");

    // 7. How many runs does a line with two scripts have?
    println!();
    report_runs(LINE);
    report_runs(MIXED);
}
