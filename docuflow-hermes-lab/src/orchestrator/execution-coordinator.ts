import { randomUUID } from "node:crypto";
import { AgentAdapterError, type AgentExecution, type AgentRuntimeAdapter, type AgentRuntimeEvent, type AgentTaskInput } from "../types/adapter.js";
import {
  buildCheckpoint,
  planRecovery,
  type CheckpointContext,
  type ExecutionCheckpoint,
  type RecoveryPlan
} from "../journal/checkpoint.js";
import { journaledEventSequences, recordAudit, recordRuntimeEvent } from "../journal/event-journal.js";
import type { JournalStore } from "../journal/journal.js";

/**
 * Wires the adapter to the journal and turns a recovery plan into an action.
 *
 * The adapter owns protocol translation; the journal owns durable history; this coordinator owns the
 * order between them. Every runtime event is journaled through one serialized chain per execution,
 * so journal order is a real order and not a race between handlers.
 */
export interface ExecutionCoordinatorOptions {
  adapter: AgentRuntimeAdapter;
  journal: JournalStore;
  now?: () => Date;
  onEvent?: (event: AgentRuntimeEvent) => void;
  /** Fetches runtime events the journal never received; required for `compensate`. */
  compensationSource?: (checkpoint: ExecutionCheckpoint) => Promise<AgentRuntimeEvent[]>;
  /** Whether the runtime-side session still exists. Defaults to false: assume it is gone. */
  isSessionResumable?: (executionId: string) => boolean | Promise<boolean>;
  /** Rebuild input when the original start call is not in memory (process restart). */
  recoveryInput?: (checkpoint: ExecutionCheckpoint) => AgentTaskInput | Promise<AgentTaskInput>;
}

interface Session {
  execution: AgentExecution;
  input: AgentTaskInput;
  chain: Promise<void>;
  nextJournalSequence: number;
  errors: Error[];
  unsubscribe?: () => Promise<void>;
}

export interface RecoverResult {
  plan: RecoveryPlan;
  execution?: AgentExecution;
  compensated?: number;
}

export class ExecutionCoordinator {
  private readonly sessions = new Map<string, Session>();
  private readonly now: () => Date;

  constructor(private readonly options: ExecutionCoordinatorOptions) {
    this.now = options.now ?? (() => new Date());
  }

  async start(input: AgentTaskInput): Promise<AgentExecution> {
    const execution = await this.options.adapter.start(input);
    const session: Session = {
      execution,
      input,
      chain: Promise.resolve(),
      nextJournalSequence: 1,
      errors: []
    };
    this.sessions.set(execution.executionId, session);

    session.unsubscribe = await this.options.adapter.subscribe(execution.executionId, event => {
      this.enqueue(session, async () => {
        await recordRuntimeEvent(this.options.journal, event, session.nextJournalSequence);
        session.nextJournalSequence += 1;
      });
      this.options.onEvent?.(event);
    });
    return execution;
  }

  async getCheckpoint(executionId: string, context?: CheckpointContext): Promise<ExecutionCheckpoint> {
    await this.flush(executionId);
    const session = this.sessions.get(executionId);
    const entries = await this.options.journal.read(executionId);
    const inferred = session ? contextFromSession(session) : {};
    return buildCheckpoint(executionId, entries, {
      context: { ...inferred, ...context },
      now: this.now
    });
  }

  async recover(executionId: string, context?: CheckpointContext): Promise<RecoverResult> {
    const checkpoint = await this.getCheckpoint(executionId, context);
    const capabilities = await this.options.adapter.getCapabilities();
    const sessionResumable = this.options.isSessionResumable
      ? await this.options.isSessionResumable(executionId)
      : false;
    const plan = planRecovery(checkpoint, { capabilities, sessionResumable });

    if (plan.action === "stop") return { plan };

    if (plan.action === "compensate") {
      if (!this.options.compensationSource) return { plan };
      const compensated = await this.compensate(checkpoint);
      await this.recordAuditFact(executionId, "recovery.compensated", { compensated });
      return { plan, compensated };
    }

    if (plan.action === "resume") {
      const execution = await this.options.adapter.resume(executionId);
      return { plan, execution };
    }

    const execution = await this.rebuild(checkpoint);
    return { plan, execution };
  }

  async cancel(executionId: string): Promise<void> {
    await this.recordAuditFact(executionId, "execution.cancel_requested", {});
    await this.options.adapter.cancel(executionId);
  }

  async terminate(executionId: string): Promise<void> {
    await this.recordAuditFact(executionId, "execution.terminate_requested", {});
    const session = this.sessions.get(executionId);
    await this.options.adapter.terminate(executionId);
    if (session?.unsubscribe) await session.unsubscribe().catch(() => undefined);
    this.sessions.delete(executionId);
  }

  /** Journaled DocuFlow facts (change applied, policy denied) join the same timeline. */
  async recordAuditFact(executionId: string, category: string, payload: Record<string, unknown>): Promise<void> {
    const session = this.sessions.get(executionId);
    if (session) {
      this.enqueue(session, async () => {
        await recordAudit(this.options.journal, {
          executionId,
          sequence: session.nextJournalSequence,
          category,
          payload
        });
        session.nextJournalSequence += 1;
      });
      await session.chain;
      return;
    }
    const entries = await this.options.journal.read(executionId);
    await recordAudit(this.options.journal, {
      executionId,
      sequence: (entries.at(-1)?.sequence ?? 0) + 1,
      category,
      payload,
      recordedAt: this.now().toISOString()
    });
  }

  /** Waits until every journal write queued for this execution has been attempted. */
  async flush(executionId: string): Promise<void> {
    await this.sessions.get(executionId)?.chain;
  }

  /** Journal write failures are collected instead of being swallowed by a fire-and-forget handler. */
  journalErrors(executionId: string): Error[] {
    return [...(this.sessions.get(executionId)?.errors ?? [])];
  }

  async stopAll(): Promise<void> {
    for (const [executionId, session] of this.sessions) {
      await session.unsubscribe?.().catch(() => undefined);
      await session.chain.catch(() => undefined);
      this.sessions.delete(executionId);
    }
  }

  private async compensate(checkpoint: ExecutionCheckpoint): Promise<number> {
    const source = this.options.compensationSource;
    if (!source) return 0;
    const events = await source(checkpoint);
    const session = this.sessions.get(checkpoint.executionId);
    const entries = await this.options.journal.read(checkpoint.executionId);
    const seen = journaledEventSequences(entries);
    let sequence = session?.nextJournalSequence ?? (entries.at(-1)?.sequence ?? 0) + 1;
    let compensated = 0;
    for (const event of [...events].sort((a, b) => a.sequence - b.sequence)) {
      if (seen.has(event.sequence)) continue;
      await recordRuntimeEvent(this.options.journal, event, sequence);
      sequence += 1;
      compensated += 1;
    }
    if (session) session.nextJournalSequence = sequence;
    return compensated;
  }

  private async rebuild(checkpoint: ExecutionCheckpoint): Promise<AgentExecution> {
    const session = this.sessions.get(checkpoint.executionId);
    const original = session?.input ?? (this.options.recoveryInput
      ? await this.options.recoveryInput(checkpoint)
      : undefined);
    if (!original) {
      throw new AgentAdapterError(
        "invalid_state",
        `Cannot rebuild execution ${checkpoint.executionId}: original input is unavailable`
      );
    }
    const input: AgentTaskInput = {
      ...original,
      runId: `run_${randomUUID()}`,
      prompt: `${original.prompt}\n\n${renderCheckpoint(checkpoint)}`
    };
    const execution = await this.start(input);
    await this.recordAuditFact(execution.executionId, "recovery.rebuild", {
      previousExecutionId: checkpoint.executionId,
      previousLastSequence: checkpoint.lastSequence,
      appliedChangeRecordIds: checkpoint.appliedChangeRecordIds
    });
    return execution;
  }

  private enqueue(session: Session, work: () => Promise<void>): void {
    session.chain = session.chain.then(work).catch(error => {
      session.errors.push(error instanceof Error ? error : new Error(String(error)));
    });
  }
}

function contextFromSession(session: Session): CheckpointContext {
  return {
    contextSnapshotId: session.input.contextSnapshotId,
    policySnapshotId: session.input.policySnapshotId,
    worktreeId: session.input.worktreeId,
    ...(session.execution.externalRunId ? { externalSessionId: session.execution.externalRunId } : {})
  };
}

/** The injected block is explicit about what must not be redone (design 14.3). */
export function renderCheckpoint(checkpoint: ExecutionCheckpoint): string {
  const summary = {
    previousExecutionId: checkpoint.executionId,
    lastSequence: checkpoint.lastSequence,
    lastEventType: checkpoint.lastEventType ?? null,
    currentStep: checkpoint.currentStep ?? null,
    gitBaseSha: checkpoint.gitBaseSha ?? null,
    appliedChangeRecordIds: checkpoint.appliedChangeRecordIds,
    pendingPermissionRequestIds: checkpoint.pendingPermissionRequestIds
  };
  return [
    "[DOCUFLOW CHECKPOINT]",
    JSON.stringify(summary, null, 2),
    "Do not re-apply any change record listed above; inspect the worktree before proposing new edits."
  ].join("\n");
}
