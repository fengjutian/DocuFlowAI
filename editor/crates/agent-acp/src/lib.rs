//! ACP client for the CodeWeaver editor.
//!
//! The editor talks to the CodeWeaver agent endpoint over ACP: JSON-RPC 2.0
//! on the endpoint's stdio. This crate owns the transport, the protocol types
//! and the mapping from protocol updates to the editor's own event model.
//!
//! The layering follows the plan's rule that protocol objects and internal
//! objects stay separate: [`protocol`] holds the wire types, [`client`] holds
//! the transport, and [`SessionEvent`] is what the rest of the editor sees.
//! An ACP upgrade therefore cannot leak into the editor core.

mod client;
mod framing;
mod protocol;

pub use client::{AcpError, Client, EndpointConfig, SessionEvent};
pub use framing::{FrameReader, Framing};
pub use protocol::{
    permission_result, AgentInfo, InitializeResult, MessageChunk, PermissionOption,
    PermissionOutcome, PermissionRequest, PromptBlock, PromptCompleted, SessionCreated,
    SessionUpdate, StopReason, ToolCall, ToolStatus, PROTOCOL_VERSION,
};
