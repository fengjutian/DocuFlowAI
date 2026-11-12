import { createHash, randomUUID } from "node:crypto";
import { captureContextSnapshot } from "../context/context-snapshot.js";
import { CodeChangePipeline, type ApplyOutcome } from "../code-engine/code-change-pipeline.js";
import { buildRevertRecord, type ApprovalRecord, type ChangeRecord } from "../code-engine/change-record.js";
import type { GitRunner } from "../code-engine/git-runner.js";
import type { CommandRunner } from "../evaluation/command-runner.js";
import {
  completionGate as evaluateCompletionGate,
  evaluateChecks,
  type CompletionGate,
  type EvaluationCheckSpec,
  type EvaluationResult
} from "../evaluation/evaluation.js";
import type { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
import type { JournalStore } from "../journal/journal.js";
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
import type { WorktreeScope } from "../policy/worktree-scope.js";
import type { SkillBinding, SkillBindingStore } from "../skills/skill-bindings.js";
import { composePromptWithSkills, describeDeliveries } from "../skills/skill-delivery.js";
import { summarizeSkillEffectiveness, type SkillEffectiveness } from "../skills/skill-effectiveness.js";
import type { SkillRegistry, SkillAuthorization } from "../skills/skill-registry.js";
import type { SkillManifest } from "../skills/skill.js";
import type { AgentExecution, AgentRuntimeAdapter, AgentTaskInput } from "../types/adapter.js";

/** A skill the caller wants bound to this execution. */
export interface RequestedSkill {
  skillId: string;
  version?: string;
}

export class SkillDeniedError extends Error {
  constructor(message: string, public readonly reasons: string[] = []) {
    super(message);
    this.name = "SkillDeniedError";
  }
}

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
  /** Skills to bind to this execution; each is authorized against `policy` first. */
  skills?: RequestedSkill[];
  /** `skillId@version` entries the caller has already had approved by a human. */
  approvedSkills?: string[];
  /**
   * The acceptance checks this task must pass.
   *
   * They are declared by the caller, not by the agent: the runtime may run them, but it may not
   * write them. A task without checks has no evidence, and the completion gate stays closed.
   */
  evaluationChecks?: EvaluationCheckSpec[];
  /**
   * Which registered runtime runs this task. Omitted means the server's default runtime.
   *
   * The runtime id is a CodeWeaver concept, not a runtime-specific one: the caller names the
   * registered runtime, and CodeWeaver resolves it to the adapter behind that id.
   */
  runtimeId?: string;
}

export interface StartTaskResult {
  execution: AgentExecution;
  taskInput: AgentTaskInput;
  contextSnapshot: ContextSnapshotRecord;
  policySnapshot: PolicySnapshotRecord;
  promptSnapshot: PromptSnapshotRecord;
  skills: SkillBinding[];
}

export interface TaskExecutionServiceOptions {
  git: GitRunner;
  coordinator: ExecutionCoordinator;
  changes: CodeChangePipeline;
  stores: TraceStores;
  traceability: TraceabilityQuery;
  /**
   * The journal, for executions this service did not start.
   *
   * A delegated child is started by the subagent manager, not here, so it has no session in this
   * service. Its request — the prompt, the runtime, the policy and the workspace it runs in — travels
   * with it in the journal, the same way the runtime id and the task input do (D-055). Without the
   * journal here, a delegated child's tool surface has no policy to read and every tool call fails.
   */
  journal?: JournalStore;
  /** Optional skill support: without it, `skills` in a request is rejected rather than ignored. */
  skills?: { registry: SkillRegistry; bindings: SkillBindingStore };
  /**
   * Resolves a runtime id to the adapter behind it. Without it every task runs on the server's
   * default runtime, and a request that names a different runtime is rejected rather than
   * silently redirected. The special id `auto` lets the server pick.
   */
  resolveRuntime?: (runtimeId: string) => AgentRuntimeAdapter | undefined | Promise<AgentRuntimeAdapter | undefined>;
  now?: () => Date;
  runIdFactory?: () => string;
  snapshotIdFactory?: (prefix: string) => string;
}

interface Session {
  taskInput: AgentTaskInput;
  contextSnapshotId: string;
  policySnapshotId: string;
  /** The acceptance checks declared for this task; empty means "no evidence declared". */
  evaluationChecks: readonly EvaluationCheckSpec[];
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

    // The runtime is resolved before the skills are authorized: a skill is authorized against the
    // runtime the task will actually run on. Authorizing against the server's default would refuse
    // a `pi-local` skill on a Pi run and allow a `hermes-local` skill there, which is the opposite
    // of what the author declared.
    const runtime = await this.selectRuntime(request.runtimeId);
    const runtimeId = this.runtimeIdFor(request.runtimeId, runtime);

    // Skills are authorized against the same policy snapshot the execution will run under, before
    // anything starts. A denied skill stops the task; nothing is "bound but disabled" silently.
    const resolvedSkills = this.authorizeSkills(request, runtimeId);

    // The prompt the runtime receives carries the authorized skill bodies. The user's own words stay
    // first; the skills are appended, and the journal hashes what was actually delivered.
    const effectivePrompt = composePromptWithSkills(request.prompt, resolvedSkills);

    // Policy and prompt records name the execution, which only exists after `start`. Their ids are
    // decided first so the task input can reference them, and they are written once the run exists.
    const policySnapshotId = this.snapshotId("pol");
    const promptSnapshotId = this.snapshotId("prm");

    const taskInput: AgentTaskInput = {
      projectId: request.projectId,
      ...(request.featureId ? { featureId: request.featureId } : {}),
      taskId: request.taskId,
      runId: (this.options.runIdFactory ?? (() => `run_${randomUUID()}`))(),
      prompt: effectivePrompt,
      contextSnapshotId: contextSnapshot.id,
      policySnapshotId,
      worktreeId: request.taskId,
      workingDirectory: request.worktreePath,
      allowedTools: [...request.allowedTools],
      timeoutSeconds: request.timeoutSeconds,
      correlationId: request.correlationId
    };

    const execution = await this.options.coordinator.start(taskInput, runtime);

    // Which runtime ran this task, and what it was asked, are journaled by the coordinator: it is
    // the one that starts executions, and a rebuild goes through it too.

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
      renderedPromptHash: hashRenderedPrompt(effectivePrompt, contextSnapshot.contextHash),
      contextSnapshotId: contextSnapshot.id,
      modelConfig: { ...request.modelConfig },
      createdAt: this.now().toISOString()
    };
    assertPromptSnapshot(promptSnapshot);
    await this.options.stores.promptSnapshots.put(promptSnapshot);

    this.sessions.set(execution.executionId, {
      taskInput,
      contextSnapshotId: contextSnapshot.id,
      policySnapshotId: policySnapshot.id,
      evaluationChecks: [...(request.evaluationChecks ?? [])]
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
      templateVersion: promptSnapshot.templateVersion,
      // How much of the prompt was skill instructions, so a reader can tell an instructed run from
      // a bare one without opening the model.
      skillInstructionsBytes: Buffer.byteLength(
        effectivePrompt.slice(request.prompt.length),
        "utf8"
      )
    });

    const skills: SkillBinding[] = [];
    for (const resolved of resolvedSkills) {
      const binding = await this.options.skills!.bindings.bind({
        executionId: execution.executionId,
        manifest: resolved.manifest,
        policyId: policySnapshot.id,
        authorization: resolved.authorization
      });
      skills.push(binding);
      await this.options.coordinator.recordAuditFact(execution.executionId, "skill.bound", {
        skillId: binding.skillId,
        skillVersion: binding.skillVersion,
        checksum: resolved.manifest.checksum
      });
    }

    // The delivery record is what says the body reached the runtime, not just that a version was
    // bound to the execution. A binding without a delivery is a skill that was authorized and then
    // quietly dropped.
    for (const delivery of describeDeliveries(resolvedSkills)) {
      await this.options.coordinator.recordAuditFact(execution.executionId, "skill.delivered", {
        ...delivery,
        runtimeId: runtimeId ?? null
      });
    }

    return {
      execution,
      taskInput,
      contextSnapshot,
      policySnapshot,
      promptSnapshot,
      skills
    };
  }

  /**
   * The worktree scope of an execution: its own directory and the worktrees of its repository.
   *
   * The tool surface asks for this on every call, because the worktrees of the repository can
   * change while an execution runs — a delegated child gets one of its own.
   */
  async worktreeScope(worktreePath: string): Promise<WorktreeScope> {
    return this.options.changes.worktreeScope(worktreePath);
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

  /**
   * Runs the evaluation checks for an execution and records the result.
   *
   * Checks are independent processes, not the agent's own claim: a change is only complete when an
   * external command says so (AT-204). The result lands in the immutable store and in the journal.
   */
  async evaluateChange(
    executionId: string,
    checks: readonly EvaluationCheckSpec[],
    options: { changeRecordId?: string; commitSha?: string; runner?: CommandRunner } = {}
  ): Promise<EvaluationResult> {
    const result = await evaluateChecks(executionId, checks, options.runner, {
      ...(options.changeRecordId ? { changeRecordId: options.changeRecordId } : {}),
      ...(options.commitSha ? { commitSha: options.commitSha } : {})
    });
    await this.options.stores.evaluations.put(result);
    await this.options.coordinator.recordAuditFact(executionId, "evaluation.completed", {
      evaluationId: result.id,
      verdict: result.verdict,
      checks: result.checks.length,
      ...(options.changeRecordId ? { changeRecordId: options.changeRecordId } : {})
    });
    return result;
  }

  /** Latest evaluation for an execution, gated: no result or a failing result denies completion. */
  async completionGate(executionId: string): Promise<CompletionGate> {
    const results = (await this.options.stores.evaluations.list())
      .filter(result => result.executionId === executionId)
      .sort((a, b) => a.finishedAt.localeCompare(b.finishedAt));
    return evaluateCompletionGate(results.at(-1));
  }

  /**
   * The policy snapshot an execution runs under, in the shape the policy engine reads.
   *
   * The tool surface asks for it at call time, because the snapshot is written after the run starts
   * and the tool must be judged by the snapshot the execution actually runs under.
   */
  async policyFor(executionId: string): Promise<PolicySnapshot> {
    const session = this.sessions.get(executionId);
    if (session) {
      const record = await this.resolvePolicy(executionId, session.policySnapshotId);
      return toEvaluatorSnapshot(record);
    }
    // A delegated child was not started by this service, so it has no session here. Its policy
    // travels with its request, and the request is in the journal.
    const delegated = await this.delegatedPolicy(executionId);
    if (delegated) return delegated;
    const record = await this.resolvePolicy(executionId, undefined);
    return toEvaluatorSnapshot(record);
  }

  /** The acceptance checks the task declared; empty means the task has no evidence of its own. */
  evaluationChecksFor(executionId: string): readonly EvaluationCheckSpec[] {
    return this.sessions.get(executionId)?.evaluationChecks ?? [];
  }

  /**
   * Resolves the runtime a task should run on.
   *
   * A request that names a runtime is honoured or refused, never quietly redirected: running a task
   * on a different runtime than the caller asked for would change which policy and tools apply.
   */
  private async selectRuntime(runtimeId: string | undefined): Promise<AgentRuntimeAdapter | undefined> {
    if (!runtimeId) return undefined;
    const resolve = this.options.resolveRuntime;
    if (!resolve) {
      throw new Error(`Runtime ${runtimeId} was requested but this server has no runtime registry`);
    }
    const adapter = await resolve(runtimeId);
    if (!adapter) throw new Error(`Unknown runtime ${runtimeId}`);
    return adapter;
  }

  /**
   * The registered id of the runtime a task will run under.
   *
   * A named runtime is its own id. "auto" means the scheduler picked, so the adapter's registered
   * id is the answer. No name means the server's default runtime. When none of that yields an id,
   * the caller gets `undefined` and the skill authorization fails closed.
   */
  private runtimeIdFor(
    requested: string | undefined,
    resolved: AgentRuntimeAdapter | undefined
  ): string | undefined {
    if (requested && requested !== "auto") return requested;
    if (resolved) return this.options.coordinator.runtimeIdOf(resolved);
    return this.options.coordinator.defaultRuntimeId();
  }

  /** Validates the requested skill set before anything starts. */
  private authorizeSkills(
    request: StartTaskRequest,
    runtimeId: string | undefined
  ): Array<{ manifest: SkillManifest; authorization: SkillAuthorization }> {
    const requested = request.skills ?? [];
    if (requested.length === 0) return [];
    if (!this.options.skills) {
      throw new SkillDeniedError("Skills are not configured on this server");
    }
    const approved = new Set(request.approvedSkills ?? []);
    const resolved: Array<{ manifest: SkillManifest; authorization: SkillAuthorization }> = [];

    for (const skill of requested) {
      const manifest = this.options.skills.registry.resolve(skill.skillId, skill.version);
      const authorization = this.options.skills.registry.authorize(manifest, {
        runtimeId,
        policy: request.policy
      });
      const key = `${manifest.skillId}@${manifest.version}`;
      if (authorization.decision === "deny") {
        throw new SkillDeniedError(`Skill ${key} is denied`, authorization.reasons);
      }
      if (authorization.decision === "require_approval" && !approved.has(key)) {
        throw new SkillDeniedError(`Skill ${key} requires approval`, authorization.reasons);
      }
      resolved.push({ manifest, authorization });
    }
    return resolved;
  }

  /** Aggregates evaluation verdicts per bound skill version (Phase 4). */
  async skillEffectiveness(): Promise<SkillEffectiveness[]> {
    if (!this.options.skills) return [];
    return summarizeSkillEffectiveness(
      this.options.skills.bindings.list(),
      await this.options.stores.evaluations.list()
    );
  }

  async trace(executionId: string) {
    return this.options.traceability.byExecution(executionId);
  }

  /** Every execution a task produced, each with its own chain. */
  async traceTask(taskId: string) {
    return this.options.traceability.byTask(taskId);
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
    if (context) return context;
    // A delegated child's request names the context snapshot it was given. The journal is where that
    // request is, so the child resolves the same context its parent did.
    const request = await this.journaledRequest(executionId);
    const contextSnapshotId = request?.["contextSnapshotId"];
    if (typeof contextSnapshotId === "string" && contextSnapshotId !== "") {
      const found = await this.options.stores.contextSnapshots.get(contextSnapshotId);
      if (found) return found;
    }
    throw new Error(`No context snapshot is linked to execution ${executionId}`);
  }

  /**
   * The policy a delegated child runs under.
   *
   * It inherits its parent's capabilities, but it is scoped to the workspace it was given: a child
   * that could act in the parent's workspace would not be isolated from it, and the tool surface
   * would stop being the place where the isolation is enforced.
   */
  private async delegatedPolicy(executionId: string): Promise<PolicySnapshot | undefined> {
    const request = await this.journaledRequest(executionId);
    const policySnapshotId = request?.["policySnapshotId"];
    if (typeof policySnapshotId !== "string" || policySnapshotId === "") return undefined;
    const record = await this.options.stores.policySnapshots.get(policySnapshotId);
    if (!record) return undefined;
    const inherited = toEvaluatorSnapshot(record);
    const workingDirectory = request?.["workingDirectory"];
    if (typeof workingDirectory !== "string" || workingDirectory === "") return inherited;
    const worktreeId = request?.["worktreeId"];
    return {
      ...inherited,
      executionId,
      worktreeId: typeof worktreeId === "string" && worktreeId !== "" ? worktreeId : inherited.worktreeId,
      allowedPaths: [workingDirectory]
    };
  }

  /** The request an execution was started with, as the journal recorded it. */
  private async journaledRequest(executionId: string): Promise<Record<string, unknown> | undefined> {
    const journal = this.options.journal;
    if (!journal) return undefined;
    const entries = await journal.read(executionId);
    return entries.find(entry => entry.category === "execution.input")?.payload;
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
