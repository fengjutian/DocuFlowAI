//! The highlighter on a thread of its own.
//!
//! Parsing a document is the one thing the editor does that grows with the
//! size of the file. Doing it on the thread that draws the picture means a
//! keystroke in a big file waits for a parse 鈥?the caret stops moving, the
//! window stops responding, and the person typing thinks the editor crashed.
//!
//! So the parse happens here, on a thread of its own. The window asks for the
//! highlights of the text that is on screen and keeps drawing while the answer
//! is being made.
//!
//! Two rules make this safe:
//!
//! - **The answer belongs to a version of the text.** A result that arrives
//!   after the text has changed again is dropped: drawing the highlights of an
//!   older text would put colours on the wrong words.
//! - **Only the newest request matters.** A person who types faster than the
//!   highlighter works must not leave a backlog of parses behind; the worker
//!   takes the newest request and forgets the ones in between.

use std::{
    ops::Range,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

use codeweaver_text::BufferVersion;

use crate::{Highlight, HighlightPolicy, Language, Syntax};

/// What the window asks the highlighter for.
enum Job {
    /// Changes the language that is highlighted.
    SetLanguage(Option<Language>),
    /// Changes the sizes at which the highlighter gives up.
    SetPolicy(HighlightPolicy),
    /// Asks for the highlights of a range of the text.
    Highlight {
        version: BufferVersion,
        text: String,
        range: Range<usize>,
        lines: usize,
        generation: u64,
    },
}

/// The answer of the highlighter.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// The version of the text the highlights belong to.
    pub version: BufferVersion,
    /// The range the highlights cover.
    pub range: Range<usize>,
    /// Which request this is the answer to.
    pub generation: u64,
    /// The highlights themselves.
    pub highlights: Vec<Highlight>,
    /// The document was too big to be highlighted.
    pub degraded: bool,
}

/// Runs the highlighter on a thread of its own.
pub struct SyntaxWorker {
    requests: Sender<Job>,
    results: Receiver<Outcome>,
    language: Option<Language>,
    /// The number of the request that was sent last.
    generation: u64,
    /// The number of the request that was answered last.
    finished: u64,
}

impl SyntaxWorker {
    /// Starts the highlighter.
    pub fn new(language: Option<Language>) -> Self {
        let (requests, queue) = channel::<Job>();
        let (sink, results) = channel::<Outcome>();
        thread::Builder::new()
            .name("codeweaver-syntax".to_owned())
            .spawn(move || run(queue, sink, language))
            .expect("the highlighter thread can be started");
        SyntaxWorker {
            requests,
            results,
            language,
            generation: 0,
            finished: 0,
        }
    }

    /// The language that is highlighted, if any.
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// Changes the language that is highlighted.
    ///
    /// The new language is used from the next request on; the tree of the old
    /// language is dropped with it.
    pub fn set_language(&mut self, language: Option<Language>) {
        if self.language == language {
            return;
        }
        self.language = language;
        // A failed send means the thread is gone; there is nothing to tell.
        let _ = self.requests.send(Job::SetLanguage(language));
    }

    /// Changes the sizes at which the highlighter gives up.
    ///
    /// The new limits are used from the next request on.
    pub fn set_policy(&mut self, policy: HighlightPolicy) {
        let _ = self.requests.send(Job::SetPolicy(policy));
    }

    /// Asks for the highlights of a range of the text.
    ///
    /// The text is a copy of the document: the buffer stays on the thread that
    /// draws the picture, and the highlighter gets what it needs to parse.
    pub fn request(
        &mut self,
        version: BufferVersion,
        text: String,
        range: Range<usize>,
        lines: usize,
    ) {
        self.generation += 1;
        let _ = self.requests.send(Job::Highlight {
            version,
            text,
            range,
            lines,
            generation: self.generation,
        });
    }

    /// The answer of the highlighter, if there is one.
    ///
    /// Only the newest answer is returned: an answer for an older request is
    /// worth nothing, because the text it belongs to is gone.
    pub fn poll(&mut self) -> Option<Outcome> {
        let mut newest: Option<Outcome> = None;
        while let Ok(outcome) = self.results.try_recv() {
            if newest
                .as_ref()
                .map(|current| outcome.generation > current.generation)
                .unwrap_or(true)
            {
                newest = Some(outcome);
            }
        }
        let outcome = newest?;
        self.finished = self.finished.max(outcome.generation);
        Some(outcome)
    }

    /// Whether a request is still waiting for its answer.
    pub fn is_busy(&self) -> bool {
        self.generation > self.finished
    }
}

/// The loop of the highlighter thread.
fn run(requests: Receiver<Job>, results: std::sync::mpsc::Sender<Outcome>, language: Option<Language>) {
    let mut syntax = Syntax::new();
    syntax.set_language(language);
    while let Ok(first) = requests.recv() {

        // Only the newest *highlight* matters. A backlog of parses would make
        // the highlighter answer questions nobody is waiting for any more.
        //
        // The other jobs are not merged away: a job that changes the language
        // or the limits has to happen, and it has to happen in the order it
        // was asked for — a highlight that was asked for before the language
        // changed is parsed with the language that was in force then.
        let mut pending = vec![first];
        while let Ok(newer) = requests.try_recv() {
            pending.push(newer);
        }
        let newest = pending
            .iter()
            .rposition(|job| matches!(job, Job::Highlight { .. }));
        let mut outcome: Option<Outcome> = None;
        for (index, job) in pending.into_iter().enumerate() {
            match job {
                Job::SetLanguage(language) => {
                    syntax.set_language(language);
                }
                Job::SetPolicy(policy) => syntax.set_policy(policy),
                Job::Highlight {
                    version,
                    text,
                    range,
                    lines,
                    generation,
                } => {
                    if Some(index) != newest {
                        // An older request: the answer would be about a text
                        // that is gone.
                        continue;
                    }
                    syntax.parse_text(&text, version, lines);
                    let degraded = syntax.is_degraded();
                    let highlights = if degraded {
                        Vec::new()
                    } else {
                        syntax.highlights_of_text(&text, range.clone()).to_vec()
                    };
                    outcome = Some(Outcome {
                        version,
                        range,
                        generation,
                        highlights,
                        degraded,
                    });
                }
            }
        }
        if let Some(outcome) = outcome
            && results.send(outcome).is_err()
        {
            // Nobody is listening any more.
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HighlightKind;
    use std::time::{Duration, Instant};

    /// Waits for the answer of the highlighter.
    fn wait_for(worker: &mut SyntaxWorker) -> Outcome {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(outcome) = worker.poll() {
                return outcome;
            }
            assert!(
                Instant::now() < deadline,
                "the highlighter did not answer in time"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn the_worker_answers_with_the_highlights_of_the_text() {
        let mut worker = SyntaxWorker::new(Some(Language::Rust));
        let text = "fn main() {}\n".to_owned();
        worker.request(1, text.clone(), 0..text.len(), 1);

        let outcome = wait_for(&mut worker);
        assert_eq!(outcome.version, 1);
        assert!(!outcome.degraded);
        assert!(
            outcome
                .highlights
                .iter()
                .any(|highlight| highlight.kind == HighlightKind::Keyword),
            "the keyword of the text is missing: {:?}",
            outcome.highlights
        );
    }

    #[test]
    fn a_document_that_is_too_big_is_reported_as_degraded() {
        let mut worker = SyntaxWorker::new(Some(Language::Rust));
        worker.set_policy(HighlightPolicy {
            max_bytes: 10,
            max_lines: 10,
        });
        let text = "fn main() {}\n".to_owned();
        worker.request(1, text.clone(), 0..text.len(), 1);

        let outcome = wait_for(&mut worker);
        assert!(outcome.degraded, "a big document has to be reported as degraded");
        assert!(outcome.highlights.is_empty());
    }

    #[test]
    fn the_newest_request_is_the_one_that_is_answered() {
        let mut worker = SyntaxWorker::new(Some(Language::Rust));
        for version in 1..=4u64 {
            let text = format!("fn main() {{}}\n// {version}\n");
            worker.request(version, text.clone(), 0..text.len(), 2);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = None;
        while Instant::now() < deadline && last != Some(4) {
            if let Some(outcome) = worker.poll() {
                last = Some(outcome.version);
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(last, Some(4), "the newest request must be answered");
        assert!(!worker.is_busy());
    }

    #[test]
    fn a_worker_that_is_dropped_does_not_keep_the_thread_alive() {
        let mut worker = SyntaxWorker::new(Some(Language::Rust));
        let text = "fn main() {}\n".to_owned();
        worker.request(1, text.clone(), 0..text.len(), 1);
        drop(worker);
        // Nothing is listening any more; the thread ends on its own.
    }
}
