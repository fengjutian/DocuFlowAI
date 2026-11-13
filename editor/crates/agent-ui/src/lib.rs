//! The agent panel: the editor's own model of an agent session.
//!
//! The panel is what the UI draws. It holds the text the agent produced, the
//! tools it called, the permission it asked for and whether the turn is still
//! running. The transport lives in `codeweaver-agent-acp`; this module turns
//! its events into something the editor can render, and it is where the
//! approval flow and the Diff preview are built.

mod panel;

pub use panel::{
    AgentPanel, ContextItem, ContextSource, DiffProposal, HunkDecision, HunkEntry, PendingPermission,
    ToolEntry, TraceEntry, TraceKind,
};
