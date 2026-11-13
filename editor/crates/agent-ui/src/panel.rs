//! The agent panel: the editor's own model of an agent session.
//!
//! The panel is what the UI draws. It holds the text the agent produced, the
//! tools it called, the permission it asked for and whether the turn is still
//! running. The transport lives in `codeweaver-agent-acp`; this module turns
//! its events into something the editor can render, and it is where the
//! approval flow and the Diff preview are built.
//!
//! Three things live here that the protocol does not carry:
//!
//! - **The context selection**: what the user chose to send with the prompt.
//!   It is the user's decision, so it lives in the editor and not in the
//!   session.
//! - **The diff review**: the hunks of a proposed change and what the user
//!   decided about each of them. The decision is made here, not by the
//!   agent.
//! - **The trace**: what happened during the session, in the order it
//!   happened, so a run can be read back afterwards.

use std::collections::{BTreeMap, BTreeSet};

use codeweaver_agent_acp::{
    PermissionOutcome, SessionEvent, StopReason, ToolCallContent, ToolStatus,
};

/// The state of one tool call in the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolEntry {
    pub tool_call_id: String,
    pub title: String,
    pub status: ToolStatus,
    /// What kind of tool this is.
    pub kind: String,
}

/// A permission the agent is waiting for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPermission {
    pub request_id: i64,
    pub tool_call_id: String,
    pub title: String,
    pub options: Vec<String>,
}

/// Where a piece of context comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSource {
    /// The file that is open.
    CurrentFile,
    /// The part of the file that is selected.
    Selection,
    /// Another file that is open in a tab.
    OtherTab,
}

/// One piece of context the user can send with the prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextItem {
    /// A stable id, so the selection survives a redraw.
    pub id: String,
    /// Where it comes from.
    pub source: ContextSource,
    /// The path it belongs to, for the user to recognise it.
    pub path: String,
    /// The text that would be sent.
    pub text: String,
    /// Whether the user chose to send it.
    pub selected: bool,
}

impl ContextItem {
    /// The label the panel shows for this item.
    pub fn label(&self) -> String {
        match self.source {
            ContextSource::CurrentFile => format!("当前文件 · {}", self.path),
            ContextSource::Selection => format!("选区 · {}", self.path),
            ContextSource::OtherTab => format!("标签 · {}", self.path),
        }
    }

    /// How many bytes the item would send.
    pub fn bytes(&self) -> usize {
        self.text.len()
    }
}

/// What the user decided about one block of a proposed change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkDecision {
    /// Not decided yet.
    Pending,
    /// Approved.
    Approved,
    /// Rejected.
    Rejected,
}

/// One block of a proposed change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HunkEntry {
    /// The line the block starts at in the file, counting from one.
    pub line: usize,
    /// The lines the block adds.
    pub added: Vec<String>,
    /// The lines the block removes.
    pub removed: Vec<String>,
    /// What the user decided.
    pub decision: HunkDecision,
}

impl HunkEntry {
    /// A short description of the block, for the panel.
    pub fn summary(&self) -> String {
        format!("+{} -{}", self.added.len(), self.removed.len())
    }
}

/// A change the agent proposes, waiting for the user's decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffProposal {
    /// The path the change is about.
    pub path: String,
    /// The blocks of the change, in order.
    pub hunks: Vec<HunkEntry>,
    /// The version of the buffer the change was computed against.
    ///
    /// It is `None` when the agent did not say; the editor then assumes the
    /// change was computed against what is there now.
    pub base_version: Option<u64>,
    /// The text the change was computed against.
    pub base_text: Option<String>,
}

impl DiffProposal {
    /// How many blocks are approved.
    pub fn approved_count(&self) -> usize {
        self.hunks
            .iter()
            .filter(|hunk| hunk.decision == HunkDecision::Approved)
            .count()
    }

    /// How many blocks are rejected.
    pub fn rejected_count(&self) -> usize {
        self.hunks
            .iter()
            .filter(|hunk| hunk.decision == HunkDecision::Rejected)
            .count()
    }

    /// Whether every block has a decision.
    pub fn is_settled(&self) -> bool {
        self.hunks
            .iter()
            .all(|hunk| hunk.decision != HunkDecision::Pending)
    }
}

/// One entry of the session trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceEntry {
    /// What kind of thing happened.
    pub kind: TraceKind,
    /// What happened, in words.
    pub detail: String,
}

/// The kinds of thing the trace records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKind {
    /// A turn started.
    Turn,
    /// The agent called a tool.
    Tool,
    /// The agent proposed a change.
    Change,
    /// The user approved or rejected something.
    Approval,
    /// A change was written into the buffer.
    Applied,
    /// A check ran.
    Check,
}

impl TraceKind {
    /// The word the panel shows for this kind.
    pub fn label(&self) -> &'static str {
        match self {
            TraceKind::Turn => "执行",
            TraceKind::Tool => "工具",
            TraceKind::Change => "变更",
            TraceKind::Approval => "审批",
            TraceKind::Applied => "应用",
            TraceKind::Check => "检查",
        }
    }
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
    /// The context the user chose to send.
    pub context: Vec<ContextItem>,
    /// The changes waiting for the user's decision, in the order they came.
    pub pending_diffs: Vec<DiffProposal>,
    /// The unified diffs the agent sent, by path.
    ///
    /// The patch is kept as it arrived: it is what the blocks were read from,
    /// and it is what has to be applied to the buffer.
    pub patches: BTreeMap<String, String>,
    /// What happened, in the order it happened.
    pub trace: Vec<TraceEntry>,
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
                kind,
                content,
            } => {
                match self
                    .tools
                    .iter_mut()
                    .find(|tool| tool.tool_call_id == tool_call_id)
                {
                    Some(existing) => {
                        existing.status = status;
                        if !title.is_empty() {
                            existing.title = title.clone();
                        }
                        if !kind.is_empty() {
                            existing.kind = kind.clone();
                        }
                    }
                    None => self.tools.push(ToolEntry {
                        tool_call_id: tool_call_id.clone(),
                        title: title.clone(),
                        status,
                        kind: kind.clone(),
                    }),
                }
                self.trace.push(TraceEntry {
                    kind: TraceKind::Tool,
                    detail: title.clone(),
                });
                // A change the agent proposes comes with the tool call that
                // proposes it.
                for item in content {
                    if let ToolCallContent::Diff { path, patch } = item {
                        self.offer_diff(path, &patch);
                    }
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
    ///
    /// The trace is not cleared: what happened earlier in the session is still
    /// what happened, and a trace that forgets is not a trace.
    pub fn begin_turn(&mut self) {
        self.transcript.clear();
        self.tools.clear();
        self.pending_permission = None;
        self.last_stop_reason = None;
        self.error = None;
        self.running = true;
        self.trace.push(TraceEntry {
            kind: TraceKind::Turn,
            detail: "开始新的一轮".to_owned(),
        });
    }

    /// The answer to the pending permission request, if there is one.
    pub fn pending_permission_answer(&self, option_id: &str) -> Option<PermissionOutcome> {
        self.pending_permission
            .as_ref()
            .map(|_| PermissionOutcome::selected(option_id))
    }

    /// Sets the context the user may choose from.
    ///
    /// The selection of the items that are already there is kept, so the
    /// panel does not forget what the user picked when the file list
    /// changes.
    pub fn set_context(&mut self, items: Vec<ContextItem>) {
        let previous: BTreeMap<String, bool> = self
            .context
            .iter()
            .map(|item| (item.id.clone(), item.selected))
            .collect();
        self.context = items
            .into_iter()
            .map(|mut item| {
                if let Some(selected) = previous.get(&item.id) {
                    item.selected = *selected;
                }
                item
            })
            .collect();
    }

    /// Chooses or unchooses a context item.
    pub fn toggle_context(&mut self, id: &str) {
        if let Some(item) = self.context.iter_mut().find(|item| item.id == id) {
            item.selected = !item.selected;
        }
    }

    /// The context the user chose to send.
    pub fn selected_context(&self) -> Vec<&ContextItem> {
        self.context
            .iter()
            .filter(|item| item.selected)
            .collect()
    }

    /// Records a proposed change and waits for the user's decision.
    pub fn offer_diff(&mut self, path: String, patch: &str) {
        let hunks = parse_hunks(patch);
        if hunks.is_empty() {
            return;
        }
        self.trace.push(TraceEntry {
            kind: TraceKind::Change,
            detail: format!("{path} · {} 个变更块", hunks.len()),
        });
        self.patches.insert(path.clone(), patch.to_owned());
        self.pending_diffs.push(DiffProposal {
            path,
            hunks,
            base_version: None,
            base_text: None,
        });
    }

    /// The change waiting for a decision about the given path.
    ///
    /// A path that has no change waiting returns nothing: the user is never
    /// asked about a file the agent did not touch.
    pub fn diff_for(&self, path: &str) -> Option<&DiffProposal> {
        self.pending_diffs
            .iter()
            .find(|proposal| proposal.path == path)
    }

    /// The change waiting for a decision about the given path, for editing.
    pub fn diff_for_mut(&mut self, path: &str) -> Option<&mut DiffProposal> {
        self.pending_diffs
            .iter_mut()
            .find(|proposal| proposal.path == path)
    }

    /// Records the buffer version the proposal was computed against.
    pub fn set_diff_base(&mut self, path: &str, version: u64, text: String) {
        if let Some(proposal) = self.diff_for_mut(path) {
            proposal.base_version = Some(version);
            proposal.base_text = Some(text);
        }
    }

    /// Decides one block of the proposed change.
    pub fn decide_hunk(&mut self, path: &str, index: usize, decision: HunkDecision) {
        let Some(proposal) = self.diff_for_mut(path) else {
            return;
        };
        let Some(hunk) = proposal.hunks.get_mut(index) else {
            return;
        };
        hunk.decision = decision;
        self.trace.push(TraceEntry {
            kind: TraceKind::Approval,
            detail: format!("{} · 第 {} 块", path, index + 1),
        });
    }

    /// Decides every block of the proposed change.
    pub fn decide_all_hunks(&mut self, path: &str, decision: HunkDecision) {
        let Some(proposal) = self.diff_for_mut(path) else {
            return;
        };
        for hunk in &mut proposal.hunks {
            hunk.decision = decision;
        }
        let count = proposal.hunks.len();
        self.trace.push(TraceEntry {
            kind: TraceKind::Approval,
            detail: format!("{path} · 全部 {count} 块"),
        });
    }

    /// The blocks the user approved for the given path.
    pub fn approved_hunks(&self, path: &str) -> BTreeSet<usize> {
        self.diff_for(path)
            .map(|proposal| {
                proposal
                    .hunks
                    .iter()
                    .enumerate()
                    .filter(|(_, hunk)| hunk.decision == HunkDecision::Approved)
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Forgets the change about the given path.
    pub fn take_diff(&mut self, path: &str) -> Option<DiffProposal> {
        let index = self.pending_diffs.iter().position(|proposal| proposal.path == path)?;
        Some(self.pending_diffs.remove(index))
    }

    /// Records that a change was written into the buffer.
    pub fn record_applied(&mut self, detail: String) {
        self.trace.push(TraceEntry {
            kind: TraceKind::Applied,
            detail,
        });
    }

    /// Records that a check ran.
    pub fn record_check(&mut self, detail: String) {
        self.trace.push(TraceEntry {
            kind: TraceKind::Check,
            detail,
        });
    }
}

/// Reads the blocks of a unified diff.
///
/// The patch is parsed here rather than by the diff crate, because the panel
/// shows what the agent proposes without knowing what the buffer holds: the
/// buffer is only checked when the change is applied.
fn parse_hunks(patch: &str) -> Vec<HunkEntry> {
    let mut hunks: Vec<HunkEntry> = Vec::new();
    let mut current: Option<HunkEntry> = None;

    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            let line_number = header
                .split_whitespace()
                .next()
                .and_then(|range| range.strip_prefix('-'))
                .and_then(|range| range.split(',').next())
                .and_then(|start| start.parse::<usize>().ok())
                .unwrap_or(1);
            current = Some(HunkEntry {
                line: line_number,
                added: Vec::new(),
                removed: Vec::new(),
                decision: HunkDecision::Pending,
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix('+') {
            hunk.added.push(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix('-') {
            hunk.removed.push(rest.to_owned());
        }
    }
    if let Some(hunk) = current.take() {
        hunks.push(hunk);
    }

    hunks
        .into_iter()
        .filter(|hunk| !hunk.added.is_empty() || !hunk.removed.is_empty())
        .collect()
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
            kind: "read".to_owned(),
            content: Vec::new(),
        });
        panel.apply(SessionEvent::ToolCall {
            tool_call_id: "call-1".to_owned(),
            title: String::new(),
            status: ToolStatus::Completed,
            kind: String::new(),
            content: Vec::new(),
        });

        assert_eq!(panel.transcript, "hello world");
        assert_eq!(panel.tools.len(), 1);
        assert_eq!(panel.tools[0].status, ToolStatus::Completed);
        assert_eq!(panel.tools[0].title, "read_file");
        assert_eq!(panel.tools[0].kind, "read");
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

    #[test]
    fn a_proposed_change_is_split_into_blocks() {
        let mut panel = AgentPanel::new();
        panel.begin_turn();
        panel.apply(SessionEvent::ToolCall {
            tool_call_id: "call-2".to_owned(),
            title: "request_diff".to_owned(),
            status: ToolStatus::InProgress,
            kind: "edit".to_owned(),
            content: vec![ToolCallContent::Diff {
                path: "src/app.rs".to_owned(),
                patch: "@@ -10,2 +10,3 @@\n keep\n-old\n+new\n+extra\n@@ -40,1 +41,1 @@\n-x\n+y\n"
                    .to_owned(),
            }],
        });

        let proposal = panel.diff_for("src/app.rs").expect("the change is offered");
        assert_eq!(proposal.path, "src/app.rs");
        assert_eq!(proposal.hunks.len(), 2);
        assert_eq!(proposal.hunks[0].line, 10);
        assert_eq!(proposal.hunks[0].added, vec!["new".to_owned(), "extra".to_owned()]);
        assert_eq!(proposal.hunks[0].removed, vec!["old".to_owned()]);
        assert_eq!(proposal.hunks[1].line, 40);
    }

    #[test]
    fn a_file_the_agent_did_not_touch_has_no_change_waiting() {
        let mut panel = AgentPanel::new();
        panel.offer_diff("a.rs".to_owned(), "@@ -1,1 +1,2 @@\n a\n+b\n");
        assert!(panel.diff_for("b.rs").is_none());
    }

    #[test]
    fn two_files_keep_their_own_changes() {
        let mut panel = AgentPanel::new();
        panel.offer_diff("a.rs".to_owned(), "@@ -1,1 +1,2 @@\n a\n+b\n");
        panel.offer_diff("b.rs".to_owned(), "@@ -5,1 +5,2 @@\n c\n+d\n");
        panel.decide_hunk("a.rs", 0, HunkDecision::Approved);

        assert_eq!(panel.approved_hunks("a.rs"), BTreeSet::from([0usize]));
        assert!(panel.approved_hunks("b.rs").is_empty());
        assert_eq!(panel.pending_diffs.len(), 2);
    }

    #[test]
    fn blocks_are_approved_one_at_a_time() {
        let mut panel = AgentPanel::new();
        panel.offer_diff("f.txt".to_owned(), "@@ -1,1 +1,2 @@\n a\n+b\n@@ -5,1 +6,1 @@\n c\n+d\n");
        panel.decide_hunk("f.txt", 0, HunkDecision::Approved);
        panel.decide_hunk("f.txt", 1, HunkDecision::Rejected);

        let proposal = panel.diff_for("f.txt").expect("the change is offered");
        assert_eq!(proposal.approved_count(), 1);
        assert_eq!(proposal.rejected_count(), 1);
        assert!(proposal.is_settled());
        assert_eq!(panel.approved_hunks("f.txt"), BTreeSet::from([0usize]));
    }

    #[test]
    fn a_whole_file_can_be_approved_at_once() {
        let mut panel = AgentPanel::new();
        panel.offer_diff("f.txt".to_owned(), "@@ -1,1 +1,2 @@\n a\n+b\n@@ -5,1 +6,1 @@\n c\n+d\n");
        panel.decide_all_hunks("f.txt", HunkDecision::Approved);

        let proposal = panel.diff_for("f.txt").expect("the change is offered");
        assert_eq!(proposal.approved_count(), 2);
        assert_eq!(proposal.rejected_count(), 0);
    }

    #[test]
    fn a_whole_file_can_be_rejected_at_once() {
        let mut panel = AgentPanel::new();
        panel.offer_diff("f.txt".to_owned(), "@@ -1,1 +1,2 @@\n a\n+b\n");
        panel.decide_all_hunks("f.txt", HunkDecision::Rejected);

        let proposal = panel.diff_for("f.txt").expect("the change is offered");
        assert_eq!(proposal.approved_count(), 0);
        assert!(panel.approved_hunks("f.txt").is_empty());
    }

    #[test]
    fn the_context_keeps_what_the_user_chose() {
        let mut panel = AgentPanel::new();
        let item = ContextItem {
            id: "file:src/app.rs".to_owned(),
            source: ContextSource::CurrentFile,
            path: "src/app.rs".to_owned(),
            text: "fn main() {}".to_owned(),
            selected: false,
        };
        panel.set_context(vec![item.clone()]);
        panel.toggle_context("file:src/app.rs");
        assert!(panel.selected_context().len() == 1);

        // The list changes, the choice does not.
        panel.set_context(vec![item]);
        assert!(panel.selected_context().len() == 1);
    }

    #[test]
    fn an_unchosen_context_item_is_not_sent() {
        let mut panel = AgentPanel::new();
        panel.set_context(vec![ContextItem {
            id: "a".to_owned(),
            source: ContextSource::CurrentFile,
            path: "a.rs".to_owned(),
            text: "text".to_owned(),
            selected: false,
        }]);
        assert!(panel.selected_context().is_empty());
    }

    #[test]
    fn the_trace_records_what_happened() {
        let mut panel = AgentPanel::new();
        panel.begin_turn();
        panel.offer_diff("f.txt".to_owned(), "@@ -1,1 +1,2 @@\n a\n+b\n");
        panel.decide_hunk("f.txt", 0, HunkDecision::Approved);
        panel.record_applied("写入 1 个变更块，版本 4 → 5".to_owned());

        let kinds: Vec<TraceKind> = panel.trace.iter().map(|entry| entry.kind).collect();
        assert!(kinds.contains(&TraceKind::Turn));
        assert!(kinds.contains(&TraceKind::Change));
        assert!(kinds.contains(&TraceKind::Approval));
        assert!(kinds.contains(&TraceKind::Applied));
    }

    #[test]
    fn a_new_turn_does_not_erase_the_trace() {
        let mut panel = AgentPanel::new();
        panel.begin_turn();
        panel.record_check("检查通过".to_owned());
        panel.begin_turn();
        assert!(panel.trace.iter().any(|entry| entry.kind == TraceKind::Check));
    }
}
