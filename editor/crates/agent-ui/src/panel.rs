//! The agent panel state: what the editor knows about the agent's turn.
//!
//! The panel is the editor's own model of an ACP session. It holds the text
//! the agent produced, the tools it called, the permission it asked for and
//! whether the turn is still running. The transport lives in
//! `codeweaver-agent-acp`; this module turns its events into something the UI
//! can draw.

use codeweaver_agent_acp::{PermissionOutcome, SessionEvent, StopReason, ToolStatus};

/// The state of one tool call in the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolEntry {
    pub tool_call_id: String,
    pub title: String,
    pub status: ToolStatus,
}

/// A permission the agent is waiting for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPermission {
    pub request_id: i64,
    pub tool_call_id: String,
    pub title: String,
    pub options: Vec<String>,
}

/// The editor's view of the agent session.
#[derive(Clone, Debug, Default)]
pub struct AgentPanel {
    /// The text the agent has produced so far.
    pub transcript: String,
    /// The tools the agent called, in the order they appeared.
    pub tools: Vec<ToolEntry>,
    /// The permission the agent is waiting for, if any.
    pub pending_permission: Option<PendingPermission>,
    /// Whether a turn is running.
    pub running: bool,
    /// Why the last turn ended.
    pub last_stop_reason: Option<StopReason>,
    /// A problem that ended the session.
    pub error: Option<String>,
}

impl AgentPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one event from the agent.
    pub fn apply(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Message { text } => {
                self.transcript.push_str(&text);
                self.error = None;
            }
            SessionEvent::ToolCall {
                tool_call_id,
                title,
                status,
            } => {
                match self
                    .tools
                    .iter_mut()
                    .find(|tool| tool.tool_call_id == tool_call_id)
                {
                    Some(existing) => {
                        existing.status = status;
                        if !title.is_empty() {
                            existing.title = title;
                        }
                    }
                    None => self.tools.push(ToolEntry {
                        tool_call_id,
                        title,
                        status,
                    }),
                }
            }
            SessionEvent::PermissionRequested {
                request_id,
                request,
            } => {
                self.pending_permission = Some(PendingPermission {
                    request_id,
                    tool_call_id: request.tool_call_id,
                    title: request.title,
                    options: request
                        .options
                        .iter()
                        .map(|option| option.option_id.clone())
                        .collect(),
                });
            }
            SessionEvent::TurnCompleted { stop_reason } => {
                self.running = false;
                self.last_stop_reason = Some(stop_reason);
                // A turn that ends cannot still be waiting for a decision.
                self.pending_permission = None;
            }
            SessionEvent::Disconnected { reason } => {
                self.running = false;
                self.error = Some(reason);
            }
        }
    }

    /// Starts a new turn, clearing the previous transcript.
    pub fn begin_turn(&mut self) {
        self.transcript.clear();
        self.tools.clear();
        self.pending_permission = None;
        self.last_stop_reason = None;
        self.error = None;
        self.running = true;
    }

    /// The answer to the pending permission request, if there is one.
    pub fn pending_permission_answer(&self, option_id: &str) -> Option<PermissionOutcome> {
        self.pending_permission
            .as_ref()
            .map(|_| PermissionOutcome::selected(option_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeweaver_agent_acp::{PermissionOption, PermissionRequest};

    #[test]
    fn collects_the_transcript_and_the_tools() {
        let mut panel = AgentPanel::new();
        panel.begin_turn();
        panel.apply(SessionEvent::Message {
            text: "hello ".to_owned(),
        });
        panel.apply(SessionEvent::Message {
            text: "world".to_owned(),
        });
        panel.apply(SessionEvent::ToolCall {
            tool_call_id: "call-1".to_owned(),
            title: "read_file".to_owned(),
            status: ToolStatus::InProgress,
        });
        panel.apply(SessionEvent::ToolCall {
            tool_call_id: "call-1".to_owned(),
            title: String::new(),
            status: ToolStatus::Completed,
        });

        assert_eq!(panel.transcript, "hello world");
        assert_eq!(panel.tools.len(), 1);
        assert_eq!(panel.tools[0].status, ToolStatus::Completed);
        assert_eq!(panel.tools[0].title, "read_file");
    }

    #[test]
    fn a_finished_turn_clears_the_pending_permission() {
        let mut panel = AgentPanel::new();
        panel.begin_turn();
        panel.apply(SessionEvent::PermissionRequested {
            request_id: 7,
            request: PermissionRequest {
                session_id: "s".to_owned(),
                tool_call_id: "call-9".to_owned(),
                title: "apply_diff".to_owned(),
                options: vec![PermissionOption {
                    option_id: "allow".to_owned(),
                    name: "Allow".to_owned(),
                    kind: "allow_once".to_owned(),
                }],
            },
        });
        assert!(panel.pending_permission.is_some());

        panel.apply(SessionEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
        });
        assert!(panel.pending_permission.is_none());
        assert!(!panel.running);
    }

    #[test]
    fn a_disconnect_is_reported_and_stops_the_turn() {
        let mut panel = AgentPanel::new();
        panel.begin_turn();
        panel.apply(SessionEvent::Disconnected {
            reason: "the agent endpoint closed its output".to_owned(),
        });
        assert!(!panel.running);
        assert!(panel.error.is_some());
    }
}
