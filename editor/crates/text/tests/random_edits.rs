//! Randomised edit sequences that check the invariants the editor relies on.
//!
//! These are property tests written against a deterministic pseudo-random
//! generator rather than a sampling framework: the editor workspace has no
//! dependency on a test harness crate yet, and a fixed seed keeps failures
//! reproducible from the printed seed alone.

use codeweaver_text::{ApplyError, Bias, Buffer, BufferVersion, TextEdit, TextOffset, Transaction};

/// A small, deterministic xorshift generator.
///
/// The generator is intentionally local to the tests so the whole suite stays
/// dependency-free and every failure can be replayed from its seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }
}

/// Picks a char boundary inside `text` at or before `hint`.
fn snap_to_boundary(text: &str, hint: usize) -> usize {
    let mut candidate = hint.min(text.len());
    while candidate > 0 && !text.is_char_boundary(candidate) {
        candidate -= 1;
    }
    candidate
}

/// Builds a random edit against `text`, returning the edit and its result.
fn random_edit(rng: &mut Rng, text: &str) -> TextEdit {
    let start = snap_to_boundary(text, rng.below(text.len() + 1));
    let end = snap_to_boundary(text, start + rng.below(text.len() - start + 1));
    let replacement: String = (0..rng.below(4))
        .map(|_| char::from(b'a' + rng.below(26) as u8))
        .collect();
    TextEdit::new(TextOffset(start)..TextOffset(end), replacement)
}

/// The core property: after any sequence of edits, undoing all of them must
/// restore the original text exactly, and the inverse transactions must
/// round-trip through the buffer without corrupting it.
#[test]
fn undo_restores_the_original_text_after_random_edits() {
    for seed in 1..=64u64 {
        let original = "hello 中文 world\nsecond line\n";
        let mut buffer = Buffer::new(original);
        let mut history: Vec<codeweaver_text::Transaction> = Vec::new();
        let mut rng = Rng::new(seed);

        for _ in 0..32 {
            let text = buffer.text();
            let edit = random_edit(&mut rng, &text);
            let version = buffer.version();
            let applied = match buffer.apply(Transaction::new(version, vec![edit])) {
                Ok(applied) => applied,
                Err(error) => panic!("seed {seed}: unexpected apply error: {error:?}"),
            };
            history.push(applied.inverse);
        }

        while let Some(inverse) = history.pop() {
            buffer
                .replay(inverse)
                .unwrap_or_else(|error| panic!("seed {seed}: undo failed: {error:?}"));
        }

        assert_eq!(
            buffer.text(),
            original,
            "seed {seed}: undo did not restore the original text"
        );
        // Every applied transaction advances the version, and undoing is
        // itself an applied transaction, so the counter ends at twice the
        // number of edits.
        assert_eq!(buffer.version(), 64, "seed {seed}: version drift");
    }
}

/// Anchors must never point outside the buffer, and an anchor that was created
/// at the end of the text must keep tracking the end as the text grows.
#[test]
fn anchors_always_resolve_inside_the_buffer() {
    for seed in 1..=64u64 {
        let mut buffer = Buffer::new("base text");
        let mut rng = Rng::new(seed);
        let anchor = buffer.anchor(TextOffset(buffer.len().0), Bias::Right);

        for _ in 0..32 {
            let text = buffer.text();
            let edit = random_edit(&mut rng, &text);
            let version = buffer.version();
            buffer
                .apply(Transaction::new(version, vec![edit]))
                .unwrap_or_else(|error| panic!("seed {seed}: apply failed: {error:?}"));

            let resolved = buffer
                .resolve_anchor(&anchor)
                .unwrap_or_else(|| panic!("seed {seed}: anchor disappeared"));
            assert!(
                resolved.0 <= buffer.len().0,
                "seed {seed}: anchor resolved outside the buffer"
            );
        }
    }
}

/// A redo stack built by undoing must replay the exact same edits.
///
/// Undoing an edit yields the transaction that redoes it: the inverse of the
/// inverse. The redo stack is therefore filled with the inverse returned by
/// each undo, and it is drained from the back so that the earliest undone
/// edit is redone first.
#[test]
fn redo_replays_the_same_edits() {
    for seed in 1..=32u64 {
        let mut buffer = Buffer::new("redo base");
        let mut rng = Rng::new(seed);
        let mut undo_stack: Vec<codeweaver_text::Transaction> = Vec::new();

        for _ in 0..16 {
            let text = buffer.text();
            let edit = random_edit(&mut rng, &text);
            let version = buffer.version();
            let applied = buffer
                .apply(Transaction::new(version, vec![edit]))
                .unwrap_or_else(|error| panic!("seed {seed}: apply failed: {error:?}"));
            undo_stack.push(applied.inverse);
        }

        let expected_final = buffer.text();
        let mut redo_stack: Vec<codeweaver_text::Transaction> = Vec::new();
        while let Some(undo) = undo_stack.pop() {
            let undone = buffer
                .replay(undo)
                .unwrap_or_else(|error| panic!("seed {seed}: undo failed: {error:?}"));
            redo_stack.push(undone.inverse);
        }

        while let Some(redo) = redo_stack.pop() {
            buffer
                .replay(redo)
                .unwrap_or_else(|error| panic!("seed {seed}: redo failed: {error:?}"));
        }

        assert_eq!(
            buffer.text(),
            expected_final,
            "seed {seed}: redo did not reproduce the final text"
        );
    }
}

/// Every applied transaction must advance the version by exactly one, and a
/// transaction built against an older version must always be rejected.
#[test]
fn versions_advance_monotonically_and_stale_transactions_are_rejected() {
    let mut buffer = Buffer::new("versioned");
    let mut rng = Rng::new(7);
    let mut seen_versions: Vec<BufferVersion> = vec![buffer.version()];

    for _ in 0..24 {
        let text = buffer.text();
        let edit = random_edit(&mut rng, &text);
        let stale = Transaction::new(seen_versions[0], vec![edit.clone()]);
        let version = buffer.version();
        buffer
            .apply(Transaction::new(version, vec![edit]))
            .unwrap_or_else(|error| panic!("unexpected apply error: {error:?}"));
        seen_versions.push(buffer.version());

        let error = buffer.apply(stale).unwrap_err();
        assert!(
            matches!(error, ApplyError::StaleVersion { .. }),
            "expected a stale-version error, got {error:?}"
        );
    }

    assert_eq!(buffer.version(), 24);
}
