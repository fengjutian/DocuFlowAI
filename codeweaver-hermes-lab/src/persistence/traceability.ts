import type { ChangeRecord } from "../code-engine/change-record.js";
import type { EvaluationResult } from "../evaluation/evaluation.js";
import type { JournalEntry, JournalStore } from "../journal/journal.js";
import type {
  ContextSnapshotRecord,
  PolicySnapshotRecord,
  PromptSnapshotRecord
} from "./records.js";
import type { RecordStore } from "./record-store.js";

export interface TraceStores {
  changeRecords: RecordStore<ChangeRecord>;
  contextSnapshots: RecordStore<ContextSnapshotRecord>;
  promptSnapshots: RecordStore<PromptSnapshotRecord>;
  policySnapshots: RecordStore<PolicySnapshotRecord>;
  evaluations: RecordStore<EvaluationResult>;
}

/**
 * A permission request and the decision it was answered with.
 *
 * The request alone is half a fact. A reader who sees "the runtime asked" but not "CodeWeaver
 * answered" cannot tell an enforced denial from a request that was never settled.
 */
export interface TracedPermission {
  requestId: string;
  decision: "approve" | "deny";
  tool?: string | undefined;
  /** The tool call the decision was about, when the runtime provided the id. */
  toolCallId?: string | undefined;
  reason?: string | undefined;
}

/** One tool call, with the decision that gated it. */
export interface TracedToolCall {
  toolCallId: string;
  toolName: string;
  /** The journal sequence the call was made at, so the order is the recorded one. */
  sequence: number;
  status: "started" | "completed" | "failed";
  args?: Record<string, unknown> | undefined;
  /** The decision that let the call through, when the call was gated. */
  permission?: TracedPermission | undefined;
}

/**
 * The parts of the trace that live on the timeline rather than in a record store.
 *
 * Design 15 draws the chain as Task → Run → Tool → Change → Commit → Test. The change records
 * and the evaluations answer the last two; the task, the run and the tool calls are facts of the
 * execution, and they are read from the journal — the same append-only timeline the audit engine
 * writes.
 */
export interface TraceChain {
  taskIds: string[];
  runtimeId?: string | undefined;
  /** The runtime's own run id, when the runtime has one. */
  externalRunId?: string | undefined;
  toolCalls: TracedToolCall[];
  /** Every decision, including the ones no tool call could be paired with. */
  permissions: TracedPermission[];
  /** The commits the changes landed in. */
  gitCommits: string[];
}

/**
 * Answers the traceability questions from design 15:
 * commit → agent run, execution → context/policy/prompt/diff, task → all executions.
 */
export interface ExecutionTrace extends TraceChain {
  executionId: string;
  changeRecords: ChangeRecord[];
  contextSnapshots: ContextSnapshotRecord[];
  promptSnapshots: PromptSnapshotRecord[];
  policySnapshots: PolicySnapshotRecord[];
  evaluations: EvaluationResult[];
}

export interface CommitTrace {
  changeRecord: ChangeRecord;
  trace: ExecutionTrace;
}

/**
 * Reads the chain out of a journal.
 *
 * The journal is the only place the timeline exists, so the task, the runtime and the tool calls
 * come from there rather than from a second bookkeeping structure that could disagree with it.
 */
export function buildTraceChain(entries: readonly JournalEntry[]): TraceChain {
  const taskIds: string[] = [];
  const gitCommits: string[] = [];
  const calls = new Map<string, TracedToolCall>();
  const order: string[] = [];
  const permissions: TracedPermission[] = [];
  const requested = new Map<string, { tool?: string | undefined; toolCallId?: string | undefined }>();
  let runtimeId: string | undefined;
  let externalRunId: string | undefined;

  const sorted = [...entries].sort((left, right) => left.sequence - right.sequence);
  for (const entry of sorted) {
    const payload = entry.payload;
    if (entry.kind === "audit") {
      if (entry.category === "execution.input") {
        const taskId = text(payload["taskId"]);
        if (taskId && !taskIds.includes(taskId)) taskIds.push(taskId);
      }
      if (entry.category === "execution.runtime") {
        runtimeId = text(payload["runtimeId"]) ?? runtimeId;
      }
      if (entry.category === "change.applied") {
        const sha = text(payload["gitCommitSha"]);
        if (sha && !gitCommits.includes(sha)) gitCommits.push(sha);
      }
      continue;
    }

    switch (entry.category) {
      case "execution.started": {
        externalRunId = text(payload["externalRunId"]) ?? externalRunId;
        break;
      }
      case "tool.started": {
        const id = text(payload["toolCallId"]) ?? `seq-${entry.sequence}`;
        if (!calls.has(id)) order.push(id);
        calls.set(id, {
          toolCallId: id,
          toolName: text(payload["toolName"]) ?? "unknown",
          sequence: entry.sequence,
          status: "started",
          ...(isRecord(payload["args"]) ? { args: payload["args"] } : {})
        });
        break;
      }
      case "tool.completed":
      case "tool.failed": {
        const id = text(payload["toolCallId"]);
        const call = id ? calls.get(id) : undefined;
        if (call) call.status = entry.category === "tool.failed" ? "failed" : "completed";
        break;
      }
      case "permission.requested": {
        const requestId = text(payload["requestId"]);
        if (requestId) {
          requested.set(requestId, {
            ...(text(payload["tool"]) ? { tool: text(payload["tool"]) } : {}),
            ...(text(payload["toolCallId"]) ? { toolCallId: text(payload["toolCallId"]) } : {})
          });
        }
        break;
      }
      case "permission.resolved": {
        const requestId = text(payload["requestId"]);
        if (!requestId) break;
        const asked = requested.get(requestId);
        const decision = text(payload["decision"]);
        permissions.push({
          requestId,
          decision: decision === "approve" ? "approve" : "deny",
          ...(text(payload["tool"]) ?? asked?.tool
            ? { tool: text(payload["tool"]) ?? asked?.tool }
            : {}),
          ...(text(payload["toolCallId"]) ?? asked?.toolCallId
            ? { toolCallId: text(payload["toolCallId"]) ?? asked?.toolCallId }
            : {}),
          ...(text(payload["reason"]) ? { reason: text(payload["reason"]) } : {})
        });
        break;
      }
      default:
        break;
    }
  }

  const toolCalls = order.map(id => calls.get(id)).filter((call): call is TracedToolCall => call !== undefined);
  attachPermissions(toolCalls, permissions);
  return {
    taskIds,
    ...(runtimeId ? { runtimeId } : {}),
    ...(externalRunId ? { externalRunId } : {}),
    toolCalls,
    permissions,
    gitCommits
  };
}

/**
 * Pairs each decision with the tool call it was about.
 *
 * The decision is recorded before the call runs, so the order in the journal cannot pair them. The
 * tool call id is the exact link when the runtime provides one; without it the decision goes to the
 * earliest still-undecided call of the same tool, which is the only pairing the record supports.
 * A decision that matches nothing stays in the permission list instead of being attached to
 * something it may not be about.
 */
function attachPermissions(toolCalls: TracedToolCall[], permissions: TracedPermission[]): void {
  const taken = new Set<string>();
  for (const permission of permissions) {
    if (permission.toolCallId) {
      const call = toolCalls.find(item => item.toolCallId === permission.toolCallId);
      if (call && !taken.has(call.toolCallId)) {
        call.permission = permission;
        taken.add(call.toolCallId);
        continue;
      }
    }
    if (!permission.tool) continue;
    const call = toolCalls.find(
      item => item.toolName === permission.tool && !taken.has(item.toolCallId)
    );
    if (call) {
      call.permission = permission;
      taken.add(call.toolCallId);
    }
  }
}

export class TraceabilityQuery {
  constructor(
    private readonly stores: TraceStores,
    private readonly journal: JournalStore
  ) {}

  async byExecution(executionId: string): Promise<ExecutionTrace> {
    const [changes, prompts, policies, evaluations] = await Promise.all([
      this.stores.changeRecords.list(),
      this.stores.promptSnapshots.list(),
      this.stores.policySnapshots.list(),
      this.stores.evaluations.list()
    ]);
    const executionPrompts = prompts.filter(record => record.executionId === executionId);
    const chain = buildTraceChain(await this.journal.read(executionId));
    return {
      executionId,
      ...chain,
      changeRecords: changes.filter(record => record.executionId === executionId),
      contextSnapshots: await this.resolveContexts(executionPrompts),
      promptSnapshots: executionPrompts,
      policySnapshots: policies.filter(record => record.executionId === executionId),
      evaluations: evaluations.filter(record => record.executionId === executionId)
    };
  }

  /** Design 15: "从 Git Commit 反查对应 Agent Run". */
  async byCommit(commitSha: string): Promise<CommitTrace | undefined> {
    const wanted = commitSha.toLowerCase();
    const changes = await this.stores.changeRecords.list();
    const changeRecord = changes.find(record => record.gitCommitSha?.toLowerCase() === wanted);
    if (!changeRecord) return undefined;
    return { changeRecord, trace: await this.byExecution(changeRecord.executionId) };
  }

  async byTask(taskId: string): Promise<ExecutionTrace[]> {
    const [changes, prompts, policies] = await Promise.all([
      this.stores.changeRecords.list(),
      this.stores.promptSnapshots.list(),
      this.stores.policySnapshots.list()
    ]);
    const executionIds = new Set<string>([
      ...changes.filter(record => record.taskId === taskId).map(record => record.executionId),
      ...prompts.filter(record => record.taskId === taskId).map(record => record.executionId),
      ...policies.filter(record => record.taskId === taskId).map(record => record.executionId)
    ]);
    // The journal knows which executions a task produced even when no record store links them
    // yet — a task whose run crashed before its first snapshot is still a task's execution.
    for (const executionId of await this.journal.listExecutions()) {
      const chain = buildTraceChain(await this.journal.read(executionId));
      if (chain.taskIds.includes(taskId)) executionIds.add(executionId);
    }
    return Promise.all([...executionIds].sort().map(executionId => this.byExecution(executionId)));
  }

  /**
   * Context snapshots have no execution id in design 11.7; the link runs through the prompt
   * snapshot that consumed them.
   */
  private async resolveContexts(prompts: readonly PromptSnapshotRecord[]): Promise<ContextSnapshotRecord[]> {
    const ids = [...new Set(prompts.map(prompt => prompt.contextSnapshotId))];
    const contexts: ContextSnapshotRecord[] = [];
    for (const id of ids) {
      const record = await this.stores.contextSnapshots.get(id);
      if (record) contexts.push(record);
    }
    return contexts;
  }
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
