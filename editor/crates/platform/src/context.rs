//! What the editor offers the agent as context.
//!
//! The user decides what the agent gets to see. The editor offers three
//! kinds of context — the file that is open, the part of it that is
//! selected, and the other files that are open — and sends only the ones
//! the user picked.
//!
//! Secret-bearing files are never offered. A file called `.env` or
//! `credentials` holds things that must not leave the machine just because
//! a file happened to be open next to the one the user is working on.

use codeweaver_agent_ui::{ContextItem, ContextSource};
use codeweaver_editor_core::Editor;

/// The name of a file that holds something secret.
fn is_secret(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    if name == ".env" || name.starts_with(".env.") {
        return true;
    }
    if name.starts_with("credential") || name.starts_with("secret") {
        return true;
    }
    if name.ends_with(".pem") || name.ends_with(".key") {
        return true;
    }
    if name.starts_with("id_rsa") || name.starts_with("id_ed25519") {
        return true;
    }
    name == ".npmrc" || name == ".pypirc"
}

/// The context the editor offers for the given state.
///
/// `selection` is the text the user has selected, if any. `tabs` are the
/// other open files, as `(path, text)`.
pub fn build_context(
    current: Option<(&str, &str)>,
    selection: Option<(&str, &str)>,
    tabs: &[(&str, &str)],
) -> Vec<ContextItem> {
    let mut items: Vec<ContextItem> = Vec::new();

    if let Some((path, text)) = current
        && !is_secret(path)
    {
        items.push(ContextItem {
            id: format!("file:{path}"),
            source: ContextSource::CurrentFile,
            path: path.to_owned(),
            text: text.to_owned(),
            selected: true,
        });
    }

    if let Some((path, text)) = selection
        && !text.is_empty()
        && !is_secret(path)
    {
        items.push(ContextItem {
            id: format!("selection:{path}"),
            source: ContextSource::Selection,
            path: path.to_owned(),
            text: text.to_owned(),
            selected: false,
        });
    }

    for (path, text) in tabs {
        if is_secret(path) {
            continue;
        }
        items.push(ContextItem {
            id: format!("tab:{path}"),
            source: ContextSource::OtherTab,
            path: (*path).to_owned(),
            text: (*text).to_owned(),
            selected: false,
        });
    }

    items
}

/// The text of the selection, if the editor has one.
pub fn selection_text(editor: &Editor) -> Option<String> {
    let range = editor.selection().range(editor.buffer());
    if range.start == range.end {
        return None;
    }
    Some(editor.buffer().slice(range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_file_is_offered_and_selected() {
        let items = build_context(Some(("src/app.rs", "fn main() {}")), None, &[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source, ContextSource::CurrentFile);
        assert!(items[0].selected);
    }

    #[test]
    fn the_selection_is_offered_but_not_selected() {
        let items = build_context(
            Some(("src/app.rs", "fn main() {}")),
            Some(("src/app.rs", "fn main()")),
            &[],
        );
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].source, ContextSource::Selection);
        assert!(!items[1].selected);
    }

    #[test]
    fn the_other_tabs_are_offered() {
        let items = build_context(
            Some(("a.rs", "a")),
            None,
            &[("b.rs", "b"), ("c.rs", "c")],
        );
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].source, ContextSource::OtherTab);
        assert_eq!(items[1].path, "b.rs");
    }

    #[test]
    fn a_secret_file_is_never_offered() {
        for path in [
            ".env",
            ".env.local",
            "credentials.json",
            "secrets.yml",
            "server.pem",
            "private.key",
            "id_rsa",
            ".npmrc",
            ".pypirc",
        ] {
            let items = build_context(Some((path, "secret")), None, &[(path, "secret")]);
            assert!(items.is_empty(), "{path} was offered");
        }
    }

    #[test]
    fn a_file_that_only_looks_secret_is_offered() {
        let items = build_context(Some(("src/environment.rs", "code")), None, &[]);
        assert_eq!(items.len(), 1);
    }
}
