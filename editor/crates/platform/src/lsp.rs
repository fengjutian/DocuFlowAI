//! The language server of the open document.
//!
//! Which server runs for a file is a matter of the language, and the server
//! is told about the document in the protocol's own terms: the places in a
//! document are lines and columns counted in UTF-16 code units, not bytes.

use std::path::{Path, PathBuf};

use codeweaver_lsp::{
    ServerConfig,
    positions,
    protocol::{Position, Range, TextChange},
};
use codeweaver_syntax::Language;
use codeweaver_text::{Buffer, BufferChange};

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
        return vec![TextChange {
            range: Range { start, end },
            text: edit.replacement.clone(),
        }];
    }
    vec![TextChange {
        range: Range {
            start: Position::new(0, 0),
            end: positions::position_at(buffer, buffer.len().0),
        },
        text: buffer.text(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_text::{TextEdit, TextOffset, Transaction};

    #[test]
    fn the_root_of_a_workspace_is_the_folder_with_the_manifest() {
        let root = workspace_root(Path::new("C:/code/project/src/main.rs"));
        assert_eq!(root, PathBuf::from("C:/code/project"));
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
        assert_eq!(changes[0].range.start, Position::new(1, 0));
        // The old text was "let", three characters on the second line.
        assert_eq!(changes[0].range.end, Position::new(1, 3));
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
        assert_eq!(changes[0].range.start, Position::new(0, 0));
    }
}
