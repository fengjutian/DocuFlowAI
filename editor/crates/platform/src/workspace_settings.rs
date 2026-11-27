//! The settings of a workspace, and the settings of the user.
//!
//! There are two layers of settings:
//!
//! - the file of the user (`%APPDATA%\CodeWeaver\settings.json`), which says
//!   how the editor looks for everybody, on every project;
//! - the `settings` of the workspace file, which says how the editor looks
//!   for the one project that file is about.
//!
//! The **workspace wins**. A person who writes a setting down in a workspace
//! file writes it down for that one project, and not for every project they
//! open afterwards. That is the way VS Code has it, and it is the reason a
//! project can be handed to somebody else with a size that suits them.
//!
//! The two layers do not use the same names, and they do not use the same
//! unit: the workspace file is the shape VS Code writes, where the size of
//! the text is `editor.fontSize` in **pixels**; the file of the user is the
//! shape this editor writes, where it is `fontSize` in **percent**. They say
//! the same thing — 16 pixels is the usual size, which is 100 percent — and
//! the change between the two is made here, in one place.

use std::collections::BTreeMap;

use crate::zoom::{Zoom, DEFAULT_FONT_SIZE};

/// The name of the setting that says how big the text is.
pub const FONT_SIZE: &str = "editor.fontSize";

/// The settings of one layer.
///
/// The values are the way the file writes them: the workspace file turns
/// every value into a string when it reads the file, and a number is read
/// back from that string here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    values: BTreeMap<String, String>,
}

impl Settings {
    /// The settings of this layer.
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Settings { values }
    }

    /// The value of one setting, the way the file writes it.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// The size of the text this layer asks for.
    ///
    /// `Ok(None)` means the layer does not say anything about the size.
    /// `Err` means it does say something, and what it says cannot be used:
    /// the reason is in the error, because a setting that is broken must be
    /// named and not swallowed.
    ///
    /// The size in the workspace file is in pixels; the usual size is 16
    /// pixels, which is 100 percent.
    pub fn font_size(&self) -> Result<Option<Zoom>, String> {
        let Some(value) = self.get(FONT_SIZE) else {
            return Ok(None);
        };
        match value.trim().parse::<f32>() {
            Ok(pixels) => Ok(Some(Zoom::of(pixels / DEFAULT_FONT_SIZE * 100.0))),
            Err(_) => Err(format!("{FONT_SIZE}（{value}）不是一个数字，没有用上")),
        }
    }

    /// The settings of this layer that the editor does not know.
    ///
    /// A setting that is not known is not wrong: it may be a setting of
    /// another editor. It is named all the same, so that the person knows it
    /// has no effect here.
    pub fn unknown(&self) -> Vec<&str> {
        self.values
            .keys()
            .map(String::as_str)
            .filter(|key| *key != FONT_SIZE)
            .collect()
    }
}

/// The settings that are in force.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolved {
    /// The size of the text that is in force.
    pub zoom: Zoom,
    /// Whether the size comes from the workspace rather than from the user.
    pub from_the_workspace: bool,
    /// What could not be used, one line per mistake.
    pub errors: Vec<String>,
    /// The settings of the workspace that the editor does not know.
    pub unknown: Vec<String>,
}

/// Works out the settings that are in force.
///
/// The workspace wins over the user. A value that cannot be used is **named**
/// and the setting of the user is used instead: a broken setting must not
/// take the editor down with it.
pub fn resolve(user: Zoom, workspace: Option<&Settings>) -> Resolved {
    let mut resolved = Resolved {
        zoom: user,
        ..Resolved::default()
    };
    let Some(workspace) = workspace else {
        return resolved;
    };
    resolved.unknown = workspace
        .unknown()
        .iter()
        .map(|key| (*key).to_owned())
        .collect();
    match workspace.font_size() {
        Ok(None) => {}
        Ok(Some(size)) => {
            resolved.zoom = size;
            resolved.from_the_workspace = true;
        }
        Err(error) => resolved.errors.push(error),
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> Settings {
        let mut values = BTreeMap::new();
        for (key, value) in pairs {
            values.insert((*key).to_owned(), (*value).to_owned());
        }
        Settings::new(values)
    }

    #[test]
    fn a_layer_without_a_size_leaves_the_size_alone() {
        let layer = settings(&[("editor.tabSize", "4")]);
        assert_eq!(layer.font_size().expect("the size is read"), None);
    }

    #[test]
    fn the_size_of_the_workspace_is_read_in_pixels() {
        // 16 pixels is the usual size, which is 100 percent.
        let layer = settings(&[(FONT_SIZE, "16")]);
        let size = layer
            .font_size()
            .expect("the size is read")
            .expect("the layer says what the size is");
        assert_eq!(size.percent, 100.0);
        // 20 pixels is a quarter bigger.
        let layer = settings(&[(FONT_SIZE, "20")]);
        let size = layer
            .font_size()
            .expect("the size is read")
            .expect("the layer says what the size is");
        assert_eq!(size.percent, 125.0);
    }

    #[test]
    fn a_size_that_is_not_a_number_is_named() {
        let layer = settings(&[(FONT_SIZE, "big")]);
        let said = layer.font_size().expect_err("the size is not a number");
        assert!(said.contains("editor.fontSize"), "got {said}");
        assert!(said.contains("big"), "got {said}");
    }

    #[test]
    fn the_settings_the_editor_does_not_know_are_named() {
        let layer = settings(&[(FONT_SIZE, "16"), ("editor.tabSize", "4")]);
        let unknown = layer.unknown();
        assert_eq!(unknown, vec!["editor.tabSize"]);
    }

    #[test]
    fn without_a_workspace_the_size_of_the_user_is_in_force() {
        let user = Zoom::of(120.0);
        let resolved = resolve(user, None);
        assert_eq!(resolved.zoom.percent, 120.0);
        assert!(!resolved.from_the_workspace, "the user says what the size is");
        assert!(resolved.errors.is_empty());
        assert!(resolved.unknown.is_empty());
    }

    #[test]
    fn the_workspace_wins_over_the_user() {
        let user = Zoom::of(120.0);
        let workspace = settings(&[(FONT_SIZE, "20")]);
        let resolved = resolve(user, Some(&workspace));
        assert_eq!(resolved.zoom.percent, 125.0, "the workspace says what the size is");
        assert!(resolved.from_the_workspace);
        assert!(resolved.errors.is_empty());
    }

    #[test]
    fn a_workspace_that_says_nothing_about_the_size_leaves_the_users_size() {
        let user = Zoom::of(120.0);
        let workspace = settings(&[("editor.tabSize", "4")]);
        let resolved = resolve(user, Some(&workspace));
        assert_eq!(resolved.zoom.percent, 120.0);
        assert!(!resolved.from_the_workspace);
        assert_eq!(resolved.unknown, vec!["editor.tabSize".to_owned()]);
    }

    #[test]
    fn a_broken_size_of_the_workspace_falls_back_to_the_size_of_the_user() {
        let user = Zoom::of(120.0);
        let workspace = settings(&[(FONT_SIZE, "big")]);
        let resolved = resolve(user, Some(&workspace));
        assert_eq!(resolved.zoom.percent, 120.0, "the size of the user is in force");
        assert!(!resolved.from_the_workspace);
        assert_eq!(resolved.errors.len(), 1);
        assert!(resolved.errors[0].contains("editor.fontSize"), "got {:?}", resolved.errors);
    }
}
