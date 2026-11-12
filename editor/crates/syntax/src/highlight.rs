//! What the highlighter makes of a piece of text.

use std::ops::Range;

/// The kinds of text that the highlighter tells apart.
///
/// The kinds are what a theme gives a colour to. They are deliberately few:
/// a theme that has a colour for every kind of token is a theme nobody can
/// read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    /// A word that the language gives a meaning to, like `fn` or `if`.
    Keyword,
    /// The name of a function or a method.
    Function,
    /// The name of a type.
    Type,
    /// Text inside quotes.
    String,
    /// A number.
    Number,
    /// A comment.
    Comment,
    /// An operator, like `+` or `=>`.
    Operator,
    /// Punctuation, like a comma or a bracket.
    Punctuation,
    /// A value that cannot change, like `true` or a constant.
    Constant,
    /// A name of something that holds a value.
    Variable,
    /// A field of an object.
    Property,
    /// The name of a constructor.
    Constructor,
    /// A label that a jump goes to.
    Label,
    /// An escape sequence inside a string.
    Escape,
    /// An annotation, like `#[derive]`.
    Attribute,
    /// The name of a module or a namespace.
    Module,
}

impl HighlightKind {
    /// The kind that a capture of a query stands for.
    ///
    /// The grammars name their captures after the tree of `nvim-treesitter`:
    /// `function.method` is a method, `type.builtin` is a built-in type. The
    /// part before the first dot says what the text is.
    ///
    /// Returns `None` for a capture that says nothing about the text, such as
    /// the captures that other programs use.
    pub fn from_capture(name: &str) -> Option<HighlightKind> {
        let kind = name.split('.').next().unwrap_or(name);
        Some(match kind {
            "keyword" => HighlightKind::Keyword,
            "function" => HighlightKind::Function,
            "type" => HighlightKind::Type,
            "string" => HighlightKind::String,
            "number" => HighlightKind::Number,
            "comment" => HighlightKind::Comment,
            "operator" => HighlightKind::Operator,
            "punctuation" => HighlightKind::Punctuation,
            "constant" => HighlightKind::Constant,
            "variable" => HighlightKind::Variable,
            "property" => HighlightKind::Property,
            "constructor" => HighlightKind::Constructor,
            "label" => HighlightKind::Label,
            "escape" => HighlightKind::Escape,
            "attribute" => HighlightKind::Attribute,
            "module" => HighlightKind::Module,
            // A note that is left in the code is a comment to the reader.
            "todo" => HighlightKind::Comment,
            _ => return None,
        })
    }
}

/// A piece of text that is drawn in its own colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Highlight {
    /// The byte range of the text in the document.
    pub range: Range<usize>,
    /// What the text is.
    pub kind: HighlightKind,
}
