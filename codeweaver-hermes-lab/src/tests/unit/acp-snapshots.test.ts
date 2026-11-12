import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createCodeWeaverAcpBackend, toSessionUpdate } from "../../acp/codeweaver-backend.js";
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
import { TaskExecutionService } from "../../tasks/task-execution-service.js";
import type { ChangeRecord } from "../../code-engine/change-record.js";
import type { EvaluationResult } from "../../evaluation/evaluation.js";
import type { PolicySnapshot } from "../../policy/permission-policy.js";
import {
  NO_CAPABILITIES,
  type AgentRuntimeAdapter,
  type AgentRuntimeEvent
} from "../../types/adapter.js";

function adapter(): AgentRuntimeAdapter {
  const completed: AgentRuntimeEvent = {
    eventId: "e1",
    executionId: "exec-1",
    projectId: "zed",
    taskId: "acp-sess-1",
    sequence: 1,
    eventType: "execution.completed",
    timestamp: "2026-11-09T00:00:00.000Z",
    payload: {}
  };
  return {
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() {
      return { executionId: "exec-1", runtimeType: "hermes", status: "running", externalRunId: "run-1" };
    },
    async getStatus() { return "succeeded"; },
    async cancel() { return undefined; },
    async resume() { throw new Error("not used"); },
    async subscribe(_id, handler) {
      handler(completed);
      return async () => undefined;
    },
    async terminate() { return undefined; }
  };
}

async function harness(
  repo: string,
  storeRoot: string,
  overrides: { runtimeId?: string; resolveRuntime?: (id: string) => AgentRuntimeAdapter | undefined } = {}
) {
  const git = new SpawnGitRunner();
  const stores = {
    contextSnapshots: new ImmutableRecordStore<ContextSnapshotRecord>(storeRoot, "contexts", assertContextSnapshot),
    policySnapshots: new ImmutableRecordStore<PolicySnapshotRecord>(storeRoot, "policies", assertPolicySnapshot),
    promptSnapshots: new ImmutableRecordStore<PromptSnapshotRecord>(storeRoot, "prompts", assertPromptSnapshot),
    changeRecords: new ImmutableRecordStore<ChangeRecord>(storeRoot, "changes"),
    evaluations: new ImmutableRecordStore<EvaluationResult>(storeRoot, "evaluations")
  };
  const journal = new InMemoryJournalStore();
  const defaultAdapter = adapter();
  const coordinator = new ExecutionCoordinator({
    adapter: defaultAdapter,
    journal,
    // Which runtime ran is a fact on the timeline; the harness names the adapter it hands out.
    runtimeIdOf: candidate => (candidate === defaultAdapter ? "pi-local" : undefined)
  });
  const worktrees = new WorktreeManager(git, join(repo, ".worktrees"), { repository: repo });
  const taskService = new TaskExecutionService({
    git,
    coordinator,
    changes: new CodeChangePipeline(git, worktrees),
    stores,
    traceability: new TraceabilityQuery(stores, journal),
    ...(overrides.resolveRuntime
      ? { resolveRuntime: overrides.resolveRuntime }
      : { resolveRuntime: async (id: string) => (id === "pi-local" ? defaultAdapter : undefined) })
  });

  const warnings: string[] = [];
  const backend = createCodeWeaverAcpBackend({
    coordinator,
    defaults: {
      projectId: "zed",
      contextSnapshotId: "ctx",
      policySnapshotId: "pol",
      timeoutSeconds: 5,
      ...(overrides.runtimeId ? { runtimeId: overrides.runtimeId } : {})
    },
    idFactory: () => "sess-1",
    runIdFactory: () => "run-acp-1",
    onWarning: message => warnings.push(message),
    snapshots: {
      taskService,
      policyFor: ({ cwd, taskId }): PolicySnapshot => ({
        snapshotId: `pol_acp_${taskId}`,
        projectId: "zed",
        taskId,
        executionId: "",
        worktreeId: taskId,
        policyVersion: "acp-default-1",
        grantedCapabilities: ["read_file", "search_code", "generate_diff"],
        allowedPaths: [cwd],
        expiresAt: new Date(Date.now() + 86_400_000).toISOString()
      })
    }
  });

  return { stores, journal, backend, warnings };
}

async function gitRepo(): Promise<string> {
  const repo = await mkdtemp(join(tmpdir(), "codeweaver-acp-snap-"));
  const git = new SpawnGitRunner();
  await gitOrThrow(git, ["init", "-b", "main"], { cwd: repo });
  await gitOrThrow(git, ["config", "user.name", "CodeWeaver Test"], { cwd: repo });
  await gitOrThrow(git, ["config", "user.email", "test@codeweaver.local"], { cwd: repo });
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
  await gitOrThrow(git, ["add", "-A"], { cwd: repo });
  await gitOrThrow(git, ["commit", "-m", "initial"], { cwd: repo });
  return repo;
}

test("acp snapshots: a prompt creates context, policy and prompt records plus audit entries", async () => {
  const repo = await gitRepo();
  const storeRoot = await mkdtemp(join(tmpdir(), "codeweaver-acp-snap-store-"));
  try {
    const { stores, journal, backend } = await harness(repo, storeRoot);
    const session = await backend.createSession({ cwd: repo });
    const stopReason = await backend.prompt(session.sessionId, "do the thing", () => undefined);
    assert.equal(stopReason, "end_turn");

    assert.equal((await stores.contextSnapshots.list()).length, 1);
    assert.equal((await stores.policySnapshots.list()).length, 1);
    assert.equal((await stores.promptSnapshots.list()).length, 1);

    const context = (await stores.contextSnapshots.list())[0]!;
    assert.match(context.gitBaseSha, /^[0-9a-f]{40}$/);
    assert.equal(context.architectureVersion, "acp");

    const policy = (await stores.policySnapshots.list())[0]!;
    assert.equal(policy.executionId, "exec-1");
    assert.deepEqual(policy.allowedPaths, [repo]);

    const categories = (await journal.read("exec-1")).map(entry => entry.category);
    assert.ok(categories.includes("context.snapshot"));
    assert.ok(categories.includes("policy.snapshot"));
    assert.ok(categories.includes("prompt.snapshot"));
  } finally {
    await rm(repo, { recursive: true, force: true });
    await rm(storeRoot, { recursive: true, force: true });
  }
});

test("acp snapshots: a non-git cwd degrades to a journaled skip instead of a fake snapshot", async () => {  const plain = await mkdtemp(join(tmpdir(), "codeweaver-acp-plain-"));
  const storeRoot = await mkdtemp(join(tmpdir(), "codeweaver-acp-plain-store-"));
  try {
    const { stores, journal, backend, warnings } = await harness(plain, storeRoot);
    const session = await backend.createSession({ cwd: plain });
    const stopReason = await backend.prompt(session.sessionId, "do the thing", () => undefined);

    assert.equal(stopReason, "end_turn");
    assert.equal((await stores.contextSnapshots.list()).length, 0);
    assert.equal((await stores.policySnapshots.list()).length, 0);
    assert.equal(warnings.length, 1);
    assert.match(warnings[0]!, /snapshots skipped/);

    const entries = await journal.read("exec-1");
    const skipped = entries.find(entry => entry.category === "context.snapshot.skipped");
    assert.ok(skipped, "the skip must be auditable, not just a stderr line");
    assert.equal(skipped?.payload["cwd"], plain);
    assert.match(String(skipped?.payload["reason"]), /not a git repository/i);
  } finally {
    await rm(plain, { recursive: true, force: true });
    await rm(storeRoot, { recursive: true, force: true });
  }
});

test("acp: the editor session runs on the runtime it names", async () => {
  const repo = await gitRepo();
  const storeRoot = await mkdtemp(join(tmpdir(), "codeweaver-acp-runtime-store-"));
  try {
    const { journal, backend } = await harness(repo, storeRoot, {
      runtimeId: "pi-local"
    });
    const session = await backend.createSession({ cwd: repo });
    const stopReason = await backend.prompt(session.sessionId, "do the thing", () => undefined);
    assert.equal(stopReason, "end_turn");

    // The runtime that ran is on the timeline: an editor run that lands somewhere else is a bug
    // nobody can see.
    const runtimeFact = (await journal.read("exec-1")).find(entry => entry.category === "execution.runtime");
    assert.equal(runtimeFact?.payload["runtimeId"], "pi-local");
  } finally {
    await rm(repo, { recursive: true, force: true });
    await rm(storeRoot, { recursive: true, force: true });
  }
});

test("acp: a runtime that is not registered is refused, not silently replaced", async () => {
  const repo = await gitRepo();
  const storeRoot = await mkdtemp(join(tmpdir(), "codeweaver-acp-missing-store-"));
  try {
    const { backend } = await harness(repo, storeRoot, {
      runtimeId: "no-such-runtime",
      resolveRuntime: () => undefined
    });
    const session = await backend.createSession({ cwd: repo });
    await assert.rejects(
      () => backend.prompt(session.sessionId, "do the thing", () => undefined),
      /no-such-runtime|Unknown runtime/
    );
  } finally {
    await rm(repo, { recursive: true, force: true });
    await rm(storeRoot, { recursive: true, force: true });
  }
});

/**
 * A tool call the editor shows.
 *
 * The runtimes name the same thing differently — Pi carries `toolName`, Hermes carries `tool` —
 * and the editor has only one label to draw. If the ACP layer reads only one of the keys, every
 * tool call reaches the editor as "tool": the call was real, its name was not.
 */
function toolStarted(payload: Record<string, unknown>): AgentRuntimeEvent {
  return {
    eventId: "e-tool",
    executionId: "exec-1",
    projectId: "zed",
    taskId: "acp-sess-1",
    sequence: 2,
    eventType: "tool.started",
    timestamp: "2026-11-09T00:00:00.000Z",
    payload
  };
}

test("acp: a tool call carries the name the runtime gave it", () => {
  const piUpdate = toSessionUpdate(toolStarted({ toolCallId: "call-1", toolName: "read_file" }));
  assert.equal(piUpdate?.sessionUpdate, "tool_call");
  assert.equal(piUpdate?.title, "read_file");
  assert.equal(piUpdate?.toolCallId, "call-1");

  // The same call from a runtime that names it the other way.
  const hermesUpdate = toSessionUpdate(toolStarted({ toolCallId: "call-2", tool: "search_code" }));
  assert.equal(hermesUpdate?.title, "search_code");
});

test("acp: a tool call without a name still reaches the editor", () => {
  const update = toSessionUpdate(toolStarted({ toolCallId: "call-3" }));
  assert.equal(update?.sessionUpdate, "tool_call");
  assert.equal(update?.title, "tool");
});
