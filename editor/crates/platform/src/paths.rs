//! Putting the agent's name for a file together with the editor's name for it.
//!
//! The agent names a file the way it sees it: relative to the project, with
//! forward slashes. The editor knows the same file by the path it opened it
//! with. A change proposed for `src/app.rs` has to find the file the editor
//! has open, or the user is asked to approve a change to a file that is not
//! in front of them.
//!
//! When two open files could be the one meant, nothing is resolved. A change
//! must never land in a file the agent did not mean.

use codeweaver_diff::same_file;

/// Finds the open file the agent's name refers to.
///
/// `open` holds the paths of the files that are open. The one that matches is
/// returned; when none does, or when more than one could, nothing is.
pub fn resolve_file(proposed: &str, open: &[String]) -> Option<String> {
    let candidates: Vec<&String> = open
        .iter()
        .filter(|path| same_file(proposed, path))
        .collect();
    match candidates.len() {
        0 => None,
        1 => Some(candidates[0].clone()),
        _ => {
            // More than one open file carries that name. Only a name with a
            // folder in it can tell them apart.
            let named = normalize(proposed);
            if !named.contains('/') {
                return None;
            }
            let deeper: Vec<&&String> = candidates
                .iter()
                .filter(|path| normalize(path).ends_with(&format!("/{named}")))
                .collect();
            (deeper.len() == 1).then(|| (*deeper[0]).clone())
        }
    }
}

/// The change the agent proposed for the file that is open.
///
/// `proposed` holds the paths of the changes the agent proposed, `active` the
/// path of the file that is in front of the user, and `open` the paths of the
/// files that are open. The proposal that names the file that is in front is
/// returned.
pub fn proposal_for_open_file(
    proposed: &[String],
    active: &str,
    open: &[String],
) -> Option<String> {
    proposed
        .iter()
        .find(|path| resolve_file(path, open).as_deref() == Some(active))
        .cloned()
}

/// A path as one name: no separator of its own, no case.
fn normalize(path: &str) -> String {
    path.trim().replace('\\', "/").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn a_relative_name_finds_the_file_that_is_open() {
        let open = paths(&["D:\\project\\src\\app.rs"]);
        assert_eq!(
            resolve_file("src/app.rs", &open),
            Some("D:\\project\\src\\app.rs".to_owned())
        );
    }

    #[test]
    fn a_name_with_folders_finds_the_file_that_is_open() {
        let open = paths(&["D:\\project\\src\\app.rs"]);
        assert_eq!(
            resolve_file("app.rs", &open),
            Some("D:\\project\\src\\app.rs".to_owned())
        );
    }

    #[test]
    fn a_name_that_is_not_open_finds_nothing() {
        let open = paths(&["D:\\project\\src\\app.rs"]);
        assert_eq!(resolve_file("src/other.rs", &open), None);
    }

    #[test]
    fn two_files_with_the_same_name_are_ambiguous() {
        let open = paths(&["D:\\a\\app.rs", "D:\\b\\app.rs"]);
        assert_eq!(resolve_file("app.rs", &open), None);
        // The folder in the name tells them apart.
        assert_eq!(
            resolve_file("b/app.rs", &open),
            Some("D:\\b\\app.rs".to_owned())
        );
    }

    #[test]
    fn the_proposal_of_the_file_in_front_is_the_one_that_counts() {
        let open = paths(&["D:\\project\\src\\app.rs", "D:\\project\\src\\other.rs"]);
        let proposed = paths(&["src/other.rs", "src/app.rs"]);
        assert_eq!(
            proposal_for_open_file(&proposed, "D:\\project\\src\\app.rs", &open),
            Some("src/app.rs".to_owned())
        );
        assert_eq!(
            proposal_for_open_file(&proposed, "D:\\project\\src\\other.rs", &open),
            Some("src/other.rs".to_owned())
        );
    }

    #[test]
    fn a_proposal_about_another_file_is_not_the_one_in_front() {
        let open = paths(&["D:\\project\\src\\app.rs"]);
        let proposed = paths(&["src/other.rs"]);
        assert_eq!(
            proposal_for_open_file(&proposed, "D:\\project\\src\\app.rs", &open),
            None
        );
    }
}
