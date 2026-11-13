//! The language server of the open document.
//!
//! Which server runs for a file is a matter of the language, and the server
//! is told about the document in the protocol's own terms: the places in a
//! document are lines and columns counted in UTF-16 code units, not bytes.

use std::ops::Range as TextRange;
use std::path::{Path, PathBuf};

use codeweaver_lsp::{
    ServerConfig, positions,
    protocol::{Diagnostic, DiagnosticSeverity, Position, Range, TextChange, TextEdit},
};
use codeweaver_syntax::Language;
use codeweaver_text::{Buffer, BufferChange, TextOffset};

use crate::editor_view::{Mark, MarkSeverity};

/// The language server of a language, if the machine has one.
pub fn server_for(language: Language) -> Option<ServerConfig> {
    match language {
        Language::Rust => Some(ServerConfig::new("rust-analyzer", Vec::new())),
        Language::Go => Some(ServerConfig::new("gopls", Vec::new())),
        _ => None,
    }
}

/// The name the protocol uses for a language.
pub fn language_id(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::Python => "python",
        Language::JavaScript => "javascript",
        Language::TypeScript => "typescript",
        Language::Tsx => "typescriptreact",
        Language::Json => "json",
        Language::C => "c",
        Language::Cpp => "cpp",
        Language::Go => "go",
    }
}

/// The directory the server should treat as the root of the workspace.
///
/// It is the directory of the project the file belongs to: the folder that
/// holds the manifest. A file that belongs to no project gets the folder it
/// is in.
pub fn workspace_root(path: &Path) -> PathBuf {
    let mut directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
    loop {
        if directory.join("Cargo.toml").is_file()
            || directory.join("go.mod").is_file()
            || directory.join("package.json").is_file()
            || directory.join("pyproject.toml").is_file()
        {
            return directory;
        }
        if !directory.pop() {
            break;
        }
    }
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// The range of the line the given offset is on.
///
/// The code operations of a server are asked about a place in the document,
/// and the line of the caret is the place the user is working on.
pub fn line_range(buffer: &Buffer, offset: usize) -> Range {
    let line = buffer.line_of_offset(offset);
    let text = buffer.line_text(line);
    let start = buffer.line_range(line).start.0;
    let end = start + text.len();
    Range {
        start: positions::position_at(buffer, start),
        end: positions::position_at(buffer, end),
    }
}

/// The changes the server is told about.
///
/// One edit is told as a range and the text that goes there: the range is
/// the one of the text *before* the edit, and the text that was there is
/// known, so the place of the end of the range can be worked out.
///
/// More than one edit moves the text in more than one place, and the places
/// of the old text cannot be worked out after the fact, so the whole
/// document goes.
pub fn changes_for(buffer: &Buffer, change: &BufferChange) -> Vec<TextChange> {
    if change.edits.len() == 1 {
        let edit = &change.edits[0];
        let replaced = &change.replaced[0];
        let start = positions::position_at(buffer, edit.range.start.0);
        let end = positions::advance(start, replaced);
        return vec![TextChange::in_range(
            Range { start, end },
            edit.replacement.clone(),
        )];
    }
    vec![TextChange::whole(buffer.text())]
}

/// The things the server found wrong, in the editor's own words.
///
/// The places the server gives are in its own units, so they are worked out
/// against the text that is there now.
pub fn marks_for(buffer: &Buffer, diagnostics: &[Diagnostic]) -> Vec<Mark> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let start = positions::offset_at(buffer, diagnostic.range.start)?;
            let end = positions::offset_at(buffer, diagnostic.range.end)?;
            Some(Mark {
                range: start..end,
                severity: severity_of(diagnostic.severity),
            })
        })
        .collect()
}

/// The edits of a formatting answer, as ranges of the buffer.
///
/// The places the server gives are in its own units, so they are worked out
/// against the text that is there now. A place that is not in the document
/// means the answer is about a different text, and it is not applied at all:
/// a formatting that lands in the wrong place is worse than no formatting.
pub fn formatting_changes(
    buffer: &Buffer,
    edits: &[TextEdit],
) -> Option<Vec<(TextRange<TextOffset>, String)>> {
    changes_of(buffer, edits)
}

/// The edits of a rename answer, as ranges of the buffer.
///
/// The places the server gives are in its own units, so they are worked out
/// against the text that is there now. A place that is not in the document
/// means the answer is about a different text, and it is not applied at all:
/// a renaming that lands in the wrong place is worse than no renaming.
pub fn rename_changes(
    buffer: &Buffer,
    edits: &[TextEdit],
) -> Option<Vec<(TextRange<TextOffset>, String)>> {
    changes_of(buffer, edits)
}

/// The edits of an answer, as ranges of the buffer.
fn changes_of(
    buffer: &Buffer,
    edits: &[TextEdit],
) -> Option<Vec<(TextRange<TextOffset>, String)>> {
    let lines = buffer.line_count();
    let mut changes = Vec::with_capacity(edits.len());
    for edit in edits {
        // A place that is in a line the file does not have is not a place in
        // the file: the answer is about another text.
        if edit.range.start.line as usize >= lines || edit.range.end.line as usize >= lines {
            return None;
        }
        let start = positions::offset_at(buffer, edit.range.start)?;
        let end = positions::offset_at(buffer, edit.range.end)?;
        changes.push((TextOffset(start)..TextOffset(end), edit.new_text.clone()));
    }
    Some(changes)
}

/// How bad a thing is, in the editor's own words.
fn severity_of(severity: Option<DiagnosticSeverity>) -> MarkSeverity {
    match severity {
        Some(DiagnosticSeverity::Error) => MarkSeverity::Error,
        Some(DiagnosticSeverity::Warning) => MarkSeverity::Warning,
        Some(DiagnosticSeverity::Information) => MarkSeverity::Information,
        _ => MarkSeverity::Hint,
    }
}

/// The place of the caret, in the protocol's own words.
pub fn position_of(buffer: &Buffer, offset: usize) -> Position {
    positions::position_at(buffer, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_text::{TextEdit, TextOffset, Transaction};

    #[test]
    fn the_root_of_a_workspace_is_the_folder_with_the_manifest() {
        // The manifest has to be there for the walk to find it.
        let mut fixture = std::env::current_exe().expect("the test binary has a path");
        fixture.pop();
        if fixture.ends_with("deps") {
            fixture.pop();
        }
        let project = fixture.join("workspace-fixture").join("project");
        std::fs::create_dir_all(project.join("src")).expect("the fixture is made");
        std::fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\n",
        )
        .expect("the manifest is written");

        let root = workspace_root(&project.join("src").join("main.rs"));
        assert_eq!(root, project);

        let _ = std::fs::remove_dir_all(fixture.join("workspace-fixture"));
    }

    #[test]
    fn one_edit_is_told_as_a_range() {
        let mut buffer = Buffer::new("fn main() {\n    let x = 1;\n}\n");
        let applied = buffer
            .apply(Transaction::new(
                buffer.version(),
                vec![TextEdit::new(TextOffset(12)..TextOffset(15), "let")],
            ))
            .unwrap();
        let changes = changes_for(&buffer, &applied.change);
        assert_eq!(changes.len(), 1);
        let range = changes[0].range.expect("the change has a range");
        assert_eq!(range.start, Position::new(1, 0));
        // The old text was "let", three characters on the second line.
        assert_eq!(range.end, Position::new(1, 3));
        assert_eq!(changes[0].text, "let");
    }

    #[test]
    fn more_than_one_edit_sends_the_whole_document() {
        let mut buffer = Buffer::new("fn main() {}\n");
        let applied = buffer
            .apply(Transaction::new(
                buffer.version(),
                vec![
                    TextEdit::new(TextOffset(0)..TextOffset(2), "let"),
                    TextEdit::new(TextOffset(3)..TextOffset(7), "let"),
                ],
            ))
            .unwrap();
        let changes = changes_for(&buffer, &applied.change);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].text, buffer.text());
        // Without a range, the whole document is replaced.
        assert_eq!(changes[0].range, None);
    }

    #[test]
    fn a_diagnostic_becomes_a_mark_on_the_text() {
        let buffer = Buffer::new("fn main() {\n    let x = 1;\n}\n");
        let diagnostic = Diagnostic {
            range: Range {
                start: Position::new(1, 4),
                end: Position::new(1, 5),
            },
            severity: Some(DiagnosticSeverity::Error),
            message: "cannot find value `x`".to_owned(),
            source: Some("rustc".to_owned()),
        };
        let marks = marks_for(&buffer, &[diagnostic]);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].range, 16..17);
        assert_eq!(marks[0].severity, MarkSeverity::Error);
    }
}
