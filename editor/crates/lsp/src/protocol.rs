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
///
/// The range is optional. Without it, the whole document is replaced by the
/// text, which is what the protocol calls *full* synchronisation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    pub text: String,
}

impl TextChange {
    /// The text of a range was replaced by other text.
    pub fn in_range(range: Range, text: impl Into<String>) -> Self {
        TextChange {
            range: Some(range),
            text: text.into(),
        }
    }

    /// The whole document was replaced by other text.
    pub fn whole(text: impl Into<String>) -> Self {
        TextChange {
            range: None,
            text: text.into(),
        }
    }
}

/// How the server wants to be told about changes to a document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextDocumentSyncKind {
    /// The server does not want to be told about changes at all.
    #[default]
    None,
    /// The client sends the whole document after every change.
    Full,
    /// The client sends the range that changed.
    Incremental,
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
    #[serde(default, rename = "completionProvider")]
    pub completion_provider: Option<serde_json::Value>,
    /// How the server wants to be told about changes to a document.
    #[serde(
        default,
        rename = "textDocumentSync",
        deserialize_with = "read_sync_kind"
    )]
    pub text_document_sync: TextDocumentSyncKind,
}

/// The answer to the `initialize` request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InitializeResult {
    #[serde(default)]
    pub capabilities: ServerCapabilities,
}

/// Reads how the server wants to be told about changes.
///
/// The capability is either the kind itself or an object that holds it
/// together with the other things the server wants to be told.
fn read_sync_kind<'de, D>(deserializer: D) -> Result<TextDocumentSyncKind, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let kind = match &value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::Object(fields) => {
            fields.get("change").and_then(|change| change.as_i64())
        }
        _ => None,
    };
    Ok(match kind {
        Some(1) => TextDocumentSyncKind::Full,
        Some(2) => TextDocumentSyncKind::Incremental,
        _ => TextDocumentSyncKind::None,
    })
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
    let rest = rest.strip_prefix('/')?;
    // A Windows path begins with a drive letter, which the URI writes after
    // a slash: `file:///C:/code`. The slash is not part of the path.
    let rest = match rest.strip_prefix('/') {
        Some(rest) if is_drive_letter(rest) => rest,
        _ => rest,
    };

    let mut bytes: Vec<u8> = Vec::with_capacity(rest.len());
    let mut chars = rest.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                bytes.push(byte);
                continue;
            }
            // Not an escape after all: the percent sign belongs to the path.
            bytes.push(b'%');
            bytes.extend(hex.as_bytes());
            continue;
        }
        let mut encoded = [0u8; 4];
        bytes.extend(character.encode_utf8(&mut encoded).as_bytes());
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(PathBuf::from(text))
}

/// Whether the text begins with a drive letter, like `C:`.
fn is_drive_letter(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Writes the characters that a URI does not allow as `%XX`.
fn encode_uri(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            // A colon is allowed in a path: it is what a Windows drive
            // letter is made of.
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
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

    #[test]
    fn the_sync_kind_is_read_from_the_answer_of_the_server() {
        // The server may say the kind itself or hold it in an object with
        // the other things it wants to be told.
        let plain: InitializeResult =
            serde_json::from_str(r#"{"capabilities":{"textDocumentSync":2}}"#).unwrap();
        assert_eq!(
            plain.capabilities.text_document_sync,
            TextDocumentSyncKind::Incremental
        );

        let full: InitializeResult =
            serde_json::from_str(r#"{"capabilities":{"textDocumentSync":1}}"#).unwrap();
        assert_eq!(
            full.capabilities.text_document_sync,
            TextDocumentSyncKind::Full
        );

        let options: InitializeResult = serde_json::from_str(
            r#"{"capabilities":{"textDocumentSync":{"openClose":true,"change":2}}}"#,
        )
        .unwrap();
        assert_eq!(
            options.capabilities.text_document_sync,
            TextDocumentSyncKind::Incremental
        );

        // A server that says nothing is asked for the whole document.
        let quiet: InitializeResult = serde_json::from_str(r#"{"capabilities":{}}"#).unwrap();
        assert_eq!(
            quiet.capabilities.text_document_sync,
            TextDocumentSyncKind::None
        );
    }
}
