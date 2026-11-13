//! Unified diff parsing, hunk level application, and three way merge.
//!
//! The editor sits between an agent that proposes changes and a buffer that
//! holds the text. Three things have to be right for that to be safe:
//!
//! - **Parsing**: the agent's patch has to be turned into blocks the user
//!   can approve one at a time, not into one blob of new text.
//! - **Application**: only the approved blocks may reach the buffer, and
//!   they may only reach it if the text they were computed against is still
//!   the text that is there.
//! - **Merging**: when the buffer moved on while the agent was working, the
//!   two changes have to be combined rather than one of them silently
//!   overwriting the other.
//!
//! The merge follows the diff3 rule: a region only one side changed is taken
//! from that side, a region both sides changed the same way is taken once,
//! and a region both sides changed differently is a conflict that a person
//! has to settle.

mod myers;

pub use myers::{Matches, diff_lines};

use std::collections::BTreeSet;
/// One line of a hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    /// A line that is in both texts.
    Context(String),
    /// A line that is only in the new text.
    Added(String),
    /// A line that is only in the old text.
    Removed(String),
}

/// One block of changes inside a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The first line of the hunk in the old text, counting from one.
    pub old_start: usize,
    /// How many lines the hunk covers in the old text.
    pub old_lines: usize,
    /// The first line of the hunk in the new text, counting from one.
    pub new_start: usize,
    /// How many lines the hunk covers in the new text.
    pub new_lines: usize,
    /// The lines of the hunk, in order.
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// How many lines the hunk adds.
    pub fn added_lines(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| matches!(line, DiffLine::Added(_)))
            .count()
    }

    /// How many lines the hunk removes.
    pub fn removed_lines(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| matches!(line, DiffLine::Removed(_)))
            .count()
    }
}

/// The changes a patch proposes for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePatch {
    /// The path the patch names, relative to the workspace.
    pub path: String,
    /// The blocks of the patch, in order.
    pub hunks: Vec<Hunk>,
}

impl FilePatch {
    /// How many lines the whole patch adds.
    pub fn added_lines(&self) -> usize {
        self.hunks.iter().map(Hunk::added_lines).sum()
    }

    /// How many lines the whole patch removes.
    pub fn removed_lines(&self) -> usize {
        self.hunks.iter().map(Hunk::removed_lines).sum()
    }
}

/// Why a patch could not be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// The text the patch was computed against is not the text that is there.
    Stale {
        /// Which hunk did not fit.
        hunk: usize,
        /// The line the hunk starts at, counting from one.
        line: usize,
        /// What the patch expected to find there.
        expected: String,
        /// What is actually there.
        found: String,
    },
    /// The hunk does not describe a change.
    Empty {
        /// Which hunk is empty.
        hunk: usize,
    },
}

/// Parses a unified diff into the patches of the files it touches.
///
/// A patch that cannot be attributed to a file is skipped rather than
/// guessed at: a change whose target is unknown must not be applied to
/// whatever file happens to be open.
pub fn parse_unified(patch: &str) -> Vec<FilePatch> {
    let mut patches: Vec<FilePatch> = Vec::new();
    let mut old_path: Option<String> = None;
    let mut path: Option<String> = None;
    let mut hunks: Vec<Hunk> = Vec::new();

    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("--- ") {
            let candidate = rest.split('\t').next().unwrap_or(rest).trim();
            old_path = if candidate == "/dev/null" {
                None
            } else {
                Some(strip_prefix(candidate))
            };
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            let candidate = rest.split('\t').next().unwrap_or(rest).trim();
            // A deleted file has no new side; the old side names it.
            path = if candidate == "/dev/null" {
                old_path.take()
            } else {
                Some(strip_prefix(candidate))
            };
            continue;
        }
        if let Some(header) = line.strip_prefix("@@ ") {
            let Some((old_start, old_lines, new_start, new_lines)) = parse_hunk_header(header) else {
                continue;
            };
            let mut hunk = Hunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
                lines: Vec::new(),
            };
            // The header says how many lines the hunk has; the body has to
            // agree, and a body that stops early ends the hunk.
            let mut old_seen = 0;
            let mut new_seen = 0;
            while old_seen < old_lines || new_seen < new_lines {
                let Some(body) = lines.peek() else { break };
                let body = *body;
                if body.starts_with("@@") || body.starts_with("diff --git") {
                    break;
                }
                lines.next();
                if let Some(rest) = body.strip_prefix(' ') {
                    hunk.lines.push(DiffLine::Context(rest.to_owned()));
                    old_seen += 1;
                    new_seen += 1;
                } else if let Some(rest) = body.strip_prefix('+') {
                    hunk.lines.push(DiffLine::Added(rest.to_owned()));
                    new_seen += 1;
                } else if let Some(rest) = body.strip_prefix('-') {
                    hunk.lines.push(DiffLine::Removed(rest.to_owned()));
                    old_seen += 1;
                } else if body.starts_with('\\') {
                    // The marker that says the file has no line break at the
                    // end; it carries no lines of its own.
                    continue;
                } else {
                    break;
                }
            }
            hunks.push(hunk);
            continue;
        }
        if line.starts_with("diff --git ") {
            // A new file starts; the previous one is complete.
            if let Some(path) = path.take() {
                patches.push(FilePatch {
                    path,
                    hunks: std::mem::take(&mut hunks),
                });
            }
        }
    }

    if let Some(path) = path {
        patches.push(FilePatch { path, hunks });
    }

    patches
}

/// Parses the header of a hunk: `@@ -1,4 +1,5 @@`.
fn parse_hunk_header(header: &str) -> Option<(usize, usize, usize, usize)> {
    let header = header.trim_end_matches(" @@");
    let mut parts = header.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (old_start, old_lines) = parse_range(old)?;
    let (new_start, new_lines) = parse_range(new)?;
    Some((old_start, old_lines, new_start, new_lines))
}

/// Parses `start` or `start,count`.
fn parse_range(value: &str) -> Option<(usize, usize)> {
    match value.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((value.parse().ok()?, 1)),
    }
}

/// Removes the `a/` or `b/` prefix a patch puts in front of a path.
fn strip_prefix(path: &str) -> String {
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_owned()
}

/// Applies the approved hunks of a patch to a text.
///
/// A hunk that is not approved is left out; the text it covered stays as it
/// was. A hunk that no longer fits the text is refused rather than applied
/// at the wrong place, because a patch that lands in the wrong spot is worse
/// than a patch that was not applied.
pub fn apply_hunks(
    base: &str,
    hunks: &[Hunk],
    approved: &BTreeSet<usize>,
) -> Result<String, ApplyError> {
    let (lines, trailing_newline) = split_lines(base);
    let mut result: Vec<String> = Vec::with_capacity(lines.len());
    let mut cursor = 0usize;

    for (index, hunk) in hunks.iter().enumerate() {
        if hunk.old_lines == 0 && hunk.new_lines == 0 {
            return Err(ApplyError::Empty { hunk: index });
        }
        if !approved.contains(&index) {
            continue;
        }

        let start = hunk.old_start.saturating_sub(1);
        // The first line the hunk expects to find. It is what the error
        // names when the hunk does not fit.
        let wanted = hunk
            .lines
            .iter()
            .find_map(|line| match line {
                DiffLine::Context(text) | DiffLine::Removed(text) => Some(text.clone()),
                DiffLine::Added(_) => None,
            })
            .unwrap_or_default();
        if start < cursor || start > lines.len() {
            return Err(ApplyError::Stale {
                hunk: index,
                line: hunk.old_start,
                expected: wanted,
                found: String::new(),
            });
        }

        // The lines the hunk expects to find must be there.
        let mut expected_cursor = start;
        for line in &hunk.lines {
            match line {
                DiffLine::Context(_) | DiffLine::Removed(_) => {
                    let found = lines.get(expected_cursor).map(String::as_str).unwrap_or("");
                    let wanted = match line {
                        DiffLine::Context(text) | DiffLine::Removed(text) => text,
                        DiffLine::Added(_) => unreachable!(),
                    };
                    if found != wanted {
                        return Err(ApplyError::Stale {
                            hunk: index,
                            line: expected_cursor + 1,
                            expected: wanted.to_owned(),
                            found: found.to_owned(),
                        });
                    }
                    expected_cursor += 1;
                }
                DiffLine::Added(_) => {}
            }
        }

        result.extend_from_slice(&lines[cursor..start]);
        for line in &hunk.lines {
            match line {
                DiffLine::Context(text) | DiffLine::Added(text) => result.push(text.clone()),
                DiffLine::Removed(_) => {}
            }
        }
        cursor = expected_cursor;
    }

    result.extend_from_slice(&lines[cursor..]);
    Ok(join_lines(&result, trailing_newline))
}

/// A region the two sides changed differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeConflict {
    /// The lines the base text had in the region.
    pub base: Vec<String>,
    /// The lines the current buffer has in the region.
    pub ours: Vec<String>,
    /// The lines the agent proposed in the region.
    pub theirs: Vec<String>,
    /// The line the region starts at in the merged text, counting from one.
    pub merged_line: usize,
}

/// One piece of a merged text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeSegment {
    /// Lines both sides kept.
    Stable(Vec<String>),
    /// Lines only one side changed.
    Changed(Vec<String>),
    /// A region both sides changed differently.
    Conflict(MergeConflict),
}

/// How a conflict was settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Keep what the buffer has.
    Ours,
    /// Keep what the agent proposed.
    Theirs,
}

/// The outcome of merging two changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    /// The pieces of the merged text, in order.
    pub segments: Vec<MergeSegment>,
    /// The conflicts, in the order they appear.
    pub conflicts: Vec<MergeConflict>,
}

impl Merge {
    /// Whether the two sides disagreed anywhere.
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    /// Renders the merged text with the conflicts settled.
    ///
    /// A conflict without a decision keeps the buffer's version, because the
    /// text that is already there is never silently thrown away.
    pub fn render(&self, choices: &[Choice]) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut conflict_index = 0usize;
        for segment in &self.segments {
            match segment {
                MergeSegment::Stable(text) | MergeSegment::Changed(text) => {
                    lines.extend(text.iter().cloned());
                }
                MergeSegment::Conflict(conflict) => {
                    let choice = choices.get(conflict_index).copied().unwrap_or(Choice::Ours);
                    let chosen = match choice {
                        Choice::Ours => &conflict.ours,
                        Choice::Theirs => &conflict.theirs,
                    };
                    lines.extend(chosen.iter().cloned());
                    conflict_index += 1;
                }
            }
        }
        join_lines(&lines, true)
    }
}

/// Merges two changes that were both made to the same base text.
///
/// `base` is the text both sides started from, `ours` is the current buffer
/// and `theirs` is what the agent proposed.
pub fn merge_three_way(base: &str, ours: &str, theirs: &str) -> Merge {
    let (base_lines, _) = split_lines(base);
    let (our_lines, _) = split_lines(ours);
    let (their_lines, _) = split_lines(theirs);

    let our_matches = diff_lines(&as_slices(&base_lines), &as_slices(&our_lines));
    let their_matches = diff_lines(&as_slices(&base_lines), &as_slices(&their_lines));

    let Some(our_matches) = our_matches else {
        return whole_file_conflict(&base_lines, &our_lines, &their_lines);
    };
    let Some(their_matches) = their_matches else {
        return whole_file_conflict(&base_lines, &our_lines, &their_lines);
    };

    // A base line that both sides kept is a point where the two sides
    // agree, so the text between two such points can be settled on its own.
    let our_by_base: BTreeSet<usize> = our_matches.iter().map(|(base_index, _)| *base_index).collect();
    let their_by_base: BTreeSet<usize> = their_matches.iter().map(|(base_index, _)| *base_index).collect();
    let sync_points: Vec<usize> = our_by_base
        .intersection(&their_by_base)
        .copied()
        .collect();

    let mut segments: Vec<MergeSegment> = Vec::new();
    let mut conflicts: Vec<MergeConflict> = Vec::new();

    let mut base_cursor = 0usize;
    let mut our_cursor = 0usize;
    let mut their_cursor = 0usize;

    for point in sync_points {
        let our_point = our_matches
            .iter()
            .find(|(base_index, _)| *base_index == point)
            .map(|(_, our_index)| *our_index)
            .unwrap_or(our_cursor);
        let their_point = their_matches
            .iter()
            .find(|(base_index, _)| *base_index == point)
            .map(|(_, their_index)| *their_index)
            .unwrap_or(their_cursor);

        let base_region = &base_lines[base_cursor..point];
        let our_region = &our_lines[our_cursor..our_point];
        let their_region = &their_lines[their_cursor..their_point];

        if base_region == our_region && base_region == their_region {
            // Nobody changed it.
            push_stable(&mut segments, base_region);
        } else if our_region == base_region {
            // Only the agent changed it.
            push_changed(&mut segments, their_region);
        } else if their_region == base_region || our_region == their_region {
            // Only the buffer changed it, or both sides made the same change.
            push_changed(&mut segments, our_region);
        } else {
            let merged_line = segments
                .iter()
                .map(|segment| match segment {
                    MergeSegment::Stable(text) | MergeSegment::Changed(text) => text.len(),
                    MergeSegment::Conflict(_) => 0,
                })
                .sum::<usize>()
                + 1;
            let conflict = MergeConflict {
                base: base_region.to_vec(),
                ours: our_region.to_vec(),
                theirs: their_region.to_vec(),
                merged_line,
            };
            conflicts.push(conflict.clone());
            segments.push(MergeSegment::Conflict(conflict));
        }

        // The agreed line itself belongs to the merged text.
        let stable = vec![base_lines[point].clone()];
        push_stable(&mut segments, &stable);

        base_cursor = point + 1;
        our_cursor = our_point + 1;
        their_cursor = their_point + 1;
    }

    // Whatever is left after the last agreed line.
    let base_region = &base_lines[base_cursor..];
    let our_region = &our_lines[our_cursor..];
    let their_region = &their_lines[their_cursor..];

    if base_region == our_region && base_region == their_region {
        push_stable(&mut segments, base_region);
    } else if our_region == base_region {
        push_changed(&mut segments, their_region);
    } else if their_region == base_region || our_region == their_region {
        push_changed(&mut segments, our_region);
    } else {
        let merged_line = segments
            .iter()
            .map(|segment| match segment {
                MergeSegment::Stable(text) | MergeSegment::Changed(text) => text.len(),
                MergeSegment::Conflict(_) => 0,
            })
            .sum::<usize>()
            + 1;
        let conflict = MergeConflict {
            base: base_region.to_vec(),
            ours: our_region.to_vec(),
            theirs: their_region.to_vec(),
            merged_line,
        };
        conflicts.push(conflict.clone());
        segments.push(MergeSegment::Conflict(conflict));
    }

    Merge { segments, conflicts }
}

/// A merge where the two texts could not be aligned at all.
fn whole_file_conflict(base: &[String], ours: &[String], theirs: &[String]) -> Merge {
    let conflict = MergeConflict {
        base: base.to_vec(),
        ours: ours.to_vec(),
        theirs: theirs.to_vec(),
        merged_line: 1,
    };
    Merge {
        segments: vec![MergeSegment::Conflict(conflict.clone())],
        conflicts: vec![conflict],
    }
}

/// Adds a region nobody changed, skipping the empty ones.
fn push_stable(segments: &mut Vec<MergeSegment>, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    segments.push(MergeSegment::Stable(lines.to_vec()));
}

/// Adds a region one side changed, skipping the empty ones.
fn push_changed(segments: &mut Vec<MergeSegment>, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    segments.push(MergeSegment::Changed(lines.to_vec()));
}

/// Splits text into lines, and says whether the text ends with a line break.
fn split_lines(text: &str) -> (Vec<String>, bool) {
    if text.is_empty() {
        return (Vec::new(), false);
    }
    let trailing_newline = text.ends_with('\n');
    let body = if trailing_newline {
        &text[..text.len() - 1]
    } else {
        text
    };
    (
        body.split('\n').map(str::to_owned).collect(),
        trailing_newline,
    )
}

/// Joins lines back into text.
fn join_lines(lines: &[String], trailing_newline: bool) -> String {
    let mut text = lines.join("\n");
    if trailing_newline && !text.is_empty() {
        text.push('\n');
    }
    text
}

/// Borrows the lines as the slices the diff works on.
fn as_slices(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch_of(patch: &str) -> FilePatch {
        let patches = parse_unified(patch);
        assert_eq!(patches.len(), 1, "the patch names one file");
        patches.into_iter().next().expect("the patch is there")
    }

    #[test]
    fn a_patch_is_split_into_hunks() {
        let patch = patch_of(
            "--- a/src/app.rs\n+++ b/src/app.rs\n@@ -1,2 +1,3 @@\n one\n-two\n+TWO\n+three\n",
        );
        assert_eq!(patch.path, "src/app.rs");
        assert_eq!(patch.hunks.len(), 1);
        assert_eq!(patch.hunks[0].old_start, 1);
        assert_eq!(patch.hunks[0].old_lines, 2);
        assert_eq!(patch.hunks[0].new_lines, 3);
        assert_eq!(patch.added_lines(), 2);
        assert_eq!(patch.removed_lines(), 1);
    }

    #[test]
    fn a_patch_with_two_hunks_keeps_both() {
        let patch = patch_of(
            "--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,2 @@\n a\n+b\n@@ -3,1 +4,2 @@\n c\n+C\n",
        );
        assert_eq!(patch.hunks.len(), 2);
        assert_eq!(patch.hunks[1].old_start, 3);
    }

    #[test]
    fn the_path_of_a_patch_drops_the_prefix() {
        let patch = patch_of("--- a/deep/path.rs\n+++ b/deep/path.rs\n@@ -1,1 +1,1 @@\n a\n");
        assert_eq!(patch.path, "deep/path.rs");
    }

    #[test]
    fn only_the_approved_hunk_is_applied() {
        let patch = patch_of(
            "--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,2 @@\n a\n+A\n@@ -3,1 +4,2 @@\n c\n+C\n",
        );
        let base = "a\nb\nc\n";
        let approved = BTreeSet::from([0usize]);
        let applied = apply_hunks(base, &patch.hunks, &approved).expect("the patch fits");
        assert_eq!(applied, "a\nA\nb\nc\n");
    }

    #[test]
    fn a_hunk_that_does_not_fit_is_refused() {
        let patch = patch_of("--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,2 @@\n a\n+b\n");
        let approved = BTreeSet::from([0usize]);
        let error = apply_hunks("different\n", &patch.hunks, &approved).expect_err("it does not fit");
        assert!(matches!(error, ApplyError::Stale { .. }));
    }

    #[test]
    fn a_hunk_applied_to_the_wrong_place_is_refused() {
        let patch = patch_of("--- a/f.txt\n+++ b/f.txt\n@@ -5,1 +5,2 @@\n e\n+E\n");
        let approved = BTreeSet::from([0usize]);
        let error = apply_hunks("a\nb\nc\n", &patch.hunks, &approved).expect_err("it does not fit");
        match error {
            ApplyError::Stale { line, .. } => assert_eq!(line, 5),
            other => panic!("the wrong error: {other:?}"),
        }
    }

    #[test]
    fn changes_in_different_places_merge_cleanly() {
        let base = "one\ntwo\nthree\nfour\n";
        let ours = "ONE\ntwo\nthree\nfour\n";
        let theirs = "one\ntwo\nthree\nFOUR\n";
        let merge = merge_three_way(base, ours, theirs);
        assert!(!merge.has_conflicts(), "the two changes do not overlap");
        let merged = merge.render(&[]);
        assert_eq!(merged, "ONE\ntwo\nthree\nFOUR\n");
    }

    #[test]
    fn the_same_change_on_both_sides_is_taken_once() {
        let base = "one\ntwo\n";
        let ours = "one\nTWO\n";
        let theirs = "one\nTWO\n";
        let merge = merge_three_way(base, ours, theirs);
        assert!(!merge.has_conflicts());
        assert_eq!(merge.render(&[]), "one\nTWO\n");
    }

    #[test]
    fn changes_to_the_same_line_conflict() {
        let base = "one\ntwo\nthree\n";
        let ours = "one\nMINE\nthree\n";
        let theirs = "one\nTHEIRS\nthree\n";
        let merge = merge_three_way(base, ours, theirs);
        assert!(merge.has_conflicts());
        assert_eq!(merge.conflicts.len(), 1);
        assert_eq!(merge.conflicts[0].ours, vec!["MINE".to_owned()]);
        assert_eq!(merge.conflicts[0].theirs, vec!["THEIRS".to_owned()]);
    }

    #[test]
    fn adjacent_changes_conflict_too() {
        // There is no unchanged line between the two changes to sync on, so
        // both fall into one region and conflict.
        let base = "one\ntwo\nthree\n";
        let ours = "one\nMINE\nthree\n";
        let theirs = "one\ntwo\nTHEIRS\n";
        let merge = merge_three_way(base, ours, theirs);
        assert!(merge.has_conflicts());
    }

    #[test]
    fn an_unchanged_line_between_the_changes_lets_them_merge() {
        let base = "one\ntwo\nthree\nfour\n";
        let ours = "one\nMINE\nthree\nfour\n";
        let theirs = "one\ntwo\nthree\nTHEIRS\n";
        let merge = merge_three_way(base, ours, theirs);
        assert!(!merge.has_conflicts(), "the changes are separated");
        assert_eq!(merge.render(&[]), "one\nMINE\nthree\nTHEIRS\n");
    }

    #[test]
    fn a_conflict_is_settled_by_the_choice() {
        let base = "one\ntwo\n";
        let ours = "one\nMINE\n";
        let theirs = "one\nTHEIRS\n";
        let merge = merge_three_way(base, ours, theirs);
        assert_eq!(merge.render(&[Choice::Theirs]), "one\nTHEIRS\n");
        assert_eq!(merge.render(&[Choice::Ours]), "one\nMINE\n");
    }

    #[test]
    fn a_conflict_without_a_choice_keeps_the_buffer() {
        let base = "one\ntwo\n";
        let ours = "one\nMINE\n";
        let theirs = "one\nTHEIRS\n";
        let merge = merge_three_way(base, ours, theirs);
        assert_eq!(merge.render(&[]), "one\nMINE\n");
    }

    #[test]
    fn deleting_on_one_side_is_taken_when_the_other_side_left_it_alone() {
        let base = "one\ntwo\nthree\n";
        let ours = "one\nthree\n";
        let theirs = "one\ntwo\nthree\n";
        let merge = merge_three_way(base, ours, theirs);
        assert!(!merge.has_conflicts());
        assert_eq!(merge.render(&[]), "one\nthree\n");
    }

    #[test]
    fn a_deletion_on_both_sides_is_taken_once() {
        let base = "one\ntwo\nthree\n";
        let ours = "one\nthree\n";
        let theirs = "one\nthree\n";
        let merge = merge_three_way(base, ours, theirs);
        assert!(!merge.has_conflicts());
        assert_eq!(merge.render(&[]), "one\nthree\n");
    }
}
