//! Applying an approved change to a buffer.
//!
//! The agent proposes a change as a unified diff. The user approves the
//! blocks they want. This module turns that into text and puts it into the
//! buffer — but only when the text the change was computed against is still
//! the text that is there.
//!
//! When it is not, the change and the buffer have to be merged rather than
//! one of them overwriting the other. The merge follows diff3: a region only
//! one side changed is taken from that side, and a region both sides changed
//! differently is a conflict the user settles.

use std::collections::BTreeSet;

use codeweaver_agent_ui::DiffProposal;
use codeweaver_diff::{self, Choice, Merge};

/// What happened when a change was applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The change was written into the buffer.
    Applied {
        /// The text the buffer should hold now.
        text: String,
        /// How many blocks were written.
        hunks: usize,
        /// How many lines the buffer grew by.
        added: usize,
        /// How many lines the buffer lost.
        removed: usize,
        /// The version of the buffer before the change.
        from_version: u64,
        /// The version of the buffer after the change.
        to_version: u64,
    },
    /// The change does not fit the text that is there.
    ///
    /// The text was not changed: a change that does not fit is not applied
    /// somewhere else either.
    DoesNotFit {
        /// Which block did not fit.
        hunk: usize,
        /// The line the block starts at.
        line: usize,
        /// What the block expected to find there.
        expected: String,
        /// What is actually there.
        found: String,
    },
    /// The buffer moved on and the two changes disagree.
    ///
    /// The text was not changed. The conflicts are what the user has to
    /// settle before the change can be applied.
    Conflict {
        /// The conflict itself.
        merge: Merge,
    },
}

/// Applies the approved blocks of a proposal to the buffer's text.
///
/// `base_text` is the text the proposal was computed against and
/// `base_version` the version of the buffer at that time. `current_text` and
/// `current_version` are what the buffer holds now.
///
/// The version is the thing that decides the path: the same version means
/// the change still fits, a different version means the two have to be
/// merged.
pub fn apply_proposal(
    proposal: &DiffProposal,
    patch: &str,
    base_text: &str,
    base_version: u64,
    current_text: &str,
    current_version: u64,
    approved: &BTreeSet<usize>,
) -> ApplyOutcome {
    let Some(file) = codeweaver_diff::parse_unified(patch)
        .into_iter()
        .find(|file| file.path == proposal.path)
    else {
        return ApplyOutcome::DoesNotFit {
            hunk: 0,
            line: 1,
            expected: proposal.path.clone(),
            found: String::new(),
        };
    };

    // The change is computed against the base, so that is what it is applied
    // to. The result is then merged into what the buffer holds now.
    let hunks: Vec<codeweaver_diff::Hunk> = file
        .hunks
        .iter()
        .enumerate()
        .filter(|(index, _)| approved.contains(index))
        .map(|(_, hunk)| hunk.clone())
        .collect();
    // The blocks were already picked, so all of the ones that are left are
    // the ones to apply.
    let all = hunks.iter().enumerate().map(|(index, _)| index).collect();
    let proposed = match codeweaver_diff::apply_hunks(base_text, &hunks, &all) {
        Ok(text) => text,
        Err(codeweaver_diff::ApplyError::Stale {
            hunk,
            line,
            expected,
            found,
        }) => {
            return ApplyOutcome::DoesNotFit {
                hunk,
                line,
                expected,
                found,
            };
        }
        Err(codeweaver_diff::ApplyError::Empty { hunk }) => {
            return ApplyOutcome::DoesNotFit {
                hunk,
                line: 1,
                expected: String::new(),
                found: String::new(),
            };
        }
    };

    let added = file.added_lines();
    let removed = file.removed_lines();

    if base_version == current_version {
        // Nothing moved: the change is the new text.
        return ApplyOutcome::Applied {
            text: proposed,
            hunks: approved.len(),
            added,
            removed,
            from_version: base_version,
            to_version: current_version,
        };
    }

    // The buffer moved on. The base is what both sides started from, the
    // current text is what the user has now, and the proposed text is what
    // the agent wanted.
    //
    // The merge decides on its own whether the two can be combined: a region
    // only one side changed is taken from that side, and a region both sides
    // changed differently is a conflict. The change not fitting the current
    // text is exactly what a conflict looks like, so it is not refused here.
    //
    // Two texts that share nothing cannot be aligned, so there is nothing to
    // merge them on. That is the one case where the change is refused
    // instead of being merged.
    if !can_align(base_text, current_text) {
        return ApplyOutcome::DoesNotFit {
            hunk: 0,
            line: 1,
            expected: "the text the change was made against".to_owned(),
            found: "text that shares nothing with it".to_owned(),
        };
    }

    let merge = codeweaver_diff::merge_three_way(base_text, current_text, &proposed);
    if merge.has_conflicts() {
        return ApplyOutcome::Conflict { merge };
    }

    ApplyOutcome::Applied {
        text: merge.render(&[]),
        hunks: approved.len(),
        added,
        removed,
        from_version: current_version,
        to_version: current_version,
    }
}

/// Whether the two texts can be aligned at all.
///
/// Two texts that share nothing cannot be merged: there is no point to merge
/// them on, and pretending otherwise would produce a text that is neither
/// side's. Sharing one line is enough to align on.
pub fn can_align(base: &str, current: &str) -> bool {
    let base_lines: Vec<&str> = base.lines().collect();
    let current_lines: Vec<&str> = current.lines().collect();
    match codeweaver_diff::diff_lines(&base_lines, &current_lines) {
        Some(matches) => !matches.is_empty(),
        None => false,
    }
}

/// The text the merged change produces.
///
/// It is only called once the conflicts are settled; a merge with conflicts
/// left in it is not a text to write into a buffer.
pub fn merged_text(merge: &Merge, choices: &[Choice]) -> String {
    merge.render(choices)
}

/// How many blocks the user has to settle.
pub fn conflict_count(merge: &Merge) -> usize {
    merge.conflicts.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_agent_ui::{DiffProposal, HunkEntry};

    fn proposal(path: &str, hunks: usize) -> DiffProposal {
        DiffProposal {
            path: path.to_owned(),
            hunks: (0..hunks)
                .map(|index| HunkEntry {
                    line: index + 1,
                    added: vec!["new".to_owned()],
                    removed: vec!["old".to_owned()],
                    decision: codeweaver_agent_ui::HunkDecision::Approved,
                })
                .collect(),
            base_version: Some(1),
            base_text: None,
        }
    }

    const PATCH: &str = "--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,2 @@\n keep\n-old\n+new\n";

    #[test]
    fn a_change_that_fits_is_applied() {
        let approved = BTreeSet::from([0usize]);
        let outcome = apply_proposal(
            &proposal("f.txt", 1),
            PATCH,
            "keep\nold\n",
            1,
            "keep\nold\n",
            1,
            &approved,
        );
        match outcome {
            ApplyOutcome::Applied {
                text,
                hunks,
                added,
                removed,
                from_version,
                to_version,
            } => {
                assert_eq!(text, "keep\nnew\n");
                assert_eq!(hunks, 1);
                assert_eq!(added, 1);
                assert_eq!(removed, 1);
                assert_eq!(from_version, 1);
                assert_eq!(to_version, 1);
            }
            other => panic!("the wrong outcome: {other:?}"),
        }
    }

    #[test]
    fn a_rejected_block_does_not_reach_the_buffer() {
        let approved = BTreeSet::new();
        let outcome = apply_proposal(
            &proposal("f.txt", 1),
            PATCH,
            "keep\nold\n",
            1,
            "keep\nold\n",
            1,
            &approved,
        );
        match outcome {
            ApplyOutcome::Applied { text, hunks, .. } => {
                assert_eq!(text, "keep\nold\n");
                assert_eq!(hunks, 0);
            }
            other => panic!("the wrong outcome: {other:?}"),
        }
    }

    #[test]
    fn only_the_approved_block_is_applied() {
        let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,2 @@\n a\n+A\n@@ -3,1 +4,2 @@\n c\n+C\n";
        let approved = BTreeSet::from([0usize]);
        let outcome = apply_proposal(
            &proposal("f.txt", 2),
            patch,
            "a\nb\nc\n",
            1,
            "a\nb\nc\n",
            1,
            &approved,
        );
        match outcome {
            ApplyOutcome::Applied { text, hunks, .. } => {
                assert_eq!(text, "a\nA\nb\nc\n");
                assert_eq!(hunks, 1);
            }
            other => panic!("the wrong outcome: {other:?}"),
        }
    }

    #[test]
    fn a_change_that_does_not_fit_is_refused() {
        let approved = BTreeSet::from([0usize]);
        let outcome = apply_proposal(
            &proposal("f.txt", 1),
            PATCH,
            "keep\nold\n",
            1,
            "something else\n",
            2,
            &approved,
        );
        match outcome {
            ApplyOutcome::DoesNotFit { line, .. } => assert_eq!(line, 1),
            other => panic!("the wrong outcome: {other:?}"),
        }
    }

    #[test]
    fn a_change_against_a_moved_buffer_becomes_a_conflict() {
        let approved = BTreeSet::from([0usize]);
        // The buffer changed the very line the agent wanted to change.
        let outcome = apply_proposal(
            &proposal("f.txt", 1),
            PATCH,
            "keep\nold\n",
            1,
            "keep\nMINE\n",
            2,
            &approved,
        );
        match outcome {
            ApplyOutcome::Conflict { merge } => {
                assert!(merge.has_conflicts());
                assert_eq!(merge.conflicts.len(), 1);
            }
            other => panic!("the wrong outcome: {other:?}"),
        }
    }

    #[test]
    fn a_change_against_a_moved_buffer_merges_when_they_do_not_overlap() {
        let approved = BTreeSet::from([0usize]);
        // The agent changes the first line; the user changes the last one.
        // An unchanged line between them keeps the two apart.
        let patch = "--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,2 @@\n keep\n+AGENT\n@@ -3,1 +4,1 @@\n tail\n";
        let outcome = apply_proposal(
            &proposal("f.txt", 2),
            patch,
            "keep\nmiddle\ntail\n",
            1,
            "keep\nmiddle\nMINE\n",
            2,
            &approved,
        );
        match outcome {
            ApplyOutcome::Applied { text, .. } => {
                assert_eq!(text, "keep\nAGENT\nmiddle\nMINE\n");
            }
            other => panic!("the wrong outcome: {other:?}"),
        }
    }

    #[test]
    fn a_change_that_touches_the_same_line_as_the_buffer_is_a_conflict() {
        let approved = BTreeSet::from([0usize]);
        let outcome = apply_proposal(
            &proposal("f.txt", 1),
            PATCH,
            "keep\nold\n",
            1,
            "keep\nMINE\n",
            2,
            &approved,
        );
        match outcome {
            ApplyOutcome::Conflict { merge } => {
                assert!(merge.has_conflicts());
                assert_eq!(merge.conflicts.len(), 1);
            }
            other => panic!("the wrong outcome: {other:?}"),
        }
    }

    #[test]
    fn a_conflict_is_settled_by_the_choice() {
        let approved = BTreeSet::from([0usize]);
        let outcome = apply_proposal(
            &proposal("f.txt", 1),
            PATCH,
            "keep\nold\n",
            1,
            "keep\nMINE\n",
            2,
            &approved,
        );
        let ApplyOutcome::Conflict { merge } = outcome else {
            panic!("there was no conflict");
        };
        assert_eq!(conflict_count(&merge), 1);
        assert_eq!(merged_text(&merge, &[Choice::Ours]), "keep\nMINE\n");
        assert_eq!(merged_text(&merge, &[Choice::Theirs]), "keep\nnew\n");
    }
}
