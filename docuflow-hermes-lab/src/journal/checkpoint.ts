import type { AgentCapability } from "../types/adapter.js";
import { detectGaps, sortEntries, type JournalEntry } from "./journal.js";

/** Fields a checkpoint must carry per design 14.3. */
export interface CheckpointContext {
  contextSnapshotId?: string;
  policySnapshotId?: string;
  gitBaseSha?: string;
  worktreeId?: string;
  externalSessionId?: string;
  /** Human-readable stage, for example `awaiting_approval` or `diff_applied`. */
  currentStep?: string;
}

export interface ExecutionCheckpoint extends CheckpointContext {
  executionId: string;
  lastSequence: number;
  eventCount: number;
  terminal: boolean;
  terminalEventType?: string;
  lastEventType?: string;
  appliedChangeRecordIds: string[];
  pendingPermissionRequestIds: string[];
  gaps: number[];
  createdAt: string;
}

const TERMINAL_CATEGORIES = new Set(["execution.completed", "execution.failed", "execution.cancelled"]);

export function buildCheckpoint(
  executionId: string,
  entries: readonly JournalEntry[],
  options: { context?: CheckpointContext; now?: () => Date } = {}
): ExecutionCheckpoint {
  const sorted = sortEntries(entries.filter(entry => entry.executionId === executionId));
  const terminalEntry = [...sorted].reverse().find(entry => TERMINAL_CATEGORIES.has(entry.category));
  const changeRecords = new Set<string>();
  const requested = new Set<string>();
  const resolved = new Set<string>();
  for (const entry of sorted) {
    if (entry.category === "change.applied") {
      const id = entry.payload["changeRecordId"];
      if (typeof id === "string" && id) changeRecords.add(id);
    }
    if (entry.category === "permission.requested") {
      const id = entry.payload["requestId"];
      if (typeof id === "string" && id) requested.add(id);
    }
    if (entry.category === "permission.resolved") {
      const id = entry.payload["requestId"];
      if (typeof id === "string" && id) resolved.add(id);
    }
  }
  const context = options.context ?? {};
  return {
    executionId,
    lastSequence: sorted.at(-1)?.sequence ?? 0,
    eventCount: sorted.length,
    terminal: terminalEntry !== undefined,
    ...(terminalEntry ? { terminalEventType: terminalEntry.category } : {}),
    ...(sorted.at(-1) ? { lastEventType: sorted.at(-1)!.category } : {}),
    appliedChangeRecordIds: [...changeRecords].sort(),
    pendingPermissionRequestIds: [...requested].filter(id => !resolved.has(id)).sort(),
    gaps: detectGaps(sorted),
    createdAt: (options.now ?? (() => new Date()))().toISOString(),
    ...cleanContext(context)
  };
}

export type RecoveryAction = "stop" | "compensate" | "resume" | "rebuild";

export interface RecoveryPlan {
  action: RecoveryAction;
  reason: string;
  /** First sequence the caller should fetch or generate from. */
  fromSequence: number;
  checkpoint: ExecutionCheckpoint;
}

/**
 * Design 14.2/14.3: recovery is a decision, not "call the model again".
 *
 * Terminal → nothing to do. Missing events → compensate before deciding anything else. The runtime
 * can only continue its own session when it advertises and confirms resumability (Hermes HTTP does
 * not), otherwise the execution is rebuilt with the checkpoint injected.
 */
export function planRecovery(
  checkpoint: ExecutionCheckpoint,
  options: {
    capabilities: Pick<AgentCapability, "sessionResume">;
    /** Whether this specific runtime session is still alive on the other side. */
    sessionResumable?: boolean;
  }
): RecoveryPlan {
  if (checkpoint.terminal) {
    return {
      action: "stop",
      reason: `Execution is already terminal (${checkpoint.terminalEventType ?? "unknown state"})`,
      fromSequence: checkpoint.lastSequence,
      checkpoint
    };
  }
  if (checkpoint.gaps.length > 0) {
    return {
      action: "compensate",
      reason: `Journal is missing sequence(s) ${checkpoint.gaps.join(", ")}`,
      fromSequence: checkpoint.gaps[0]!,
      checkpoint
    };
  }
  if (options.capabilities.sessionResume && options.sessionResumable === true) {
    return {
      action: "resume",
      reason: "Runtime session is resumable; continue from the next sequence",
      fromSequence: checkpoint.lastSequence + 1,
      checkpoint
    };
  }
  return {
    action: "rebuild",
    reason: options.capabilities.sessionResume
      ? "Runtime reports resume support but the session is gone; rebuild with the checkpoint"
      : "Runtime cannot resume a session; rebuild with the checkpoint",
    fromSequence: checkpoint.lastSequence + 1,
    checkpoint
  };
}

function cleanContext(context: CheckpointContext): Partial<CheckpointContext> {
  const entries = Object.entries(context).filter(([, value]) => value !== undefined && value !== "");
  return Object.fromEntries(entries) as Partial<CheckpointContext>;
}
