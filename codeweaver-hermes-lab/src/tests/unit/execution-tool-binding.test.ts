import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { proposeChange } from "../../code-engine/diff-analysis.js";
import type { EvaluationCheckSpec, EvaluationResult } from "../../evaluation/evaluation.js";
import type { PolicySnapshot } from "../../policy/permission-policy.js";
import type { AgentTaskInput } from "../../types/adapter.js";
import {
  CodeWeaverToolBinder,
  type ExecutionToolService
} from "../../tools/execution-tool-binding.js";

/**
 * The binder is the only bridge from a runtime's tool call to the Code Engine and the Evaluation
 * Engine. These tests check the bridge: the execution the tools are bound to, the checks they run,
 * and the fact that a tool the execution may not use is not offered at all.
 */

function taskInput(overrides: Partial<AgentTaskInput> = {}): AgentTaskInput {
  return {
    projectId: "proj-1",
    taskId: "task-1",
    runId: "run-1",
    prompt: "do the thing",
    contextSnapshotId: "ctx-1",
    policySnapshotId: "pol-1",
    worktreeId: "wt-1",
    workingDirectory: process.cwd(),
    allowedTools: ["read_file", "request_diff", "request_apply_diff", "evaluate_changes"],
    timeoutSeconds: 60,
    correlationId: "corr-1",
    ...overrides
  };
}

function policy(): PolicySnapshot {
  return {
    snapshotId: "snap-1",
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    worktreeId: "wt-1",
    policyVersion: "1.0.0",
    grantedCapabilities: ["*"],
    allowedPaths: [process.cwd()],
    expiresAt: "2099-01-01T00:00:00.000Z"
  };
}

interface Recorder {
  service: ExecutionToolService;
  applied: Array<{ executionId: string; patch: string; commitMessage?: string }>;
  evaluated: Array<{ executionId: string; checks: readonly EvaluationCheckSpec[] }>;
  policyCalls: number;
}

function recorder(checks: EvaluationCheckSpec[] = []): Recorder {
  const applied: Recorder["applied"] = [];
  const evaluated: Recorder["evaluated"] = [];
  let policyCalls = 0;
  const service: ExecutionToolService = {
    async policyFor() {
      policyCalls += 1;
      return policy();
    },
    async worktreeScope(worktreePath) {
      return { own: worktreePath, worktrees: [worktreePath] };
    },
    evaluationChecksFor: () => checks,
    async applyProposedChange(executionId, patch, options) {
      applied.push({ executionId, patch, ...(options?.commitMessage ? { commitMessage: options.commitMessage } : {}) });
      const proposal = proposeChange(patch);
      return {
        status: "applied",
        proposal,
        record: {
          id: "chg-1",
          projectId: "proj-1",
          taskId: "task-1",
          executionId,
          operation: "apply",
          gitBaseSha: "a".repeat(40),
          diffHash: proposal.diffHash,
          createdAt: "2026-11-09T00:00:00.000Z"
        }
      };
    },
    async evaluateChange(executionId, selected): Promise<EvaluationResult> {
      evaluated.push({ executionId, checks: selected });
      return {
        id: "eval-1",
        executionId,
        checks: selected.map(check => ({
          id: check.id,
          name: check.name,
          verdict: "passed",
          exitCode: 0,
          durationMs: 1,
          output: ""
        })),
        verdict: "passed",
        startedAt: "2026-11-09T00:00:00.000Z",
        finishedAt: "2026-11-09T00:00:01.000Z",
        summary: `${selected.length}/${selected.length} checks passed`
      };
    }
  };
  return {
    service,
    applied,
    evaluated,
    get policyCalls() {
      return policyCalls;
    }
  };
}

const PATCH = [
  "diff --git a/app.ts b/app.ts",
  "--- a/app.ts",
  "+++ b/app.ts",
  "@@ -1 +1 @@",
  "-const a = 1;",
  "+const a = 2;",
  ""
].join("\n");

test("the bound tool set contains only the tools the execution may use", () => {
  const rec = recorder();
  const binder = new CodeWeaverToolBinder({
    service: rec.service,
    propose: patch => proposeChange(patch)
  });

  const tools = binder.bind(taskInput({ allowedTools: ["read_file", "request_diff"] }), "exec-1");
  assert.deepEqual(tools.map(tool => tool.name), ["read_file", "request_diff"]);
});

test("request_apply_diff reaches applyProposedChange for its own execution", async () => {
  const rec = recorder();
  const binder = new CodeWeaverToolBinder({
    service: rec.service,
    propose: patch => proposeChange(patch)
  });
  const tools = binder.bind(taskInput(), "exec-9");
  const apply = tools.find(tool => tool.name === "request_apply_diff")!;

  const result = await apply.execute("call-1", { patch: PATCH, commitMessage: "fix" });
  assert.equal(result.details["applied"], true);
  assert.equal(rec.applied.length, 1);
  assert.equal(rec.applied[0]!.executionId, "exec-9", "the change is attributed to the execution");
  assert.equal(rec.applied[0]!.commitMessage, "fix");
});

test("evaluate_changes runs the checks the task declared", async () => {
  const checks: EvaluationCheckSpec[] = [
    { id: "build", name: "Build", command: "npm", args: ["run", "build"] }
  ];
  const rec = recorder(checks);
  const binder = new CodeWeaverToolBinder({
    service: rec.service,
    propose: patch => proposeChange(patch)
  });
  const tools = binder.bind(taskInput(), "exec-9");
  const evaluate = tools.find(tool => tool.name === "evaluate_changes")!;

  const result = await evaluate.execute("call-1", {});
  assert.equal(result.details["verdict"], "passed");
  assert.deepEqual(rec.evaluated[0]!.checks.map(check => check.id), ["build"]);
  assert.equal(rec.evaluated[0]!.executionId, "exec-9");
});

test("the policy snapshot is read when the tool runs, not when it is bound", async () => {
  const rec = recorder();
  const binder = new CodeWeaverToolBinder({
    service: rec.service,
    propose: patch => proposeChange(patch)
  });
  const dir = await mkdtemp(join(tmpdir(), "codeweaver-binding-"));
  await writeFile(join(dir, "a.txt"), "x", "utf8");
  const tools = binder.bind(taskInput({ workingDirectory: dir }), "exec-9");
  assert.equal(rec.policyCalls, 0, "binding does not read the policy");

  const read = tools.find(tool => tool.name === "read_file")!;
  await read.execute("call-1", { path: "a.txt" });
  assert.ok(rec.policyCalls > 0, "the tool asked for the policy when it ran");
});

test("the bound worktree is the execution's worktree", async () => {
  const rec = recorder();
  const binder = new CodeWeaverToolBinder({
    service: rec.service,
    propose: patch => proposeChange(patch)
  });
  const dir = await mkdtemp(join(tmpdir(), "codeweaver-binding-"));
  const tools = binder.bind(taskInput({ workingDirectory: dir }), "exec-9");
  const read = tools.find(tool => tool.name === "read_file")!;

  const outside = await read.execute("call-1", { path: join(dir, "..", "escape.txt") });
  assert.equal(outside.details["denied"], true, "the tool is bound to the execution's worktree");
});
