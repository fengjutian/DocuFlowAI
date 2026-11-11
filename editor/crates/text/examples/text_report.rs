//! A plain-text report of the text model measurements.
//!
//! Criterion produces statistical comparisons, but the stage 0 exit criteria
//! are read by a human looking for a number next to a target. This example
//! prints the same measurements in that shape and exits non-zero when a
//! target is missed, so CI can use it as a gate.
//!
//! ```powershell
//! cargo run -p codeweaver-text --example text_report --release
//! ```

use std::time::Instant;

use codeweaver_text::{Buffer, TextEdit, TextOffset, Transaction};

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

/// The targets the plan sets for the text model.
///
/// `open` is the budget for loading a 100 MB file, and `edit` is the budget
/// for one keystroke inside it. Both come from the stage 0 exit criteria:
/// a 100 MB file must open within three seconds without freezing the UI, and
/// input latency must stay inside a frame.
fn main() {
    let mut failures: Vec<String> = Vec::new();

    println!("{:<18} {:>12} {:>14}", "case", "measured", "target");
    println!("{}", "-".repeat(46));

    for megabytes in [10usize, 100] {
        let text = source_file(megabytes);

        let start = Instant::now();
        let mut buffer = Buffer::new(text);
        let open = start.elapsed();

        // One keystroke in the middle of the document, including the undo
        // that puts the document back into its original state.
        let middle = buffer.len().0 / 2;
        let start = Instant::now();
        for _ in 0..100 {
            let version = buffer.version();
            let edit = TextEdit::new(TextOffset(middle)..TextOffset(middle), "x");
            let applied = buffer
                .apply(Transaction::new(version, vec![edit]))
                .expect("edit applies");
            buffer.replay(applied.inverse).expect("undo applies");
        }
        let per_edit = start.elapsed() / 100;

        let open_budget = if megabytes == 100 {
            std::time::Duration::from_secs(3)
        } else {
            std::time::Duration::from_millis(500)
        };
        let edit_budget = std::time::Duration::from_millis(16);

        println!(
            "{:<18} {:>12} {:>14}",
            format!("{megabytes}mb/open"),
            format!("{:.1} ms", open.as_secs_f64() * 1000.0),
            format!("{} ms", open_budget.as_millis()),
        );
        println!(
            "{:<18} {:>12} {:>14}",
            format!("{megabytes}mb/edit"),
            format!("{:.3} ms", per_edit.as_secs_f64() * 1000.0),
            format!("{} ms", edit_budget.as_millis()),
        );

        if open > open_budget {
            failures.push(format!(
                "{megabytes}mb open took {:.1} ms, budget is {} ms",
                open.as_secs_f64() * 1000.0,
                open_budget.as_millis()
            ));
        }
        if per_edit > edit_budget {
            failures.push(format!(
                "{megabytes}mb edit took {:.3} ms, budget is {} ms",
                per_edit.as_secs_f64() * 1000.0,
                edit_budget.as_millis()
            ));
        }
    }

    if failures.is_empty() {
        println!("\nall targets met");
        return;
    }

    println!();
    for failure in failures {
        eprintln!("FAIL: {failure}");
    }
    std::process::exit(1);
}
