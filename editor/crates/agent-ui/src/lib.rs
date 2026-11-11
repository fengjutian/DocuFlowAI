//! The agent panel: the editor's own model of an agent session.
//!
//! The panel is what the UI draws. It holds the text the agent produced, the
//! tools it called, the permission it asked for and whether the turn is still
//! running. The transport lives in `codeweaver-agent-acp`; this crate turns
//! its events into something the editor can render, and it is where the
//! approval flow and the Diff preview will be built.

mod panel;

pub use panel::{AgentPanel, PendingPermission, ToolEntry};
