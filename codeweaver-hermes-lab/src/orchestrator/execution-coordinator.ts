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
import type { JournalEntry, JournalStore } from "../journal/journal.js";

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
  /**
   * Called for every runtime event.
   *
   * The runtime event is what the usage accounting reads; the journal entry comes through
   * `onRecorded` instead, because the live stream has to carry what was recorded.
   */
  onEvent?: (event: AgentRuntimeEvent) => void;
  /**
   * Called for every journal entry — runtime events and CodeWeaver's own facts alike — after it was
   * written.
   *
   * The journal position is the only sequence the replay and the live stream can both speak, so the
   * live stream carries the record rather than the raw runtime event. A client that reconnects with
   * `since` then has one number to compare, and the two sides cannot disagree about what it means.
   *
   * A promise is awaited: a caller that has to know the record is complete before it answers (the
   * usage ledger writes its record from the terminal entry) gets the answer it asked for instead of
   * a race with a write that is still in flight.
   */
  onRecorded?: (entry: JournalEntry) => void | Promise<void>;
  /** Fetches runtime events the journal never received; required for `compensate`. */
  compensationSource?: (checkpoint: ExecutionCheckpoint) => Promise<AgentRuntimeEvent[]>;
  /** Whether the runtime-side session still exists. Defaults to false: assume it is gone. */
  isSessionResumable?: (executionId: string) => boolean | Promise<boolean>;
  /**
   * The adapter that owns an execution whose session is gone.
   *
   * After a restart the coordinator has no session, and the default adapter is not necessarily the
   * one that ran the task. Rebuilding on the default runtime would run the task somewhere the
   * caller never chose, which is the one thing a named runtime must never do (D-047).
   */
  recoveringAdapter?: (executionId: string) => AgentRuntimeAdapter | undefined | Promise<AgentRuntimeAdapter | undefined>;
  /**
   * The registered id of the runtime behind an adapter, for example `pi-local`.
   *
   * It is journaled with the execution so that a reader can still name the runtime after a
   * restart; without it the console would have to guess from the server's default.
   */
  runtimeIdOf?: (adapter: AgentRuntimeAdapter) => string | undefined;
  /**
   * Rebuild input when the original start call is not in memory (process restart).
   *
   * It may be unavailable: an execution that never recorded its request cannot be rebuilt, and the
   * caller is told that instead of being handed a task that is not the one it asked for.
   */
  recoveryInput?: (
    checkpoint: ExecutionCheckpoint
  ) => AgentTaskInput | undefined | Promise<AgentTaskInput | undefined>;
}

interface Session {
  execution: AgentExecution;
  input: AgentTaskInput;
  /** The adapter that owns this execution. Different executions may run on different runtimes. */
  adapter: AgentRuntimeAdapter;
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

  /**
   * Starts an execution.
   *
   * `adapter` overrides the default runtime for this one execution, which is how a multi-runtime
   * setup routes work: the caller picks the runtime, the coordinator remembers which adapter owns
   * the execution so later status, approval and cancellation calls reach the right one.
   */
  async start(input: AgentTaskInput, adapter?: AgentRuntimeAdapter): Promise<AgentExecution> {
    const selected = adapter ?? this.options.adapter;
    const execution = await selected.start(input);
    const session: Session = {
      execution,
      input,
      adapter: selected,
      chain: Promise.resolve(),
      nextJournalSequence: 1,
      errors: []
    };
    this.sessions.set(execution.executionId, session);

    // Two facts that outlive this process: which runtime ran the task, and what it was asked.
    // A worker that dies takes its memory with it, and a rebuild needs both: the runtime to
    // rebuild on, and the request to rebuild from.
    const runtimeId = this.options.runtimeIdOf?.(selected);
    if (runtimeId) {
      await this.recordAuditFact(execution.executionId, "execution.runtime", { runtimeId });
    }
    await this.recordAuditFact(execution.executionId, "execution.input", {
      projectId: input.projectId,
      ...(input.featureId ? { featureId: input.featureId } : {}),
      taskId: input.taskId,
      prompt: input.prompt,
      contextSnapshotId: input.contextSnapshotId,
      policySnapshotId: input.policySnapshotId,
      worktreeId: input.worktreeId,
      workingDirectory: input.workingDirectory,
      allowedTools: input.allowedTools,
      timeoutSeconds: input.timeoutSeconds,
      correlationId: input.correlationId
    });

    session.unsubscribe = await selected.subscribe(execution.executionId, event => {
      this.enqueue(session, async () => {
        const entry = await recordRuntimeEvent(
          this.options.journal,
          event,
          session.nextJournalSequence,
          this.now().toISOString()
        );
        session.nextJournalSequence += 1;
        // Published after the write, not before it. An event that is announced before it is recorded
        // can be lost by a client that reconnects in between: it is in neither the replay nor the
        // live stream. The journal is the source of truth, so the live stream carries the record.
        this.options.onEvent?.(event);
        await this.options.onRecorded?.(entry);
      });
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

    // A terminal execution has nothing to resume and nothing to rebuild.
    //
    // The runtime is asked for its capabilities only when that can still change the decision, and
    // the order matters exactly when the runtime is what broke. Asking a dead runtime first turns
    // "this execution is over" into "the runtime is unreachable" — and the answer the caller
    // needs is the first one: the record is kept and no new run is started.
    if (checkpoint.terminal) {
      return { plan: planRecovery(checkpoint, { capabilities: { sessionResume: false } }) };
    }

    // Recovery must ask the runtime that owns the execution: a resume only works on the runtime
    // that still holds the session, and capabilities differ between runtimes.
    const adapter = await this.owningAdapter(executionId);
    const capabilities = await adapter.getCapabilities();
    // The runtime that owns the execution is the one that can say whether its session survived.
    // An injected answer wins (tests drive it), otherwise the adapter is asked; an adapter that
    // cannot answer has no session to offer.
    const sessionResumable = this.options.isSessionResumable
      ? await this.options.isSessionResumable(executionId)
      : (adapter.isSessionResumable?.(executionId) ?? false);
    const plan = planRecovery(checkpoint, { capabilities, sessionResumable });

    if (plan.action === "stop") return { plan };

    if (plan.action === "compensate") {
      if (!this.options.compensationSource) return { plan };
      const compensated = await this.compensate(checkpoint);
      await this.recordAuditFact(executionId, "recovery.compensated", { compensated });
      return { plan, compensated };
    }

    if (plan.action === "resume") {
      const execution = await adapter.resume(executionId);
      return { plan, execution };
    }

    const execution = await this.rebuild(checkpoint);
    return { plan, execution };
  }

  async cancel(executionId: string): Promise<void> {
    await this.recordAuditFact(executionId, "execution.cancel_requested", {});
    await this.adapterFor(executionId).cancel(executionId);
  }

  async terminate(executionId: string): Promise<void> {
    await this.recordAuditFact(executionId, "execution.terminate_requested", {});
    const session = this.sessions.get(executionId);
    await this.adapterFor(executionId).terminate(executionId);
    if (session?.unsubscribe) await session.unsubscribe().catch(() => undefined);
    this.sessions.delete(executionId);
  }

  /**
   * Continues a finished turn on the session the execution already used.
   *
   * The runtime keeps the transcript, so the new answer can depend on the earlier one — that is
   * what makes it a second turn instead of a second execution. A runtime that cannot continue
   * says so: starting a fresh run and calling it a continuation would look right and carry none
   * of the context.
   */
  async followUp(executionId: string, prompt: string): Promise<AgentExecution> {
    const session = this.sessions.get(executionId);
    if (!session) {
      throw new AgentAdapterError(
        "execution_not_found",
        `Execution ${executionId} is not live in this process, so it cannot be continued`
      );
    }
    const adapter = session.adapter;
    if (!adapter.followUp) {
      throw new AgentAdapterError(
        "unsupported_capability",
        `Runtime ${this.runtimeIdOf(adapter) ?? "unknown"} cannot continue a finished turn`
      );
    }
    // The turn boundary is a fact on the timeline, so a reader can tell where the second turn
    // began without reconstructing it from the event stream.
    await this.recordAuditFact(executionId, "turn.started", { prompt });
    return adapter.followUp(executionId, prompt);
  }

  /** Journaled CodeWeaver facts (change applied, policy denied) join the same timeline. */
  async recordAuditFact(executionId: string, category: string, payload: Record<string, unknown>): Promise<void> {
    const session = this.sessions.get(executionId);
    if (session) {
      this.enqueue(session, async () => {
        const entry = await recordAudit(this.options.journal, {
          executionId,
          sequence: session.nextJournalSequence,
          category,
          payload,
          // The same clock the runtime events are recorded with, so one journal carries one time.
          recordedAt: this.now().toISOString()
        });
        session.nextJournalSequence += 1;
        // A CodeWeaver fact is on the timeline too, so the live stream carries it like any other
        // event: design 13 lists `code.diff.applied` and `evaluation.completed` as events.
        await this.options.onRecorded?.(entry);
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

  /**
   * The adapter that owns an execution.
   *
   * With more than one runtime registered, "the adapter" is ambiguous: a permission response or a
   * cancel must reach the runtime that is actually running the task. The session records which
   * adapter started the execution; for an unknown execution the default adapter answers, because
   * guessing wrong would send the call to a runtime that never saw the task.
   */
  adapterFor(executionId: string): AgentRuntimeAdapter {
    return this.sessions.get(executionId)?.adapter ?? this.options.adapter;
  }

  /**
   * The registered id of the runtime behind an adapter.
   *
   * A task has to know which runtime it will run on *before* it starts: skills are authorized
   * against that runtime, and a skill allowed on `pi-local` must not be refused because the
   * server's default happens to be another one.
   */
  runtimeIdOf(adapter: AgentRuntimeAdapter): string | undefined {
    return this.options.runtimeIdOf?.(adapter);
  }

  /** The registered id of the runtime that runs a task when the caller names none. */
  defaultRuntimeId(): string | undefined {
    return this.options.runtimeIdOf?.(this.options.adapter);
  }

  /**
   * Whether this server process still holds the live session for an execution.
   *
   * A restart empties the sessions but not the journal, so an execution can be recorded and
   * describable while nothing about it is running here any more. Callers need to tell those two
   * apart: a live execution can still be cancelled or resumed, a recorded one can only be read.
   */
  isLive(executionId: string): boolean {
    return this.sessions.has(executionId);
  }

  /**
   * The adapter that owns an execution, even when its session is gone.
   *
   * A live session knows its own adapter. A dead one has to be looked up, because falling back to
   * the server's default runtime would move the task to a runtime nobody asked for.
   */
  private async owningAdapter(executionId: string): Promise<AgentRuntimeAdapter> {
    const session = this.sessions.get(executionId);
    if (session) return session.adapter;
    const recovered = await this.options.recoveringAdapter?.(executionId);
    return recovered ?? this.options.adapter;
  }

  /** The default adapter; used when no execution context identifies a runtime. */
  getAdapter(): AgentRuntimeAdapter {
    return this.options.adapter;
  }

  /**
   * The execution that owns a pending approval request.
   *
   * Design 12.3 addresses an approval by its own request id, not by execution id, so the server
   * has to find the execution first. The owner is the adapter that raised the request — the same
   * adapter that must receive the answer.
   */
  async findByPermissionRequest(requestId: string): Promise<string | undefined> {
    for (const [executionId, session] of this.sessions) {
      const pending = session.adapter.getPendingPermissions?.(executionId) ?? [];
      if (pending.some(request => request.requestId === requestId)) return executionId;
    }
    return undefined;
  }

  /** The task input this coordinator started, for lineage and subagent spawning. */
  getTaskInput(executionId: string): AgentTaskInput | undefined {
    return this.sessions.get(executionId)?.input;
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
      await recordRuntimeEvent(this.options.journal, event, sequence, this.now().toISOString());
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
    // A rebuild continues on the same runtime: the checkpoint describes that runtime's work.
    const execution = await this.start(input, await this.owningAdapter(checkpoint.executionId));
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
    "[CODEWEAVER CHECKPOINT]",
    JSON.stringify(summary, null, 2),
    "Do not re-apply any change record listed above; inspect the worktree before proposing new edits."
  ].join("\n");
}
