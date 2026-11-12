//! The parts of the Language Server Protocol the editor speaks.
//!
//! The protocol is big; the editor only needs a slice of it. Everything that
//! is not in this file is simply not understood: a message the editor does
//! not know is answered with an empty result, and the server carries on.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A place in a document: a line and a column.
///
/// The column counts **UTF-16 code units**, which is what the protocol says
/// unless the server asks for another encoding. A character outside the
/// basic plane counts as two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Self {
        Position { line, character }
    }
}

/// A range in a document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// How bad a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl From<u8> for DiagnosticSeverity {
    fn from(value: u8) -> Self {
        match value {
            1 => DiagnosticSeverity::Error,
            2 => DiagnosticSeverity::Warning,
            3 => DiagnosticSeverity::Information,
            _ => DiagnosticSeverity::Hint,
        }
    }
}

impl From<DiagnosticSeverity> for u8 {
    fn from(value: DiagnosticSeverity) -> Self {
        match value {
            DiagnosticSeverity::Error => 1,
            DiagnosticSeverity::Warning => 2,
            DiagnosticSeverity::Information => 3,
            DiagnosticSeverity::Hint => 4,
        }
    }
}

/// Something the server found wrong, or worth saying.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    #[serde(default)]
    pub severity: Option<DiagnosticSeverity>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub source: Option<String>,
}

/// A document the server knows about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// A document the server knows about, with the version of its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i64,
}

/// A document that is opened in the editor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i64,
    pub text: String,
}

/// A change to a document: the text of a range was replaced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChange {
    pub range: Range,
    pub text: String,
}

/// One entry of a completion list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub label: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub kind: Option<i32>,
    #[serde(default)]
    pub insert_text: Option<String>,
    #[serde(default)]
    pub sort_text: Option<String>,
    #[serde(default)]
    pub filter_text: Option<String>,
}

impl CompletionItem {
    /// The text that goes into the document when the item is picked.
    pub fn insert_text(&self) -> &str {
        self.insert_text.as_deref().unwrap_or(&self.label)
    }

    /// The text the list is sorted and filtered by.
    pub fn filter_text(&self) -> &str {
        self.filter_text
            .as_deref()
            .or(self.sort_text.as_deref())
            .unwrap_or(&self.label)
    }
}

/// The answer to a completion request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompletionList {
    #[serde(default)]
    pub is_incomplete: bool,
    #[serde(default)]
    pub items: Vec<CompletionItem>,
}

/// What the server said it can do.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Whether the server can complete code.
    #[serde(default)]
    pub completion_provider: Option<serde_json::Value>,
}

/// The answer to the `initialize` request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InitializeResult {
    #[serde(default)]
    pub capabilities: ServerCapabilities,
}

/// The path of a file, as the protocol writes it.
///
/// The protocol uses a `file://` URI: on Windows the drive letter follows the
/// host name (`file:///C:/Users/...`), and the characters that are not allowed
/// in a URI are written as `%XX`.
pub fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        // A drive letter: the path is absolute, but it does not start with a
        // slash, and the URI needs one.
        uri.push('/');
    }
    uri.push_str(&encode_uri(&text));
    uri
}

/// The path of the file a `file://` URI stands for.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // The host is empty for a local file; a host that is not empty is a
    // remote file, which the editor does not open.
    let rest = match rest.find('/') {
        Some(0) => rest,
        _ => return None,
    };
    let mut bytes = Vec::new();
    let mut raw = Vec::new();
    let mut chars = rest.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                bytes.push(byte);
                continue;
            }
        }
        raw.extend(character.to_string().bytes());
    }
    if !bytes.is_empty() {
        raw.extend(bytes);
    }
    let text = String::from_utf8(raw).ok()?;
    Some(PathBuf::from(text))
}

/// Writes the characters that a URI does not allow as `%XX`.
fn encode_uri(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_becomes_a_uri() {
        assert_eq!(
            file_uri(Path::new("C:/Users/ferris/main.rs")),
            "file:///C:/Users/ferris/main.rs"
        );
    }

    #[test]
    fn a_uri_becomes_a_path() {
        assert_eq!(
            uri_to_path("file:///C:/Users/ferris/main.rs"),
            Some(PathBuf::from("C:/Users/ferris/main.rs"))
        );
    }

    #[test]
    fn a_uri_with_spaces_becomes_a_path_with_spaces() {
        assert_eq!(
            uri_to_path("file:///C:/my%20code/main.rs"),
            Some(PathBuf::from("C:/my code/main.rs"))
        );
    }

    #[test]
    fn a_path_with_spaces_becomes_a_uri_with_escapes() {
        assert_eq!(
            file_uri(Path::new("C:/my code/main.rs")),
            "file:///C:/my%20code/main.rs"
        );
    }

    #[test]
    fn a_uri_that_is_not_a_file_is_refused() {
        assert_eq!(uri_to_path("https://example.com/main.rs"), None);
    }
}
