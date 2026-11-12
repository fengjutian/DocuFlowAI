//! The languages the editor knows how to highlight.

use std::path::Path;

use tree_sitter::Language as Grammar;

/// The queries of Rust.
const RUST: &str = include_str!("../queries/rust.scm");
/// The queries of Python.
const PYTHON: &str = include_str!("../queries/python.scm");
/// The queries of JavaScript.
const JAVASCRIPT: &str = include_str!("../queries/javascript.scm");
/// The queries that TypeScript adds to the ones of JavaScript.
const TYPESCRIPT: &str = include_str!("../queries/typescript.scm");
/// The queries of JSON.
const JSON: &str = include_str!("../queries/json.scm");
/// The queries of C.
const C: &str = include_str!("../queries/c.scm");
/// The queries that C++ adds to the ones of C.
const CPP: &str = include_str!("../queries/cpp.scm");
/// The queries of Go.
const GO: &str = include_str!("../queries/go.scm");

/// A language that the editor can highlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Json,
    C,
    Cpp,
    Go,
}

impl Language {
    /// The language of a file, by its name.
    ///
    /// Some files are named after the language they hold, and their name
    /// says more than their extension does.
    pub fn from_path(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_str()?;
        match name {
            "Cargo.toml" => return Some(Language::Rust),
            "package.json" | "tsconfig.json" => return Some(Language::Json),
            _ => {}
        }
        let extension = path.extension()?.to_str()?;
        Language::from_extension(extension)
    }

    /// The language of a file, by its extension.
    pub fn from_extension(extension: &str) -> Option<Language> {
        Some(match extension {
            "rs" => Language::Rust,
            "py" | "pyi" => Language::Python,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "json" => Language::Json,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
            "go" => Language::Go,
            _ => return None,
        })
    }

    /// The name of the language, as it is shown to the user.
    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::Json => "JSON",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Go => "Go",
        }
    }

    /// The grammar that parses the language.
    pub fn grammar(self) -> Grammar {
        match self {
            Language::Rust => Grammar::new(tree_sitter_rust::LANGUAGE),
            Language::Python => Grammar::new(tree_sitter_python::LANGUAGE),
            Language::JavaScript => Grammar::new(tree_sitter_javascript::LANGUAGE),
            Language::TypeScript => Grammar::new(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
            Language::Tsx => Grammar::new(tree_sitter_typescript::LANGUAGE_TSX),
            Language::Json => Grammar::new(tree_sitter_json::LANGUAGE),
            Language::C => Grammar::new(tree_sitter_c::LANGUAGE),
            Language::Cpp => Grammar::new(tree_sitter_cpp::LANGUAGE),
            Language::Go => Grammar::new(tree_sitter_go::LANGUAGE),
        }
    }

    /// The queries that say what to highlight.
    ///
    /// A language can need more than one file. The queries of TypeScript and
    /// of C++ only hold what is special about them; the rest comes from the
    /// language they are built on. Tree-sitter has no way of saying that
    /// itself, so the queries are put together here.
    pub fn queries(self) -> Vec<&'static str> {
        match self {
            Language::Rust => vec![RUST],
            Language::Python => vec![PYTHON],
            Language::JavaScript => vec![JAVASCRIPT],
            Language::TypeScript | Language::Tsx => vec![JAVASCRIPT, TYPESCRIPT],
            Language::Json => vec![JSON],
            Language::C => vec![C],
            Language::Cpp => vec![C, CPP],
            Language::Go => vec![GO],
        }
    }
}
