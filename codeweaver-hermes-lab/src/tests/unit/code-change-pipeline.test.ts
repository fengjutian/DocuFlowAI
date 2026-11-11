import test from "node:test";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { CodeChangePipeline } from "../../code-engine/code-change-pipeline.js";
import type { CommandResult, GitRunOptions, GitRunner } from "../../code-engine/git-runner.js";
import { WorktreeManager } from "../../code-engine/worktree-manager.js";
import type { PolicySnapshot } from "../../policy/permission-policy.js";

const baseSha = "a".repeat(40);
const newSha = "b".repeat(40);
const worktree = resolve("worktrees", "task-1");

const patch = [
  "diff --git a/src/index.ts b/src/index.ts",
  "--- a/src/index.ts",
  "+++ b/src/index.ts",
  "@@ -1 +1 @@",
  "-export const value = 1;",
  "+export const value = 2;",
  ""
].join("\n");

const dependencyPatch = [
  "diff --git a/package.json b/package.json",
  "--- a/package.json",
  "+++ b/package.json",
  "@@ -1 +1 @@",
  "-{}",
  "+{\"added\":\"dep\"}",
  ""
].join("\n");

class FakeGit implements GitRunner {
  calls: Array<{ args: string[]; options: GitRunOptions | undefined }> = [];
  committed = false;

  constructor(private readonly handler?: (args: string[]) => CommandResult | undefined) {}

  async run(args: string[], options?: GitRunOptions): Promise<CommandResult> {
    this.calls.push({ args, options });
    const override = this.handler?.(args);
    if (override) return override;
    if (args[0] === "rev-parse" && args[1] === "HEAD") {
      return { code: 0, stdout: `${this.committed ? newSha : baseSha}\n`, stderr: "" };
    }
    if (args[0] === "-c") {
      this.committed = true;
      return { code: 0, stdout: "", stderr: "" };
    }
    return { code: 0, stdout: "", stderr: "" };
  }
}

function snapshot(overrides: Partial<PolicySnapshot> = {}): PolicySnapshot {
  return {
    snapshotId: "snap-1",
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    worktreeId: "wt-1",
    policyVersion: "1.0.0",
    grantedCapabilities: ["*"],
    allowedPaths: [worktree],
    expiresAt: "2099-01-01T00:00:00.000Z",
    ...overrides
  };
}

function request(overrides: Partial<Parameters<CodeChangePipeline["apply"]>[0]> = {}) {
  return {
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    worktreePath: worktree,
    expectedBaseSha: baseSha,
    patch,
    policy: { snapshot: snapshot(), allowedPaths: [worktree] },
    commit: { message: "apply codeweaver change" },
    ...overrides
  };
}

function pipelineWith(git: GitRunner): CodeChangePipeline {
  return new CodeChangePipeline(git, new WorktreeManager(git, resolve("worktrees")), {
    now: () => new Date("2026-11-09T00:00:00.000Z"),
    recordIdFactory: () => "chg_test"
  });
}

test("applies a clean patch, commits it, and records the change", async () => {
  const git = new FakeGit();
  const outcome = await pipelineWith(git).apply(request());

  assert.equal(outcome.status, "applied");
  if (outcome.status !== "applied") return;
  assert.equal(outcome.commitSha, newSha);
  assert.equal(outcome.record.operation, "apply");
  assert.equal(outcome.record.id, "chg_test");
  assert.equal(outcome.record.gitBaseSha, baseSha);
  assert.equal(outcome.record.gitCommitSha, newSha);
  assert.equal(outcome.record.diffHash.length, 64);

  const applyCalls = git.calls.filter(call => call.args[0] === "apply");
  assert.equal(applyCalls.length, 2);
  assert.equal(applyCalls[0]!.args[1], "--check");
  assert.equal(applyCalls[0]!.options?.stdin, patch);
  assert.equal(applyCalls[1]!.options?.stdin, patch);
});

test("checks the patch for applicability before writing anything", async () => {
  const git = new FakeGit();
  await pipelineWith(git).apply(request());

  const applyIndexes = git.calls
    .map((call, index) => ({ call, index }))
    .filter(({ call }) => call.args[0] === "apply");
  assert.equal(applyIndexes.length, 2);
  assert.equal(applyIndexes[0]!.call.args[1], "--check");
  assert.equal(applyIndexes[1]!.call.args[1], "--whitespace=nowarn");
});

test("refuses to apply when the worktree drifted from the pinned base SHA", async () => {
  const git = new FakeGit(args =>
    args[0] === "rev-parse" && args[1] === "HEAD"
      ? { code: 0, stdout: `${"c".repeat(40)}\n`, stderr: "" }
      : undefined
  );
  const outcome = await pipelineWith(git).apply(request());

  assert.equal(outcome.status, "denied");
  if (outcome.status !== "denied") return;
  assert.match(outcome.reasons[0]!, /does not match the pinned base SHA/);
  assert.equal(git.calls.some(call => call.args[0] === "apply"), false);
});

test("denies a patch whose files fall outside the authorized scope", async () => {
  const git = new FakeGit();
  const outcome = await pipelineWith(git).apply(
    request({ policy: { snapshot: snapshot({ allowedPaths: [resolve("other")] }), allowedPaths: [resolve("other")] } })
  );

  assert.equal(outcome.status, "denied");
  if (outcome.status !== "denied") return;
  assert.match(outcome.reasons[0]!, /outside the authorized worktree scope/);
  assert.equal(git.calls.some(call => call.args[0] === "apply"), false);
});

test("denies a structurally unsafe patch without touching git", async () => {
  const git = new FakeGit();
  const escapePatch = "diff --git a/../outside.txt b/../outside.txt\n--- a/../outside.txt\n+++ b/../outside.txt\n@@ -1 +1 @@\n-a\n+b\n";
  const outcome = await pipelineWith(git).apply(request({ patch: escapePatch }));

  assert.equal(outcome.status, "denied");
  assert.equal(git.calls.some(call => call.args[0] === "apply"), false);
});

test("waits for approval on a risky patch instead of applying it", async () => {
  const git = new FakeGit();
  const outcome = await pipelineWith(git).apply(request({ patch: dependencyPatch }));

  assert.equal(outcome.status, "awaiting_approval");
  if (outcome.status !== "awaiting_approval") return;
  assert.ok(outcome.reasons.some(reason => /package\.json/.test(reason)));
  assert.equal(git.calls.some(call => call.args[0] === "apply"), false);
});

test("applies a risky patch once an approval record is supplied", async () => {
  const git = new FakeGit();
  const outcome = await pipelineWith(git).apply(request({
    patch: dependencyPatch,
    approval: { approvedBy: "alice", reason: "dependency reviewed", approvedAt: "2026-11-09T00:00:00.000Z" }
  }));

  assert.equal(outcome.status, "applied");
  if (outcome.status !== "applied") return;
  assert.equal(outcome.record.approval?.approvedBy, "alice");
});

test("denies when the policy snapshot has expired", async () => {
  const git = new FakeGit();
  const outcome = await pipelineWith(git).apply(
    request({ policy: { snapshot: snapshot({ expiresAt: "2000-01-01T00:00:00.000Z" }), allowedPaths: [worktree] } })
  );

  assert.equal(outcome.status, "denied");
  if (outcome.status !== "denied") return;
  assert.match(outcome.reasons[0]!, /expired/);
  assert.equal(git.calls.some(call => call.args[0] === "apply"), false);
});

test("denies a patch that git cannot apply cleanly", async () => {
  const git = new FakeGit(args =>
    args[0] === "apply" && args[1] === "--check"
      ? { code: 1, stdout: "", stderr: "error: patch does not apply" }
      : undefined
  );
  const outcome = await pipelineWith(git).apply(request());

  assert.equal(outcome.status, "denied");
  if (outcome.status !== "denied") return;
  assert.match(outcome.reasons[0]!, /does not apply cleanly/);
  assert.equal(git.calls.filter(call => call.args[0] === "apply").length, 1);
});
