//! Line diffing by Myers' O(ND) algorithm.
//!
//! The editor needs the alignment of two texts for two reasons: it has to
//! know which parts of a file the agent changed, and it has to align the
//! buffer with the text the agent saw so a three way merge can say which
//! side changed what.
//!
//! The implementation is Myers' greedy algorithm with the trace kept per
//! edit distance, which is the same family git uses. The search gives up
//! when the two texts are too far apart: a merge that cannot be aligned is
//! reported as a conflict rather than guessed at.

/// The matched line pairs of a longest common subsequence.
///
/// Every pair says "line `i` of the left text is the same line as line `j`
/// of the right text".
pub type Matches = Vec<(usize, usize)>;

/// How many edits the search explores before it gives up.
///
/// The cost of the search grows with the number of edits, so two texts that
/// share almost nothing are expensive to align. The editor meets that case
/// when a file was rewritten from scratch, and a conflict is the honest
/// answer there.
const MAX_DISTANCE: usize = 4_000;

/// Aligns two lists of lines.
///
/// Returns `None` when the texts are too far apart to align within
/// [`MAX_DISTANCE`] edits.
pub fn diff_lines(left: &[&str], right: &[&str]) -> Option<Matches> {
    let left_len = left.len();
    let right_len = right.len();
    if left_len == 0 || right_len == 0 {
        return Some(Vec::new());
    }

    let max = left_len + right_len;
    let offset = max as i64;
    let mut furthest = vec![0i64; 2 * max + 1];
    let mut trace: Vec<Vec<i64>> = Vec::new();
    let limit = max.min(MAX_DISTANCE);

    for distance in 0..=limit {
        let distance = distance as i64;
        let mut snapshot = vec![0i64; distance as usize + 1];

        for diagonal in (-distance..=distance).step_by(2) {
            // The furthest point on this diagonal is either reached by
            // extending the one below or by extending the one above.
            let mut x = if diagonal == -distance
                || (diagonal != distance
                    && furthest[(diagonal - 1 + offset) as usize]
                        < furthest[(diagonal + 1 + offset) as usize])
            {
                furthest[(diagonal + 1 + offset) as usize]
            } else {
                furthest[(diagonal - 1 + offset) as usize] + 1
            };
            let mut y = x - diagonal;

            while (x as usize) < left_len
                && (y as usize) < right_len
                && left[x as usize] == right[y as usize]
            {
                x += 1;
                y += 1;
            }

            furthest[(diagonal + offset) as usize] = x;
            snapshot[((diagonal + distance) / 2) as usize] = x;

            if x as usize >= left_len && y as usize >= right_len {
                trace.push(snapshot);
                return Some(backtrack(&trace, distance as usize, left_len, right_len));
            }
        }

        trace.push(snapshot);
    }

    None
}

/// Walks the search back, collecting the lines that stayed the same.
fn backtrack(trace: &[Vec<i64>], final_distance: usize, left_len: usize, right_len: usize) -> Matches {
    let mut matches = Matches::new();
    let mut x = left_len as i64;
    let mut y = right_len as i64;

    for distance in (1..=final_distance).rev() {
        let previous = &trace[distance - 1];
        let previous_distance = (distance - 1) as i64;
        let diagonal = x - y;
        let index = |value: i64| ((value + previous_distance) / 2) as usize;

        let came_from = if diagonal == -(distance as i64)
            || (diagonal != distance as i64
                && previous[index(diagonal - 1)] < previous[index(diagonal + 1)])
        {
            diagonal + 1
        } else {
            diagonal - 1
        };

        let previous_x = previous[index(came_from)];
        let previous_y = previous_x - came_from;

        // The lines between the two furthest points are the ones that stayed
        // the same.
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            matches.push((x as usize, y as usize));
        }
        x = previous_x;
        y = previous_y;
    }

    // The first step of the search starts at the top left corner, so its
    // matching lines are not covered by the walk above.
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        matches.push((x as usize, y as usize));
    }

    matches.reverse();
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn align(left: &str, right: &str) -> Vec<(usize, usize)> {
        let left: Vec<&str> = left.lines().collect();
        let right: Vec<&str> = right.lines().collect();
        diff_lines(&left, &right).expect("the texts are close enough")
    }

    #[test]
    fn identical_texts_match_every_line() {
        let matches = align("a\nb\nc", "a\nb\nc");
        assert_eq!(matches, vec![(0, 0), (1, 1), (2, 2)]);
    }

    #[test]
    fn an_insertion_keeps_the_other_lines_paired() {
        let matches = align("a\nc", "a\nb\nc");
        assert_eq!(matches, vec![(0, 0), (1, 2)]);
    }

    #[test]
    fn a_deletion_keeps_the_other_lines_paired() {
        let matches = align("a\nb\nc", "a\nc");
        assert_eq!(matches, vec![(0, 0), (2, 1)]);
    }

    #[test]
    fn a_replacement_keeps_the_surrounding_lines_paired() {
        let matches = align("a\nb\nc", "a\nx\nc");
        assert_eq!(matches, vec![(0, 0), (2, 2)]);
    }

    #[test]
    fn nothing_in_common_gives_no_matches() {
        let matches = align("a\nb", "x\ny");
        assert!(matches.is_empty());
    }

    #[test]
    fn an_empty_side_matches_nothing() {
        let matches = diff_lines(&[], &["a"]).expect("the texts are close enough");
        assert!(matches.is_empty());
    }

    #[test]
    fn the_search_gives_up_on_texts_that_are_far_apart() {
        let left: Vec<String> = (0..5_000).map(|index| format!("left {index}")).collect();
        let right: Vec<String> = (0..5_000).map(|index| format!("right {index}")).collect();
        let left: Vec<&str> = left.iter().map(String::as_str).collect();
        let right: Vec<&str> = right.iter().map(String::as_str).collect();
        assert!(diff_lines(&left, &right).is_none());
    }
}
