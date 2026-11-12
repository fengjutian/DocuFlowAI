//! One open document, and everything that belongs to it.
//!
//! The editor used to have exactly one document: the text, the view of it and
//! the highlights were fields of the window. With more than one file open at a
//! time, all of that belongs to the *file*, not to the window — so it lives
//! here, and the window keeps a list of panes.
//!
//! A pane owns:
//!
//! - the text and its history (`Editor`),
//! - the file on disk, when there is one (`Document`),
//! - the scroll position and the layout of the text (`EditorView`),
//! - the highlighter, which runs on a thread of its own (`SyntaxWorker`),
//! - what the language server said about the document.

use std::ops::Range;

use codeweaver_editor_core::Editor;
use codeweaver_syntax::{Highlight, Language, SyntaxWorker};
use codeweaver_text::BufferVersion;
use codeweaver_workspace::Document;
use codeweaver_workbench::TabId;

use crate::editor_view::{EditorView, Mark};

/// One open document.
pub struct EditorPane {
    /// The tab this pane belongs to.
    pub tab: TabId,
    /// The text of the document and the history of the changes to it.
    pub editor: Editor,
    /// The file on disk, when the document has one.
    pub document: Option<Document>,
    /// The scroll position and the layout of the text.
    pub view: EditorView,
    /// The highlighter, on a thread of its own.
    pub syntax: SyntaxWorker,
    /// The highlights of the last answer of the highlighter.
    pub highlights: Vec<Highlight>,
    /// Whether the document is too big to be highlighted.
    pub degraded: bool,
    /// What the language server found wrong in the document.
    pub marks: Vec<Mark>,
    /// How many of the marks are errors.
    pub errors: usize,
    /// How many of the marks are warnings.
    pub warnings: usize,
    /// The URI of the document, as the language server knows it.
    pub document_uri: Option<String>,
    /// The version the language server was told about.
    pub server_version: Option<u64>,
    /// The version and the range the highlighter was last asked about.
    ///
    /// The highlighter is asked again when the text or the visible part
    /// changed; a scroll is a new range, a keystroke is a new version.
    pub asked: Option<(BufferVersion, Range<usize>)>,
}

impl EditorPane {
    /// A pane for a document that is not on disk yet.
    pub fn scratch(tab: TabId, language: Option<Language>) -> Self {
        EditorPane {
            tab,
            editor: Editor::new(""),
            document: None,
            view: EditorView::new(),
            syntax: SyntaxWorker::new(language),
            highlights: Vec::new(),
            degraded: false,
            marks: Vec::new(),
            errors: 0,
            warnings: 0,
            document_uri: None,
            server_version: None,
            asked: None,
        }
    }

    /// A pane for a file that was opened.
    pub fn opened(tab: TabId, document: Document, language: Option<Language>) -> Self {
        let editor = Editor::new(document.saved_text());
        EditorPane {
            tab,
            editor,
            document: Some(document),
            view: EditorView::new(),
            syntax: SyntaxWorker::new(language),
            highlights: Vec::new(),
            degraded: false,
            marks: Vec::new(),
            errors: 0,
            warnings: 0,
            document_uri: None,
            server_version: None,
            asked: None,
        }
    }

    /// Whether the document has changes that are not on disk yet.
    ///
    /// The answer is read from the text and the file, not remembered: a
    /// remembered flag is wrong the moment something else touches the buffer.
    pub fn is_dirty(&self) -> bool {
        match self.document.as_ref() {
            Some(document) => document.is_dirty(&self.editor.text()),
            None => !self.editor.text().is_empty(),
        }
    }
}
