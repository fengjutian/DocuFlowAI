import { createHash, randomUUID } from "node:crypto";
import { captureContextSnapshot } from "../context/context-snapshot.js";
import { CodeChangePipeline, type ApplyOutcome } from "../code-engine/code-change-pipeline.js";
import { buildRevertRecord, type ApprovalRecord, type ChangeRecord } from "../code-engine/change-record.js";
import type { GitRunner } from "../code-engine/git-runner.js";
import type { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
import type { RecordStore } from "../persistence/record-store.js";
import {
  assertContextSnapshot,
  assertPolicySnapshot,
  assertPromptSnapshot,
  toPolicySnapshotRecord,
  type ContextSnapshotRecord,
  type PolicySnapshotRecord,
  type PromptSnapshotRecord
} from "../persistence/records.js";
import type { TraceStores, TraceabilityQuery } from "../persistence/traceability.js";
import type { PolicySnapshot } from "../policy/permission-policy.js";
import type { AgentExecution, AgentTaskInput } from "../types/adapter.js";

/**
 * End-to-end task entry point: snapshots, execution, change, trace.
 *
 * This is the only place that knows the whole order, so callers cannot start a run against context
 * that was never captured, or apply a patch without a policy snapshot behind it.
 */
export interface StartTaskRequest {
  projectId: string;
  taskId: string;
  featureId?: string;
  prompt: string;
  worktreePath: string;
  documents: string[];
  selectedFiles: string[];
  architectureVersion: string;
  gitnexusIndexVersion?: string;
  policy: PolicySnapshot;
  template: { name: string; version: string; contentHash: string };
  modelConfig: Record<string, unknown>;
  correlationId: string;
  timeoutSeconds: number;
  allowedTools: string[];
}

export interface StartTaskResult {
  execution: AgentExecution;
  taskInput: AgentTaskInput;
  contextSnapshot: ContextSnapshotRecord;
  policySnapshot: PolicySnapshotRecord;
  promptSnapshot: PromptSnapshotRecord;
}

export interface TaskExecutionServiceOptions {
  git: GitRunner;
  coordinator: ExecutionCoordinator;
  changes: CodeChangePipeline;
  stores: TraceStores;
  traceability: TraceabilityQuery;
  now?: () => Date;
  runIdFactory?: () => string;
  snapshotIdFactory?: (prefix: string) => string;
}

interface Session {
  taskInput: AgentTaskInput;
  contextSnapshotId: string;
  policySnapshotId: string;
}

export class TaskExecutionService {
  private readonly sessions = new Map<string, Session>();
  private readonly now: () => Date;

  constructor(private readonly options: TaskExecutionServiceOptions) {
    this.now = options.now ?? (() => new Date());
  }

  async startTask(request: StartTaskRequest): Promise<StartTaskResult> {
    const contextSnapshot = await captureContextSnapshot({
      projectId: request.projectId,
      taskId: request.taskId,
      worktreePath: request.worktreePath,
      git: this.options.git,
      documents: request.documents,
      selectedFiles: request.selectedFiles,
      architectureVersion: request.architectureVersion,
      ...(request.gitnexusIndexVersion ? { gitnexusIndexVersion: request.gitnexusIndexVersion } : {}),
      now: this.now,
      idFactory: () => this.snapshotId("ctx")
    });
    assertContextSnapshot(contextSnapshot);
    await this.options.stores.contextSnapshots.put(contextSnapshot);

    // Policy and prompt records name the execution, which only exists after `start`. Their ids are
    // decided first so the task input can reference them, and they are written once the run exists.
    const policySnapshotId = this.snapshotId("pol");
    const promptSnapshotId = this.snapshotId("prm");

    const taskInput: AgentTaskInput = {
      projectId: request.projectId,
      ...(request.featureId ? { featureId: request.featureId } : {}),
      taskId: request.taskId,
      runId: (this.options.runIdFactory ?? (() => `run_${randomUUID()}`))(),
      prompt: request.prompt,
      contextSnapshotId: contextSnapshot.id,
      policySnapshotId,
      worktreeId: request.taskId,
      workingDirectory: request.worktreePath,
      allowedTools: [...request.allowedTools],
      timeoutSeconds: request.timeoutSeconds,
      correlationId: request.correlationId
    };

    const execution = await this.options.coordinator.start(taskInput);

    const policySnapshot: PolicySnapshotRecord = {
      ...toPolicySnapshotRecord(request.policy, { createdAt: this.now().toISOString() }),
      id: policySnapshotId,
      executionId: execution.executionId
    };
    assertPolicySnapshot(policySnapshot);
    await this.options.stores.policySnapshots.put(policySnapshot);

    const promptSnapshot: PromptSnapshotRecord = {
      id: promptSnapshotId,
      projectId: request.projectId,
      taskId: request.taskId,
      executionId: execution.executionId,
      templateName: request.template.name,
      templateVersion: request.template.version,
      templateContentHash: request.template.contentHash,
      renderedPromptHash: hashRenderedPrompt(request.prompt, contextSnapshot.contextHash),
      contextSnapshotId: contextSnapshot.id,
      modelConfig: { ...request.modelConfig },
      createdAt: this.now().toISOString()
    };
    assertPromptSnapshot(promptSnapshot);
    await this.options.stores.promptSnapshots.put(promptSnapshot);

    this.sessions.set(execution.executionId, {
      taskInput,
      contextSnapshotId: contextSnapshot.id,
      policySnapshotId: policySnapshot.id
    });
    await this.options.coordinator.recordAuditFact(execution.executionId, "context.snapshot", {
      contextSnapshotId: contextSnapshot.id,
      contextHash: contextSnapshot.contextHash,
      gitBaseSha: contextSnapshot.gitBaseSha
    });
    await this.options.coordinator.recordAuditFact(execution.executionId, "policy.snapshot", {
      policySnapshotId: policySnapshot.id,
      policyVersion: policySnapshot.policyVersion
    });
    await this.options.coordinator.recordAuditFact(execution.executionId, "prompt.snapshot", {
      promptSnapshotId: promptSnapshot.id,
      templateVersion: promptSnapshot.templateVersion
    });

    return {
      execution,
      taskInput,
      contextSnapshot,
      policySnapshot,
      promptSnapshot
    };
  }

  /**
   * Applies a proposed patch for an execution that this service started, then records it in three
   * places: the change store, the journal (so checkpoints see it) and the trace.
   */
  async applyProposedChange(
    executionId: string,
    patch: string,
    options: {
      approval?: ApprovalRecord;
      commitMessage?: string;
      /** Override after a process restart, when the session is not in memory. */
      worktreePath?: string;
      expectedBaseSha?: string;
      policy?: PolicySnapshot;
      allowedPaths?: string[];
    } = {}
  ): Promise<ApplyOutcome> {
    const session = this.sessions.get(executionId);
    const context = await this.resolveContext(executionId, session?.contextSnapshotId);
    const policyRecord = await this.resolvePolicy(executionId, session?.policySnapshotId);
    const policy = options.policy ?? toEvaluatorSnapshot(policyRecord);

    const worktreePath = options.worktreePath ?? session?.taskInput.workingDirectory;
    if (!worktreePath) {
      throw new Error(`Execution ${executionId} has no worktree; supply worktreePath explicitly`);
    }

    const outcome = await this.options.changes.apply({
      projectId: policyRecord.projectId,
      taskId: policyRecord.taskId,
      executionId,
      worktreePath,
      expectedBaseSha: options.expectedBaseSha ?? context.gitBaseSha,
      patch,
      policy: {
        snapshot: policy,
        allowedPaths: options.allowedPaths ?? [worktreePath]
      },
      ...(options.approval ? { approval: options.approval } : {}),
      ...(options.commitMessage ? { commit: { message: options.commitMessage } } : {})
    });

    if (outcome.status !== "applied") return outcome;

    await this.options.stores.changeRecords.put(outcome.record);
    await this.options.coordinator.recordAuditFact(executionId, "change.applied", {
      changeRecordId: outcome.record.id,
      diffHash: outcome.record.diffHash,
      ...(outcome.record.gitCommitSha ? { gitCommitSha: outcome.record.gitCommitSha } : {})
    });
    return outcome;
  }

  async trace(executionId: string) {
    return this.options.traceability.byExecution(executionId);
  }

  async traceCommit(commitSha: string) {
    return this.options.traceability.byCommit(commitSha);
  }

  private async resolveContext(executionId: string, knownId?: string): Promise<ContextSnapshotRecord> {
    if (knownId) {
      const found = await this.options.stores.contextSnapshots.get(knownId);
      if (found) return found;
    }
    const prompt = (await this.options.stores.promptSnapshots.list()).find(
      record => record.executionId === executionId
    );
    const context = prompt ? await this.options.stores.contextSnapshots.get(prompt.contextSnapshotId) : undefined;
    if (!context) throw new Error(`No context snapshot is linked to execution ${executionId}`);
    return context;
  }

  private async resolvePolicy(executionId: string, knownId?: string): Promise<PolicySnapshotRecord> {
    if (knownId) {
      const found = await this.options.stores.policySnapshots.get(knownId);
      if (found) return found;
    }
    const policy = (await this.options.stores.policySnapshots.list()).find(
      record => record.executionId === executionId
    );
    if (!policy) throw new Error(`No policy snapshot is linked to execution ${executionId}`);
    return policy;
  }

  private snapshotId(prefix: string): string {
    return this.options.snapshotIdFactory?.(prefix) ?? `${prefix}_${randomUUID()}`;
  }
}

/** Reconstructs the evaluator snapshot from the persisted row, for a later apply. */
export function toEvaluatorSnapshot(record: PolicySnapshotRecord): PolicySnapshot {
  return {
    snapshotId: record.id,
    projectId: record.projectId,
    taskId: record.taskId,
    executionId: record.executionId,
    worktreeId: record.taskId,
    policyVersion: record.policyVersion,
    grantedCapabilities: [...record.grantedCapabilities],
    allowedPaths: [...record.allowedPaths],
    expiresAt: record.expiresAt
  };
}

export function hashRenderedPrompt(prompt: string, contextHash: string): string {
  return createHash("sha256").update(`${prompt}\n${contextHash}`, "utf8").digest("hex");
}

/** Records a revert of an applied change, keeping the original record untouched (design 9.5). */
export async function recordRevert(
  store: RecordStore<ChangeRecord>,
  original: ChangeRecord,
  input: { diffHash: string; gitCommitSha: string; approval: ApprovalRecord; idFactory?: () => string }
): Promise<ChangeRecord> {
  const revert = buildRevertRecord({ original, ...input });
  await store.put(revert);
  return revert;
}
