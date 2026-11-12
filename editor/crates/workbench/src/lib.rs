//! The workbench: the tab strip of the editor.
//!
//! The workbench owns what the user sees as *tabs*: which files are open,
//! which one is in front, and which of them have unsaved changes. It does not
//! own the text of those files — the buffers belong to the editor core, and
//! the window belongs to the platform.
//!
//! Keeping the model here means the rules about tabs can be tested without a
//! window: a tab that is closed while it is dirty, a file that is opened twice,
//! a tab that is closed while it is in front — all of them are decided here and
//! all of them have a test.

pub mod tabs;

pub use tabs::{CloseDecision, Tab, TabBar, TabId, TabKind, close_decision};
