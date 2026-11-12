//! Syntax highlighting with tree-sitter.
//!
//! The crate turns a buffer into the ranges of text that are drawn in a
//! different colour: the keyword of a function, the name of a type, the text
//! of a comment.
//!
//! The work is split in two:
//!
//! - [`Syntax::update`] parses the buffer and keeps the syntax tree. When
//!   the change to the buffer is known, the tree of the last parse is
//!   followed instead of parsing the whole document again.
//! - [`Syntax::highlights`] turns the tree into the ranges of text that are
//!   highlighted. Only the range that is asked for is looked at, so the cost
//!   follows the size of the screen and not the size of the document.

pub mod highlight;
pub mod language;

use std::ops::Range;

use codeweaver_text::{Buffer, BufferChange, BufferVersion};
use tree_sitter::{InputEdit, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

pub use highlight::{Highlight, HighlightKind};
pub use language::Language;

/// The syntax tree of a document, and the highlights it gives.
pub struct Syntax {
    parser: Parser,
    language: Option<Language>,
    query: Option<Query>,
    tree: Option<Tree>,
    /// The version of the buffer that the tree was parsed from.
    version: Option<BufferVersion>,
    /// The highlights of the last range that was asked for.
    highlights: Vec<Highlight>,
    /// The range that the highlights cover.
    computed: Option<Range<usize>>,
}

impl Default for Syntax {
    fn default() -> Self {
        Self::new()
    }
}

impl Syntax {
    pub fn new() -> Self {
        Syntax {
            parser: Parser::new(),
            language: None,
            query: None,
            tree: None,
            version: None,
            highlights: Vec::new(),
            computed: None,
        }
    }

    /// The language that is highlighted, if any.
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// Sets the language that is highlighted.
    ///
    /// Returns `true` when the language changed. The tree of the old language
    /// is then gone, and the document has to be parsed again.
    pub fn set_language(&mut self, language: Option<Language>) -> bool {
        if self.language == language {
            return false;
        }
        self.language = language;
        self.tree = None;
        self.version = None;
        self.highlights.clear();
        self.computed = None;

        self.query = language.and_then(|language| {
            let grammar = language.grammar();
            let source = language.queries().join("\n");
            match Query::new(&grammar, &source) {
                Ok(query) => Some(query),
                Err(error) => {
                    eprintln!(
                        "the queries of {} do not fit its grammar: {error}",
                        language.name()
                    );
                    None
                }
            }
        });
        if let Some(language) = language {
            let grammar = language.grammar();
            if let Err(error) = self.parser.set_language(&grammar) {
                eprintln!(
                    "the grammar of {} does not fit the parser: {error}",
                    language.name()
                );
            }
        }
        true
    }

    /// Parses the buffer, and keeps the tree for the next change.
    ///
    /// `change` says what the buffer looks like now compared to the last
    /// parse. Without it, or with a change that the tree cannot follow, the
    /// whole document is parsed again.
    ///
    /// Returns `true` when the document was parsed again.
    pub fn update(&mut self, buffer: &Buffer, change: Option<&BufferChange>) -> bool {
        if self.language.is_none() {
            self.tree = None;
            self.version = None;
            self.highlights.clear();
            self.computed = None;
            return false;
        }
        if self.version == Some(buffer.version()) {
            return false;
        }

        // The tree of the last parse can only be followed when the change is
        // known exactly: the parser has to know where the text moved. A
        // change of more than one edit moves the text in more than one place,
        // and the tree is left alone then.
        let mut tree = self.tree.take();
        let mut followed = false;
        if let (Some(change), Some(previous)) = (change, tree.as_mut())
            && change.edits.len() == 1
            && let Some(edit) = input_edit(buffer, change)
        {
            previous.edit(&edit);
            followed = true;
        }
        if !followed {
            tree = None;
        }

        let parsed = {
            let parser = &mut self.parser;
            parse(parser, buffer, tree.as_ref())
        };
        self.tree = parsed;
        self.version = Some(buffer.version());
        // The highlights were made from the tree that is gone now.
        self.highlights.clear();
        self.computed = None;
        true
    }

    /// The highlights of the byte range.
    ///
    /// The highlights are made from the tree of the last parse, so the buffer
    /// has to be parsed first with [`Syntax::update`].
    pub fn highlights(&mut self, buffer: &Buffer, range: Range<usize>) -> &[Highlight] {
        if self.computed.as_ref() != Some(&range) {
            self.compute(buffer, range);
        }
        &self.highlights
    }

    /// Makes the highlights of the byte range.
    fn compute(&mut self, buffer: &Buffer, range: Range<usize>) {
        self.highlights.clear();
        self.computed = Some(range.clone());
        let Some(query) = self.query.as_ref() else {
            return;
        };
        let Some(tree) = self.tree.as_ref() else {
            return;
        };

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range);
        let mut captures = cursor.captures(query, tree.root_node(), NodeText { buffer });

        let mut found: Vec<Candidate> = Vec::new();
        while let Some((matched, index)) = captures.next() {
            let capture = matched.captures[*index];
            let name = query.capture_names()[capture.index as usize];
            let Some(kind) = HighlightKind::from_capture(name) else {
                continue;
            };
            let range = capture.node.byte_range();
            if range.is_empty() {
                continue;
            }
            found.push(Candidate {
                range,
                kind,
                pattern: matched.pattern_index,
            });
        }

        // The shortest capture wins: the escape inside a string is more
        // specific than the string around it. A tie goes to the capture that
        // comes last in the query, because that is the one that was written
        // to be the more specific of the two.
        found
            .sort_by_key(|candidate| (candidate.range.len(), std::cmp::Reverse(candidate.pattern)));
        let mut claimed: Vec<Range<usize>> = Vec::new();
        for candidate in found {
            for piece in subtract(&candidate.range, &claimed) {
                self.highlights.push(Highlight {
                    range: piece,
                    kind: candidate.kind,
                });
            }
            let at = claimed.partition_point(|taken| taken.start <= candidate.range.start);
            claimed.insert(at, candidate.range);
        }
        self.highlights
            .sort_by_key(|highlight| highlight.range.start);
    }
}

/// A capture that says something about a piece of text.
struct Candidate {
    range: Range<usize>,
    kind: HighlightKind,
    pattern: usize,
}

/// The parts of the range that no other capture has claimed yet.
///
/// The claimed ranges are in the order of the text.
fn subtract(range: &Range<usize>, claimed: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut pieces = Vec::new();
    let mut start = range.start;
    for taken in claimed {
        if taken.end <= start {
            continue;
        }
        if taken.start >= range.end {
            break;
        }
        if taken.start > start {
            pieces.push(start..taken.start);
        }
        start = start.max(taken.end);
        if start >= range.end {
            return pieces;
        }
    }
    if start < range.end {
        pieces.push(start..range.end);
    }
    pieces
}

/// Parses the buffer.
///
/// The text is read in the pieces the buffer is made of, so the whole
/// document is never copied into one string.
fn parse(parser: &mut Parser, buffer: &Buffer, old_tree: Option<&Tree>) -> Option<Tree> {
    let mut read = |offset: usize, _position: Point| -> &[u8] { buffer.slice_from(offset) };
    parser.parse_with_options(&mut read, old_tree, None)
}

/// The tree-sitter edit that goes with a change to the buffer.
fn input_edit(buffer: &Buffer, change: &BufferChange) -> Option<InputEdit> {
    let edit = change.edits.first()?;
    let replaced = change.replaced.first()?;
    let start = edit.range.start.0;
    let start_position = point_at(buffer, start)?;
    Some(InputEdit {
        start_byte: start,
        old_end_byte: edit.range.end.0,
        new_end_byte: start + edit.replacement.len(),
        start_position,
        old_end_position: advance(start_position, replaced),
        new_end_position: advance(start_position, &edit.replacement),
    })
}

/// The position of a byte offset in the buffer.
fn point_at(buffer: &Buffer, offset: usize) -> Option<Point> {
    let line = buffer.line_of_offset(offset);
    let line_start = buffer.line_range(line).start.0;
    Some(Point {
        row: line,
        column: offset.saturating_sub(line_start),
    })
}

/// The position that comes after the text, when the text starts at `start`.
fn advance(start: Point, text: &str) -> Point {
    let mut row = start.row;
    let mut column = start.column;
    for character in text.chars() {
        if character == '\n' {
            row += 1;
            column = 0;
        } else {
            column += character.len_utf8();
        }
    }
    Point { row, column }
}

/// Hands the text of a node to a query, without copying it.
struct NodeText<'a> {
    buffer: &'a Buffer,
}

impl<'a> tree_sitter::TextProvider<&'a [u8]> for NodeText<'a> {
    type I = std::vec::IntoIter<&'a [u8]>;

    fn text(&mut self, node: Node) -> Self::I {
        let mut pieces = Vec::new();
        let mut offset = node.start_byte();
        let end = node.end_byte();
        while offset < end {
            let piece = self.buffer.slice_from(offset);
            if piece.is_empty() {
                break;
            }
            let take = piece.len().min(end - offset);
            pieces.push(&piece[..take]);
            offset += take;
        }
        pieces.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_text::{TextEdit, TextOffset, Transaction};

    /// A syntax that highlights the given language.
    fn syntax_of(language: Language) -> Syntax {
        let mut syntax = Syntax::new();
        syntax.set_language(Some(language));
        syntax
    }

    /// The kinds of the text of a document, in the order of the text.
    fn kinds_of(syntax: &mut Syntax, text: &str) -> Vec<(String, HighlightKind)> {
        let buffer = Buffer::new(text);
        syntax.update(&buffer, None);
        let len = buffer.len().0;
        syntax
            .highlights(&buffer, 0..len)
            .iter()
            .map(|highlight| (text[highlight.range.clone()].to_owned(), highlight.kind))
            .collect()
    }

    #[test]
    fn every_language_has_queries_that_fit_its_grammar() {
        for language in [
            Language::Rust,
            Language::Python,
            Language::JavaScript,
            Language::TypeScript,
            Language::Tsx,
            Language::Json,
            Language::C,
            Language::Cpp,
            Language::Go,
        ] {
            let grammar = language.grammar();
            let source = language.queries().join("\n");
            let query = Query::new(&grammar, &source);
            assert!(
                query.is_ok(),
                "the queries of {} do not fit its grammar: {:?}",
                language.name(),
                query.err()
            );
        }
    }

    #[test]
    fn a_language_is_found_by_the_name_of_the_file() {
        assert_eq!(
            Language::from_path(std::path::Path::new("src/main.rs")),
            Some(Language::Rust)
        );
        assert_eq!(
            Language::from_path(std::path::Path::new("Cargo.toml")),
            Some(Language::Rust)
        );
        assert_eq!(
            Language::from_path(std::path::Path::new("app/page.tsx")),
            Some(Language::Tsx)
        );
        assert_eq!(Language::from_path(std::path::Path::new("notes")), None);
    }

    #[test]
    fn the_keywords_of_rust_are_keywords() {
        let mut syntax = syntax_of(Language::Rust);
        let kinds = kinds_of(&mut syntax, "fn main() {}\n");
        assert_eq!(kinds[0], ("fn".to_owned(), HighlightKind::Keyword));
        assert!(
            kinds
                .iter()
                .any(|(text, kind)| text == "main" && *kind == HighlightKind::Function)
        );
    }

    #[test]
    fn the_comments_of_rust_are_comments() {
        let mut syntax = syntax_of(Language::Rust);
        let kinds = kinds_of(&mut syntax, "// hello\nfn main() {}\n");
        assert_eq!(kinds[0], ("// hello".to_owned(), HighlightKind::Comment));
    }

    #[test]
    fn the_strings_of_rust_are_strings() {
        let mut syntax = syntax_of(Language::Rust);
        let kinds = kinds_of(&mut syntax, "let a = \"text\";\n");
        assert!(
            kinds
                .iter()
                .any(|(text, kind)| text == "\"text\"" && *kind == HighlightKind::String)
        );
    }

    #[test]
    fn the_highlights_do_not_cover_each_other() {
        let mut syntax = syntax_of(Language::Rust);
        let text = "fn main() {\n    let x: u32 = 42;\n    println!(\"{x}\");\n}\n";
        let buffer = Buffer::new(text);
        syntax.update(&buffer, None);
        let highlights = syntax.highlights(&buffer, 0..buffer.len().0);
        for (index, highlight) in highlights.iter().enumerate() {
            if index == 0 {
                continue;
            }
            let previous = &highlights[index - 1];
            assert!(
                previous.range.end <= highlight.range.start,
                "{previous:?} and {highlight:?} overlap"
            );
        }
    }

    #[test]
    fn the_tree_follows_a_change_to_the_text() {
        let mut syntax = syntax_of(Language::Rust);
        let mut buffer = Buffer::new("fn main() {}\n");
        syntax.update(&buffer, None);

        let applied = buffer
            .apply(Transaction::new(
                buffer.version(),
                vec![TextEdit::new(
                    TextOffset(13)..TextOffset(13),
                    "let x = 1;\n",
                )],
            ))
            .unwrap();
        syntax.update(&buffer, Some(&applied.change));

        let text = buffer.text();
        let kinds = kinds_of(&mut syntax, &text);
        assert!(
            kinds
                .iter()
                .any(|(word, kind)| word == "let" && *kind == HighlightKind::Keyword),
            "the tree did not follow the change: {kinds:?}"
        );
    }

    #[test]
    fn a_change_that_cannot_be_followed_still_parses() {
        let mut syntax = syntax_of(Language::Rust);
        let mut buffer = Buffer::new("fn main() {}\n");
        syntax.update(&buffer, None);

        // Two edits at once move the text in two places, so the tree is
        // parsed again from scratch.
        let applied = buffer
            .apply(Transaction::new(
                buffer.version(),
                vec![
                    TextEdit::new(TextOffset(0)..TextOffset(2), "let"),
                    TextEdit::new(TextOffset(3)..TextOffset(7), "let"),
                ],
            ))
            .unwrap();
        syntax.update(&buffer, Some(&applied.change));

        let text = buffer.text();
        let kinds = kinds_of(&mut syntax, &text);
        assert!(kinds.iter().any(|(word, _)| word == "let"));
    }
}
