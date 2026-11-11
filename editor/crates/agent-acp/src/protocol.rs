//! ACP protocol types, limited to what the editor needs.
//!
//! These mirror the agent endpoint's contract in
//! `codeweaver-hermes-lab/src/acp/protocol.ts`. The types here are the
//! *wire* types: they exist so the editor can talk to the endpoint, and they
//! are deliberately separate from the editor's own event model so that an ACP
//! upgrade does not leak into the rest of the editor.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// A JSON-RPC request id.
pub type RequestId = i64;

/// Why a turn ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The agent finished its turn.
    EndTurn,
    /// The turn hit a token limit.
    MaxTokens,
    /// The turn hit a request limit.
    MaxTurnRequests,
    /// The agent refused.
    Refusal,
    /// The turn was cancelled.
    Cancelled,
    /// A reason the endpoint reported that this client does not know.
    Unknown,
}

impl StopReason {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "max_turn_requests" => StopReason::MaxTurnRequests,
            "refusal" => StopReason::Refusal,
            "cancelled" => StopReason::Cancelled,
            _ => StopReason::Unknown,
        }
    }

    /// Whether the turn ended without producing a usable result.
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            StopReason::Refusal | StopReason::MaxTokens | StopReason::MaxTurnRequests
        )
    }
}

/// A content block inside a prompt.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptBlock {
    Text { text: String },
}

/// A message chunk the agent streamed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageChunk {
    pub text: String,
}

/// The status of a tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    InProgress,
    Completed,
    Failed,
}

impl ToolStatus {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "completed" => ToolStatus::Completed,
            "failed" => ToolStatus::Failed,
            _ => ToolStatus::InProgress,
        }
    }
}

/// A tool call the agent started or updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub status: ToolStatus,
}

/// One option of a permission request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// A permission request from the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequest {
    pub session_id: String,
    pub tool_call_id: String,
    pub title: String,
    pub options: Vec<PermissionOption>,
}

/// The result of `initialize`.
///
/// The endpoint speaks camelCase on the wire; the Rust fields stay in the
/// project's snake_case style.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[serde(default)]
    pub protocol_version: u32,
    #[serde(default)]
    pub agent_info: Option<AgentInfo>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

/// The result of `session/new`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreated {
    pub session_id: String,
}

/// The result of `session/prompt`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCompleted {
    pub stop_reason: String,
}

impl PromptCompleted {
    pub fn reason(&self) -> StopReason {
        StopReason::from_wire(&self.stop_reason)
    }
}

/// A `session/update` payload.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionUpdate {
    Message(MessageChunk),
    ToolCall(ToolCall),
    Unknown(String),
}

impl SessionUpdate {
    /// Parses one `session/update` update.
    ///
    /// The endpoint emits `agent_message_chunk`, `tool_call` and
    /// `tool_call_update`; anything else is kept as `Unknown` so a newer
    /// endpoint does not break the editor.
    pub fn parse(value: &Value) -> Option<Self> {
        let kind = value.get("sessionUpdate")?.as_str()?;
        match kind {
            "agent_message_chunk" => {
                let text = value
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .and_then(|text| text.as_str())
                    .unwrap_or_default();
                Some(SessionUpdate::Message(MessageChunk {
                    text: text.to_owned(),
                }))
            }
            "tool_call" | "tool_call_update" => {
                let tool_call_id = value
                    .get("toolCallId")
                    .and_then(|id| id.as_str())
                    .unwrap_or_default()
                    .to_owned();
                let title = value
                    .get("title")
                    .and_then(|title| title.as_str())
                    .unwrap_or_default()
                    .to_owned();
                let status = value
                    .get("status")
                    .and_then(|status| status.as_str())
                    .map(ToolStatus::from_wire)
                    .unwrap_or(ToolStatus::InProgress);
                Some(SessionUpdate::ToolCall(ToolCall {
                    tool_call_id,
                    title,
                    status,
                }))
            }
            other => Some(SessionUpdate::Unknown(other.to_owned())),
        }
    }
}

/// The outcome of a permission request, as the editor answers it.
#[derive(Clone, Debug, Serialize)]
pub struct PermissionOutcome {
    pub outcome: String,
    pub option_id: String,
}

impl PermissionOutcome {
    /// The outcome shape the endpoint expects for a chosen option.
    pub fn selected(option_id: impl Into<String>) -> Self {
        PermissionOutcome {
            outcome: "selected".to_owned(),
            option_id: option_id.into(),
        }
    }

    /// The outcome shape for a cancelled request.
    pub fn cancelled() -> Self {
        PermissionOutcome {
            outcome: "cancelled".to_owned(),
            option_id: String::new(),
        }
    }
}

/// Serializes a `PermissionOutcome` into the wire shape the endpoint reads.
///
/// The endpoint accepts either `{ outcome: { outcome, optionId } }` or a flat
/// `{ outcome, optionId }`; the nested shape is what its own runner produces,
/// so the editor sends that.
pub fn permission_result(outcome: PermissionOutcome) -> Value {
    serde_json::json!({
        "outcome": {
            "outcome": outcome.outcome,
            "optionId": outcome.option_id,
        }
    })
}
