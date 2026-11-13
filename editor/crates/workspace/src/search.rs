//! Finding text in the files of a project.
//!
//! The search walks the project itself: it does not ask the language server,
//! because the server only knows the files that are open, and the editor has
//! to find text in every file of the project.
//!
//! Two rules decide what is searched:
//!
//! * The directories that hold build products are not walked into: `target`,
//!   `node_modules`, `dist` and the like. They hold thousands of files that
//!   nobody is looking for.
//! * The files that are not text are not read: a picture, an archive or a
//!   database holds bytes, and the text in them is not the text of the
//!   project.
//!
//! The hidden files *are* searched: the workflows of `.github` are part of
//! the project, and a search that missed them would miss a lot.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::tree;

/// The biggest file that is read, in bytes.
///
/// A file that is bigger than this is nearly always a picture, an archive or
/// a database. None of those hold the text a person is looking for.
pub const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;

/// The most places that are collected.
///
/// A search that finds a million places is not a result a person can use,
/// and the list of them would cost more memory than the search itself.
pub const MAX_HITS: usize = 1000;

/// How much of a file is looked at to tell whether it holds text.
const BINARY_SNIFF_BYTES: usize = 8000;

/// One place where the text was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    /// The file the place is in.
    pub path: PathBuf,
    /// The line the place is on, counted from one.
    pub line: usize,
    /// The text of the line, without its line break.
    pub text: String,
}

/// How a search is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchOptions {
    /// Whether the case of the letters counts.
    pub case_sensitive: bool,
    /// The most places that are collected.
    pub max_hits: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            case_sensitive: false,
            max_hits: MAX_HITS,
        }
    }
}

/// What a search found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// The places the text was found.
    pub hits: Vec<SearchHit>,
    /// How many files were read.
    pub files: usize,
    /// Whether the search stopped because there were too many places.
    pub truncated: bool,
}

/// Searches for `needle` in the files of the project rooted at `root`.
///
/// The places are in the order of the files, and the lines are in the order
/// they are in the file. A directory that cannot be read is skipped: one
/// unreadable folder does not stop the whole search.
pub fn search(root: &Path, needle: &str, options: SearchOptions) -> SearchResults {
    let mut results = walk(root, needle, options);
    // The places are sorted by the file they are in: a list in the order the
    // disk gave the files is not a list a person can read.
    results.hits.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line.cmp(&right.line))
    });
    results
}

/// Walks the project and collects the places the text is in.
fn walk(root: &Path, needle: &str, options: SearchOptions) -> SearchResults {
    let mut results = SearchResults::default();
    if needle.is_empty() {
        return results;
    }
    let matcher = Matcher::new(needle, options.case_sensitive);
    let ignored = tree::ignored_directories();

    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                if !tree::is_ignored_directory(&name, &ignored) {
                    directories.push(entry.path());
                }
                continue;
            }
            if !metadata.is_file() || metadata.len() > MAX_FILE_SIZE {
                continue;
            }
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            if holds_no_text(&bytes) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                // Not UTF-8: not the text of the project.
                continue;
            };
            results.files += 1;
            for (index, line) in text.lines().enumerate() {
                if !matcher.finds(line) {
                    continue;
                }
                results.hits.push(SearchHit {
                    path: entry.path(),
                    line: index + 1,
                    text: line.to_owned(),
                });
                if results.hits.len() >= options.max_hits {
                    results.truncated = true;
                    return results;
                }
            }
        }
    }
    results
}

/// The text that is looked for, and the way it is compared.
struct Matcher {
    /// The text as it was written.
    needle: String,
    /// The text in lower case, for the search that does not count the case.
    lower: String,
    /// Whether the case of the letters counts.
    case_sensitive: bool,
}

impl Matcher {
    /// Makes a matcher for the given text.
    fn new(needle: &str, case_sensitive: bool) -> Self {
        Matcher {
            needle: needle.to_owned(),
            lower: needle.to_lowercase(),
            case_sensitive,
        }
    }

    /// Whether the line holds the text.
    fn finds(&self, line: &str) -> bool {
        if self.case_sensitive {
            line.contains(&self.needle)
        } else {
            line.to_lowercase().contains(&self.lower)
        }
    }
}

/// Whether the bytes are a file of pictures and other things that are not text.
///
/// A zero byte is what tells a text file from a picture: a text file has
/// none in its first few kilobytes.
fn holds_no_text(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    head.contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of scratch files, empty and unique per check.
    fn scratch(name: &str) -> PathBuf {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-scratch")
            .join(name);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("the scratch directory can be created");
        directory
    }

    /// Writes a file, making the directories above it.
    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("the scratch directory can be created");
        }
        fs::write(path, text).expect("the scratch file can be written");
    }

    #[test]
    fn the_text_is_found_in_the_files_of_the_project() {
        let root = scratch("search-finds-the-text");
        write_file(&root.join("notes.txt"), "the first line\nthe needle is here\n");
        write_file(&root.join("src").join("main.rs"), "fn main() {}\n");

        let results = search(&root, "needle", SearchOptions::default());

        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.hits[0].path, root.join("notes.txt"));
        assert_eq!(results.hits[0].line, 2);
        assert_eq!(results.hits[0].text, "the needle is here");
        assert_eq!(results.files, 2, "both files are read");
        assert!(!results.truncated);
    }

    #[test]
    fn the_places_are_in_the_order_of_the_files() {
        let root = scratch("search-sorts-the-places");
        write_file(&root.join("b.txt"), "the needle is here\n");
        write_file(&root.join("a.txt"), "the needle is here\n");

        let results = search(&root, "needle", SearchOptions::default());

        assert_eq!(results.hits.len(), 2);
        assert_eq!(results.hits[0].path, root.join("a.txt"));
        assert_eq!(results.hits[1].path, root.join("b.txt"));
    }

    #[test]
    fn the_line_of_a_place_is_counted_from_one() {
        let root = scratch("search-counts-the-lines");
        write_file(&root.join("a.txt"), "one\ntwo\nthree\n");

        let results = search(&root, "three", SearchOptions::default());

        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.hits[0].line, 3);
    }

    #[test]
    fn every_place_in_a_line_is_reported() {
        let root = scratch("search-reports-every-place");
        write_file(&root.join("a.txt"), "needle and needle again\n");

        let results = search(&root, "needle", SearchOptions::default());

        assert_eq!(results.hits.len(), 1, "a line is one place");
        assert_eq!(results.hits[0].text, "needle and needle again");
    }

    #[test]
    fn the_case_of_the_letters_does_not_count_by_default() {
        let root = scratch("search-ignores-the-case");
        write_file(&root.join("a.txt"), "The Needle is here\n");

        let results = search(&root, "needle", SearchOptions::default());

        assert_eq!(results.hits.len(), 1);
    }

    #[test]
    fn the_case_of_the_letters_counts_when_it_is_asked_for() {
        let root = scratch("search-counts-the-case");
        write_file(&root.join("a.txt"), "The Needle is here\n");

        let results = search(
            &root,
            "needle",
            SearchOptions {
                case_sensitive: true,
                ..SearchOptions::default()
            },
        );

        assert!(results.hits.is_empty(), "got {:?}", results.hits);
    }

    #[test]
    fn the_directories_of_build_products_are_not_searched() {
        let root = scratch("search-skips-the-build-products");
        write_file(&root.join("a.txt"), "the needle is here\n");
        write_file(&root.join("target").join("b.txt"), "the needle is here\n");
        write_file(&root.join("node_modules").join("c.txt"), "the needle is here\n");

        let results = search(&root, "needle", SearchOptions::default());

        assert_eq!(results.hits.len(), 1, "got {:?}", results.hits);
        assert_eq!(results.hits[0].path, root.join("a.txt"));
    }

    #[test]
    fn the_hidden_files_are_searched() {
        let root = scratch("search-searches-the-hidden-files");
        write_file(&root.join(".github").join("ci.yml"), "the needle is here\n");

        let results = search(&root, "needle", SearchOptions::default());

        assert_eq!(results.hits.len(), 1, "got {:?}", results.hits);
    }

    #[test]
    fn a_file_that_is_not_text_is_not_read() {
        let root = scratch("search-skips-the-files-that-are-not-text");
        fs::write(root.join("a.bin"), [0x6E, 0x65, 0x65, 0x64, 0x6C, 0x65, 0x00]).unwrap();
        write_file(&root.join("b.txt"), "the needle is here\n");

        let results = search(&root, "needle", SearchOptions::default());

        assert_eq!(results.hits.len(), 1, "got {:?}", results.hits);
        assert_eq!(results.hits[0].path, root.join("b.txt"));
        assert_eq!(results.files, 1, "only the text file is read");
    }

    #[test]
    fn a_file_that_is_not_utf8_is_not_read() {
        let root = scratch("search-skips-the-files-that-are-not-utf8");
        fs::write(root.join("a.txt"), [0xFF, 0xFE, 0x6E, 0x65, 0x65, 0x64]).unwrap();

        let results = search(&root, "needle", SearchOptions::default());

        assert!(results.hits.is_empty(), "got {:?}", results.hits);
        assert_eq!(results.files, 0);
    }

    #[test]
    fn there_is_nothing_when_the_text_is_not_there() {
        let root = scratch("search-finds-nothing");
        write_file(&root.join("a.txt"), "something else\n");

        let results = search(&root, "needle", SearchOptions::default());

        assert!(results.hits.is_empty());
        assert_eq!(results.files, 1, "the file is still read");
    }

    #[test]
    fn an_empty_text_is_not_looked_for() {
        let root = scratch("search-of-nothing");
        write_file(&root.join("a.txt"), "the needle is here\n");

        let results = search(&root, "", SearchOptions::default());

        assert!(results.hits.is_empty());
        assert_eq!(results.files, 0, "nothing is read");
    }

    #[test]
    fn the_number_of_places_is_capped() {
        let root = scratch("search-caps-the-places");
        let mut text = String::new();
        for _ in 0..10 {
            text.push_str("the needle is here\n");
        }
        write_file(&root.join("a.txt"), &text);

        let results = search(
            &root,
            "needle",
            SearchOptions {
                max_hits: 3,
                ..SearchOptions::default()
            },
        );

        assert_eq!(results.hits.len(), 3);
        assert!(results.truncated, "the search says that it stopped early");
    }
}
