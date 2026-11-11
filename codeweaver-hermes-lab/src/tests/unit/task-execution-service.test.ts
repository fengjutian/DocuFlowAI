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
import { TaskExecutionService, SkillDeniedError, type StartTaskRequest } from "../../tasks/task-execution-service.js";
import { SkillRegistry } from "../../skills/skill-registry.js";
import { SkillBindingStore } from "../../skills/skill-bindings.js";
import { skillContentChecksum, type SkillManifest } from "../../skills/skill.js";
import type { PolicySnapshot } from "../../policy/permission-policy.js";
import {
  NO_CAPABILITIES,
  type AgentRuntimeAdapter,
  type AgentRuntimeEvent,
  type AgentTaskInput
} from "../../types/adapter.js";
import type { ChangeRecord } from "../../code-engine/change-record.js";
import type { EvaluationResult } from "../../evaluation/evaluation.js";

async function setup(options: {
  resolveRuntime?: (runtimeId: string) => AgentRuntimeAdapter | undefined | Promise<AgentRuntimeAdapter | undefined>;
} = {}) {
  const repo = await mkdtemp(join(tmpdir(), "codeweaver-task-"));
  const git = new SpawnGitRunner();
  await gitOrThrow(git, ["init", "-b", "main"], { cwd: repo });
  await gitOrThrow(git, ["config", "user.name", "CodeWeaver Test"], { cwd: repo });
  await gitOrThrow(git, ["config", "user.email", "test@codeweaver.local"], { cwd: repo });
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
  await writeFile(join(repo, "spec.md"), "# Spec\nRequirement A\n", "utf8");
  await gitOrThrow(git, ["add", "-A"], { cwd: repo });
  await gitOrThrow(git, ["commit", "-m", "initial"], { cwd: repo });
  const baseSha = (await gitOrThrow(git, ["rev-parse", "HEAD"], { cwd: repo })).stdout.trim();

  const storeRoot = await mkdtemp(join(tmpdir(), "codeweaver-task-store-"));
  const stores = {
    contextSnapshots: new ImmutableRecordStore<ContextSnapshotRecord>(storeRoot, "contexts", assertContextSnapshot),
    policySnapshots: new ImmutableRecordStore<PolicySnapshotRecord>(storeRoot, "policies", assertPolicySnapshot),
    promptSnapshots: new ImmutableRecordStore<PromptSnapshotRecord>(storeRoot, "prompts", assertPromptSnapshot),
    changeRecords: new ImmutableRecordStore<ChangeRecord>(storeRoot, "changes"),
    evaluations: new ImmutableRecordStore<EvaluationResult>(storeRoot, "evaluations")
  };

  const journal = new InMemoryJournalStore();
  /** The request the runtime was actually started with; the prompt under test. */
  let startedInput: AgentTaskInput | undefined;
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
    async start(input) {
      startedInput = input;
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

  const coordinator = new ExecutionCoordinator({ adapter, journal, runtimeIdOf: () => "hermes-local" });
  const worktrees = new WorktreeManager(git, join(repo, ".worktrees"), { repository: repo });
  const changes = new CodeChangePipeline(git, worktrees);
  const traceability = new TraceabilityQuery(stores, journal);
  const skillRegistry = new SkillRegistry();
  const skillBindings = new SkillBindingStore();
  const SKILL_CONTENT = "# React development\n\nPrefer function components and hooks.\n";
  const skill = (overrides: Partial<SkillManifest> = {}): SkillManifest => ({
    skillId: "react-dev",
    version: "1.0.0",
    name: "React development",
    kind: "engineering",
    permissions: ["read_file", "search_code", "modify_worktree"],
    allowedRuntimes: ["hermes-local"],
    riskLevel: "medium",
    approvalPolicy: "auto",
    content: SKILL_CONTENT,
    checksum: skillContentChecksum(SKILL_CONTENT),
    createdAt: "2026-11-09T00:00:00.000Z",
    ...overrides
  });
  await skillRegistry.register(skill());
  await skillRegistry.register(skill({ skillId: "deploy", permissions: [], riskLevel: "high" }));
  await skillRegistry.register(skill({
    skillId: "net-probe",
    permissions: ["network_access"],
    allowedRuntimes: ["*"]
  }));

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
    skills: { registry: skillRegistry, bindings: skillBindings },
    runIdFactory: () => "run-1",
    snapshotIdFactory: prefix => `${prefix}-1`,
    ...(options.resolveRuntime ? { resolveRuntime: options.resolveRuntime } : {})
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

  return {
    repo,
    baseSha,
    stores,
    journal,
    coordinator,
    service,
    request,
    /** The request the runtime was started with. */
    startedInput: () => startedInput
  };
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

test("runs a task on the runtime the caller named", async () => {
  const chosen: AgentRuntimeAdapter = {
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() { return { executionId: "exec-pi", runtimeType: "native", status: "running" }; },
    async getStatus() { return "running"; },
    async cancel() { return undefined; },
    async resume() { throw new Error("not used"); },
    async subscribe() { return async () => undefined; },
    async terminate() { return undefined; }
  };
  const asked: string[] = [];
  const { repo, coordinator, service, request } = await setup({
    resolveRuntime: runtimeId => {
      asked.push(runtimeId);
      return chosen;
    }
  });
  try {
    await service.startTask({ ...request, runtimeId: "pi-local" });
    assert.deepEqual(asked, ["pi-local"]);
    assert.equal(coordinator.adapterFor("exec-pi"), chosen, "the named runtime owns the execution");
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("refuses an unknown runtime instead of running on another one", async () => {
  const { repo, service, request } = await setup({ resolveRuntime: () => undefined });
  try {
    await assert.rejects(
      () => service.startTask({ ...request, runtimeId: "does-not-exist" }),
      /Unknown runtime does-not-exist/
    );
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("refuses a named runtime when this server has no runtime registry", async () => {
  const { repo, service, request } = await setup();
  try {
    await assert.rejects(
      () => service.startTask({ ...request, runtimeId: "pi-local" }),
      /no runtime registry/
    );
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("the policy of an execution is the snapshot it runs under", async () => {
  const { repo, service, request } = await setup();
  try {
    await service.startTask(request);
    const policy = await service.policyFor("exec-1");
    assert.equal(policy.grantedCapabilities.includes("*"), true);
    assert.equal(policy.allowedPaths.includes(request.worktreePath), true);
    assert.equal(policy.snapshotId, "pol-1");
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("the acceptance checks of an execution are the ones the task declared", async () => {
  const { repo, service, request } = await setup();
  try {
    const checks = [{ id: "build", name: "Build", command: "npm", args: ["run", "build"] }];
    await service.startTask({ ...request, evaluationChecks: checks });
    assert.deepEqual(service.evaluationChecksFor("exec-1"), checks);
    assert.deepEqual(service.evaluationChecksFor("exec-unknown"), [], "an unknown execution has no checks");
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

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

test("records an evaluation and denies completion while a check fails", async () => {
  const { repo, stores, journal, coordinator, service, request } = await setup();
  try {
    await service.startTask(request);
    const failing = {
      async run() {
        return { code: 1, stdout: "", stderr: "1 test failed", timedOut: false };
      }
    };

    const result = await service.evaluateChange("exec-1", [{
      id: "unit",
      name: "npm test",
      command: "npm",
      args: ["run", "test:unit"]
    }], { runner: failing });

    assert.equal(result.verdict, "failed");
    assert.equal((await stores.evaluations.list()).length, 1);

    const gate = await service.completionGate("exec-1");
    assert.equal(gate.allowed, false);
    assert.match(gate.reason, /failed/);

    const trace = await service.trace("exec-1");
    assert.equal(trace.evaluations.length, 1);

    await coordinator.flush("exec-1");
    assert.ok((await journal.read("exec-1")).some(entry => entry.category === "evaluation.completed"));
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("allows completion once every evaluation check passes", async () => {
  const { repo, service, request } = await setup();
  try {
    await service.startTask(request);
    const passing = {
      async run() {
        return { code: 0, stdout: "ok", stderr: "", timedOut: false };
      }
    };

    await service.evaluateChange("exec-1", [{
      id: "unit",
      name: "npm test",
      command: "npm",
      args: ["run", "test:unit"]
    }], { runner: passing });

    const gate = await service.completionGate("exec-1");
    assert.equal(gate.allowed, true);
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("binds a requested skill and journals the binding", async () => {
  const { repo, journal, coordinator, service, request } = await setup();
  try {
    const result = await service.startTask({ ...request, skills: [{ skillId: "react-dev" }] });

    assert.equal(result.skills.length, 1);
    assert.equal(result.skills[0]!.skillVersion, "1.0.0");

    await coordinator.flush("exec-1");
    const bound = (await journal.read("exec-1")).find(entry => entry.category === "skill.bound");
    assert.equal(bound?.payload["skillId"], "react-dev");
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("delivers the skill body to the runtime and journals the delivery", async () => {
  const { repo, journal, coordinator, service, request, startedInput } = await setup();
  try {
    await service.startTask({ ...request, skills: [{ skillId: "react-dev" }] });

    // The runtime is started with the composed prompt: the user's words first, then the skill body.
    const prompt = startedInput()?.prompt ?? "";
    assert.match(prompt, /^change value to 2/);
    assert.match(prompt, /Prefer function components and hooks\./);
    assert.match(prompt, /react-dev@1\.0\.0/);

    // The prompt snapshot hashes what was delivered, not the prompt before the skills were added.
    const snapshot = await service.trace("exec-1");
    assert.equal(snapshot.promptSnapshots.length, 1);

    await coordinator.flush("exec-1");
    const delivered = (await journal.read("exec-1")).find(entry => entry.category === "skill.delivered");
    assert.equal(delivered?.payload["skillId"], "react-dev");
    assert.equal(delivered?.payload["deliveryMode"], "prompt_inline");
    assert.equal(delivered?.payload["runtimeId"], "hermes-local");
    assert.ok(Number(delivered?.payload["bytes"]) > 0);
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("a task without skills is not given a skill block", async () => {
  const { repo, service, request, startedInput } = await setup();
  try {
    await service.startTask(request);
    assert.equal(startedInput()?.prompt, "change value to 2");
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("refuses a skill whose permissions the policy does not grant", async () => {
  const { repo, service, request } = await setup();
  try {
    await assert.rejects(
      service.startTask({
        ...request,
        policy: { ...request.policy, grantedCapabilities: ["read_file"] },
        skills: [{ skillId: "react-dev" }]
      }),
      (error: unknown) => error instanceof SkillDeniedError
    );
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("refuses a high-risk skill without an explicit approval", async () => {
  const { repo, service, request } = await setup();
  try {
    await assert.rejects(
      service.startTask({ ...request, skills: [{ skillId: "deploy" }] }),
      /requires approval/
    );
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});

test("binds a high-risk skill once it is approved", async () => {
  const { repo, service, request } = await setup();
  try {
    const result = await service.startTask({
      ...request,
      skills: [{ skillId: "deploy" }],
      approvedSkills: ["deploy@1.0.0"]
    });

    assert.equal(result.skills.length, 1);
    assert.equal(result.skills[0]!.skillId, "deploy");
  } finally {
    await rm(repo, { recursive: true, force: true });
  }
});
