import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CodeChangePipeline } from "../../code-engine/code-change-pipeline.js";
import { SpawnGitRunner, gitOrThrow } from "../../code-engine/git-runner.js";
import { WorktreeManager } from "../../code-engine/worktree-manager.js";
import { InMemoryJournalStore } from "../../journal/journal.js";
import { ExecutionCoordinator } from "../../orchestrator/execution-coordinator.js";
import { ImmutableRecordStore } from "../../persistence/record-store.js";
import {
  assertContextSnapshot,
  assertPolicySnapshot,
  assertPromptSnapshot,
  type ContextSnapshotRecord,
  type PolicySnapshotRecord,
  type PromptSnapshotRecord
} from "../../persistence/records.js";
import { TraceabilityQuery } from "../../persistence/traceability.js";
import { TaskExecutionService, type StartTaskRequest } from "../../tasks/task-execution-service.js";
import type { PolicySnapshot } from "../../policy/permission-policy.js";
import {
  NO_CAPABILITIES,
  type AgentRuntimeAdapter,
  type AgentRuntimeEvent
} from "../../types/adapter.js";
import type { ChangeRecord } from "../../code-engine/change-record.js";

async function setup() {
  const repo = await mkdtemp(join(tmpdir(), "docuflow-task-"));
  const git = new SpawnGitRunner();
  await gitOrThrow(git, ["init", "-b", "main"], { cwd: repo });
  await gitOrThrow(git, ["config", "user.name", "DocuFlow Test"], { cwd: repo });
  await gitOrThrow(git, ["config", "user.email", "test@docuflow.local"], { cwd: repo });
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
  await writeFile(join(repo, "spec.md"), "# Spec\nRequirement A\n", "utf8");
  await gitOrThrow(git, ["add", "-A"], { cwd: repo });
  await gitOrThrow(git, ["commit", "-m", "initial"], { cwd: repo });
  const baseSha = (await gitOrThrow(git, ["rev-parse", "HEAD"], { cwd: repo })).stdout.trim();

  const storeRoot = await mkdtemp(join(tmpdir(), "docuflow-task-store-"));
  const stores = {
    contextSnapshots: new ImmutableRecordStore<ContextSnapshotRecord>(storeRoot, "contexts", assertContextSnapshot),
    policySnapshots: new ImmutableRecordStore<PolicySnapshotRecord>(storeRoot, "policies", assertPolicySnapshot),
    promptSnapshots: new ImmutableRecordStore<PromptSnapshotRecord>(storeRoot, "prompts", assertPromptSnapshot),
    changeRecords: new ImmutableRecordStore<ChangeRecord>(storeRoot, "changes")
  };

  const journal = new InMemoryJournalStore();
  const started: AgentRuntimeEvent = {
    eventId: "exec-1:1",
    executionId: "exec-1",
    projectId: "proj-1",
    taskId: "task-1",
    sequence: 1,
    eventType: "execution.started",
    timestamp: "2026-11-09T00:00:00.000Z",
    payload: { hermesEvent: "run.started" }
  };
  const adapter: AgentRuntimeAdapter = {
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() {
      return { executionId: "exec-1", runtimeType: "hermes", status: "running", externalRunId: "run-ext-1" };
    },
    async getStatus() { return "running"; },
    async cancel() { return undefined; },
    async resume() { throw new Error("not used"); },
    async subscribe(_id, handler) {
      handler(started);
      return async () => undefined;
    },
    async terminate() { return undefined; }
  };

  const coordinator = new ExecutionCoordinator({ adapter, journal });
  const worktrees = new WorktreeManager(git, join(repo, ".worktrees"), { repository: repo });
  const changes = new CodeChangePipeline(git, worktrees);
  const traceability = new TraceabilityQuery(stores);

  const policy: PolicySnapshot = {
    snapshotId: "snap-1",
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "",
    worktreeId: "wt-1",
    policyVersion: "1.0.0",
    grantedCapabilities: ["*"],
    allowedPaths: [repo],
    expiresAt: "2099-01-01T00:00:00.000Z"
  };

  const service = new TaskExecutionService({
    git,
    coordinator,
    changes,
    stores,
    traceability,
    runIdFactory: () => "run-1",
    snapshotIdFactory: prefix => `${prefix}-1`
  });

  const request: StartTaskRequest = {
    projectId: "proj-1",
    taskId: "task-1",
    prompt: "change value to 2",
    worktreePath: repo,
    documents: ["spec.md"],
    selectedFiles: ["app.ts"],
    architectureVersion: "1.0",
    policy,
    template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
    modelConfig: { model: "hermes-default" },
    correlationId: "corr-1",
    timeoutSeconds: 30,
    allowedTools: ["read_file"]
  };

  return { repo, baseSha, stores, journal, coordinator, service, request };
}

const patch = [
  "diff --git a/app.ts b/app.ts",
  "--- a/app.ts",
  "+++ b/app.ts",
  "@@ -1 +1 @@",
  "-export const value = 1;",
  "+export const value = 2;",
  ""
].join("\n");

test("starts a task with context, policy and prompt snapshots", async () => {
  const { repo, baseSha, stores, journal, coordinator, service, request } = await setup();
  try {
    const result = await service.startTask(request);

    assert.equal(result.execution.executionId, "exec-1");
    assert.equal(result.taskInput.runId, "run-1");
    assert.equal(result.contextSnapshot.gitBaseSha, baseSha);

    assert.equal((await stores.contextSnapshots.get("ctx-1"))?.contextHash, result.contextSnapshot.contextHash);
    assert.equal((await stores.policySnapshots.get("pol-1"))?.executionId, "exec-1");
    const prompt = await stores.promptSnapshots.get("prm-1");
    assert.equal(prompt?.contextSnapshotId, "ctx-1");
    assert.equal(prompt?.executionId, "exec-1");

    await coordinator.flush("exec-1");
    const categories = (await journal.read("exec-1")).map(entry => entry.category);
    assert.ok(categories.includes("context.snapshot"));
    assert.ok(categories.includes("policy.snapshot"));
    assert.ok(categories.includes("prompt.snapshot"));
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("applies a change and makes it traceable from the commit", async () => {
  const { repo, stores, journal, coordinator, service, request } = await setup();
  try {
    await service.startTask(request);
    const outcome = await service.applyProposedChange("exec-1", patch, { commitMessage: "feat: value 2" });

    assert.equal(outcome.status, "applied");
    if (outcome.status !== "applied") return;
    assert.match(await readFile(join(repo, "app.ts"), "utf8"), /value = 2/);
    assert.equal((await stores.changeRecords.list()).length, 1);
    assert.ok(outcome.record.gitCommitSha);

    const trace = await service.trace("exec-1");
    assert.equal(trace.changeRecords.length, 1);
    assert.equal(trace.changeRecords[0]!.id, outcome.record.id);
    assert.equal(trace.contextSnapshots.length, 1);
    assert.equal(trace.promptSnapshots.length, 1);
    assert.equal(trace.policySnapshots.length, 1);

    const fromCommit = await service.traceCommit(outcome.record.gitCommitSha!);
    assert.equal(fromCommit?.changeRecord.executionId, "exec-1");

    const checkpoint = await coordinator.getCheckpoint("exec-1");
    assert.deepEqual(checkpoint.appliedChangeRecordIds, [outcome.record.id]);

    await coordinator.flush("exec-1");
    assert.ok((await journal.read("exec-1")).some(entry => entry.category === "change.applied"));
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("holds a risky change for approval without recording it", async () => {
  const { repo, stores, service, request } = await setup();
  try {
    await service.startTask(request);
    const risky = [
      "diff --git a/package.json b/package.json",
      "--- a/package.json",
      "+++ b/package.json",
      "@@ -1 +1 @@",
      "-{}",
      "+{\"dependencies\":{}}",
      ""
    ].join("\n");

    const outcome = await service.applyProposedChange("exec-1", risky);

    assert.equal(outcome.status, "awaiting_approval");
    assert.deepEqual(await stores.changeRecords.list(), []);
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});
