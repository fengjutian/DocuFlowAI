import type { JournalEntry, JournalEntryKind, JournalStore } from "../journal/journal.js";
import { detectGaps } from "../journal/journal.js";

/**
 * The audit timeline (design 15: "从审计事件还原完整执行过程").
 *
 * The trace answers "which records belong together"; the timeline answers "what happened, and in
 * what order". It is built from the journal and from nothing else, because the journal is the only
 * place the recorded history exists (D-069) — a timeline assembled from a second bookkeeping
 * structure would be a second opinion about what happened.
 *
 * Two orders live here and they are not the same:
 *
 * - Inside one execution the order is the journal position. That is the recorded order, and it is
 *   never second-guessed.
 * - Across executions the order is the recorded time. The journal position is allocated per
 *   execution, so there is no global number to compare; the timestamps are the only common ground.
 *
 * The merge therefore never reorders an execution's own events: it interleaves whole executions by
 * time. When two clocks disagree — the runtime's timestamps and the server's — the timestamps of
 * one execution can stop being monotonic, and that is reported (`timestampsMonotonic`) rather than
 * quietly smoothed away.
 */

export interface AuditTimelineItem {
  executionId: string;
  /** Journal position inside the execution. */
  sequence: number;
  kind: JournalEntryKind;
  category: string;
  recordedAt: string;
  /** The task this event belongs to, when the record names one. */
  taskId?: string;
  /** The execution that spawned this one, when it is a child. */
  parentExecutionId?: string;
  payload: Record<string, unknown>;
}

export interface AuditTimelineExecution {
  executionId: string;
  taskIds: string[];
  /** The execution that spawned this one, when the journal says so. */
  parentExecutionId?: string;
  /** The executions this one spawned. */
  childExecutionIds: string[];
  runtimeId?: string;
  eventCount: number;
  firstRecordedAt: string;
  lastRecordedAt: string;
  /** Journal positions missing from this execution's recorded range. */
  gaps: number[];
  /** The stages the recorded events reconstruct, in the order they happened. */
  phases: string[];
  /** False when the recorded timestamps disagree with the journal order (two different clocks). */
  timestampsMonotonic: boolean;
}

export interface AuditTimeline {
  items: AuditTimelineItem[];
  executions: AuditTimelineExecution[];
  taskIds: string[];
  /** How many events of each category the timeline holds. */
  categories: Record<string, number>;
  /** The executions whose journal has holes in it. */
  gaps: Array<{ executionId: string; sequences: number[] }>;
  /** True when a limit cut the item list short. */
  truncated: boolean;
}

/**
 * The stage a recorded event belongs to.
 *
 * The stages are the process a reader wants to follow — did it start, did it ask, did it act, did it
 * change anything, did it end — and they are derived, never invented: an event that is not in the
 * journal cannot put a stage on the timeline.
 */
const PHASE_BY_CATEGORY: Record<string, string> = {
  "execution.input": "started",
  "execution.started": "started",
  "turn.started": "turn",
  "permission.requested": "policy",
  "permission.resolved": "policy",
  "tool.started": "tool",
  "tool.completed": "tool",
  "tool.failed": "tool",
  "change.applied": "change",
  "evaluation.completed": "evaluation",
  "skill.delivered": "skill",
  "subagent.spawned": "subagent",
  "subagent.parent": "subagent",
  "recovery.compensated": "recovery",
  "recovery.rebuild": "recovery",
  "execution.completed": "completed",
  "execution.failed": "failed",
  "execution.cancelled": "cancelled"
};

/**
 * Merges per-execution streams into one time-ordered list.
 *
 * Each stream is already in journal order and stays that way: only the head of each stream is ever
 * compared, so an execution's own events cannot be reordered by a timestamp that disagrees.
 */
export function mergeAuditStreams(streams: readonly (readonly JournalEntry[])[]): JournalEntry[] {
  const cursors = streams.map(() => 0);
  const merged: JournalEntry[] = [];
  for (;;) {
    let chosen = -1;
    let chosenEntry: JournalEntry | undefined;
    for (let index = 0; index < streams.length; index += 1) {
      const entry = streams[index]?.[cursors[index] ?? 0];
      if (!entry) continue;
      if (!chosenEntry) {
        chosen = index;
        chosenEntry = entry;
        continue;
      }
      if (compareForTimeline(entry, chosenEntry) < 0) {
        chosen = index;
        chosenEntry = entry;
      }
    }
    if (!chosenEntry || chosen < 0) break;
    merged.push(chosenEntry);
    cursors[chosen] = (cursors[chosen] ?? 0) + 1;
  }
  return merged;
}

function compareForTimeline(left: JournalEntry, right: JournalEntry): number {
  const byTime = left.recordedAt.localeCompare(right.recordedAt);
  if (byTime !== 0) return byTime;
  const byExecution = left.executionId.localeCompare(right.executionId);
  if (byExecution !== 0) return byExecution;
  return left.sequence - right.sequence;
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value : undefined;
}

/** The task an event belongs to, from whichever record names it. */
function taskIdOf(entry: JournalEntry): string | undefined {
  if (entry.category === "execution.input") return text(entry.payload["taskId"]);
  return text(entry.payload["taskId"]);
}

/** The parent execution, from the child's own `subagent.parent` record. */
function parentOf(entry: JournalEntry): string | undefined {
  if (entry.category === "subagent.parent") return text(entry.payload["parentExecutionId"]);
  return undefined;
}

/** Builds the timeline out of the journal entries of every execution. */
export function buildAuditTimeline(entries: readonly JournalEntry[]): AuditTimeline {
  const byExecution = new Map<string, JournalEntry[]>();
  for (const entry of entries) {
    const bucket = byExecution.get(entry.executionId) ?? [];
    bucket.push(entry);
    byExecution.set(entry.executionId, bucket);
  }
  const streams = [...byExecution.values()].map(stream => [...stream].sort((a, b) => a.sequence - b.sequence));
  const merged = mergeAuditStreams(streams);

  const items: AuditTimelineItem[] = merged.map(entry => {
    const taskId = taskIdOf(entry);
    const parentExecutionId = parentOf(entry);
    return {
      executionId: entry.executionId,
      sequence: entry.sequence,
      kind: entry.kind,
      category: entry.category,
      recordedAt: entry.recordedAt,
      ...(taskId ? { taskId } : {}),
      ...(parentExecutionId ? { parentExecutionId } : {}),
      payload: entry.payload
    };
  });

  const categories: Record<string, number> = {};
  for (const item of items) {
    categories[item.category] = (categories[item.category] ?? 0) + 1;
  }

  const executions: AuditTimelineExecution[] = [];
  const taskIds = new Set<string>();
  const gaps: Array<{ executionId: string; sequences: number[] }> = [];

  for (const [executionId, stream] of byExecution) {
    const sorted = [...stream].sort((a, b) => a.sequence - b.sequence);
    const ownTaskIds = new Set<string>();
    const phases: string[] = [];
    let runtimeId: string | undefined;
    let parentExecutionId: string | undefined;
    const childExecutionIds: string[] = [];

    for (const entry of sorted) {
      const taskId = taskIdOf(entry);
      if (taskId) {
        ownTaskIds.add(taskId);
        taskIds.add(taskId);
      }
      if (entry.category === "execution.runtime") runtimeId = text(entry.payload["runtimeId"]) ?? runtimeId;
      const parent = parentOf(entry);
      if (parent) parentExecutionId = parent;
      if (entry.category === "subagent.spawned") {
        const child = text(entry.payload["childExecutionId"]);
        if (child && !childExecutionIds.includes(child)) childExecutionIds.push(child);
      }
      const phase = PHASE_BY_CATEGORY[entry.category];
      if (phase && !phases.includes(phase)) phases.push(phase);
    }

    const missing = detectGaps(sorted);
    if (missing.length > 0) gaps.push({ executionId, sequences: missing });

    executions.push({
      executionId,
      taskIds: [...ownTaskIds],
      ...(parentExecutionId ? { parentExecutionId } : {}),
      ...(childExecutionIds.length > 0 ? { childExecutionIds } : {}),
      ...(runtimeId ? { runtimeId } : {}),
      eventCount: sorted.length,
      firstRecordedAt: sorted[0]!.recordedAt,
      lastRecordedAt: sorted[sorted.length - 1]!.recordedAt,
      gaps: missing,
      phases,
      timestampsMonotonic: isMonotonic(sorted)
    });
  }

  executions.sort((left, right) => left.firstRecordedAt.localeCompare(right.firstRecordedAt));

  return { items, executions, taskIds: [...taskIds].sort(), categories, gaps, truncated: false };
}

/** Whether the recorded times agree with the recorded order inside one execution. */
function isMonotonic(entries: readonly JournalEntry[]): boolean {
  for (let index = 1; index < entries.length; index += 1) {
    if (entries[index]!.recordedAt < entries[index - 1]!.recordedAt) return false;
  }
  return true;
}

export interface AuditTimelineQuery {
  /** The task whose tree is wanted; a delegation tree is included with it. */
  taskId?: string;
  /** One execution; its parent and children are included with it. */
  executionId?: string;
  /** Only events of this category. */
  category?: string;
  /** Only events recorded at or after this time. */
  since?: string;
  /** The most items to return. */
  limit?: number;
}

/**
 * Reads the timeline out of the journal.
 *
 * The filters are correlation filters: asking for a task returns the whole tree that ran for it
 * (a child execution carries its own task id, so the tree is found through the spawn records), and
 * asking for an execution returns its parent and children. A timeline that showed one execution of a
 * delegation and hid the rest would not be the execution's process.
 */
export class AuditTimelineService {
  constructor(private readonly journal: JournalStore) {}

  async build(query: AuditTimelineQuery): Promise<AuditTimeline> {
    const all: JournalEntry[] = [];
    for (const executionId of await this.journal.listExecutions()) {
      all.push(...(await this.journal.read(executionId)));
    }
    const timeline = buildAuditTimeline(all);

    const wanted = new Set<string>();
    if (query.executionId) {
      wanted.add(query.executionId);
      for (const execution of timeline.executions) {
        if (execution.parentExecutionId === query.executionId) wanted.add(execution.executionId);
        if (execution.childExecutionIds.includes(query.executionId)) wanted.add(execution.executionId);
      }
    }
    if (query.taskId) {
      for (const execution of timeline.executions) {
        if (execution.taskIds.includes(query.taskId)) wanted.add(execution.executionId);
      }
      // A delegated child carries its own task id, so the tree is found through the spawn records:
      // the parent names the child, and the child names the parent.
      for (const execution of timeline.executions) {
        if (!wanted.has(execution.executionId)) continue;
        for (const child of execution.childExecutionIds) wanted.add(child);
        if (execution.parentExecutionId) wanted.add(execution.parentExecutionId);
      }
    }

    const scoped = wanted.size > 0;
    const executions = scoped
      ? timeline.executions.filter(execution => wanted.has(execution.executionId))
      : timeline.executions;
    const scopedIds = new Set(executions.map(execution => execution.executionId));

    let items = timeline.items.filter(item => scopedIds.has(item.executionId));
    if (query.category) items = items.filter(item => item.category === query.category);
    if (query.since) {
      const since = query.since;
      items = items.filter(item => item.recordedAt >= since);
    }

    let truncated = false;
    if (query.limit !== undefined && items.length > query.limit) {
      items = items.slice(0, query.limit);
      truncated = true;
    }

    const categories: Record<string, number> = {};
    for (const item of items) categories[item.category] = (categories[item.category] ?? 0) + 1;

    return {
      items,
      executions,
      taskIds: [...new Set(executions.flatMap(execution => execution.taskIds))].sort(),
      categories,
      gaps: timeline.gaps.filter(gap => scopedIds.has(gap.executionId)),
      truncated
    };
  }
}
