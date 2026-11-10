import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { proposeChange, type ChangeProposal } from "../../code-engine/diff-analysis.js";
import type { ApplyOutcome } from "../../code-engine/code-change-pipeline.js";
import type { EvaluationCheckSpec, EvaluationResult } from "../../evaluation/evaluation.js";
import type { PolicySnapshot } from "../../policy/permission-policy.js";
import { createDocuFlowTools, type DocuFlowToolContext } from "../../tools/docuflow-tools.js";

/**
 * The tool surface is the boundary between a runtime and the workspace. These tests check the
 * boundary itself: what a runtime may read, what it may write, and what it is told when the
 * policy says no. A mock here is the right shape, because the subject is DocuFlow's own rule
 * code — the runtime is not what is under test.
 */

async function worktree(): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "docuflow-tools-"));
  await writeFile(join(dir, "readme.md"), "hello\n", "utf8");
  return dir;
}

function policy(overrides: Partial<PolicySnapshot> = {}): PolicySnapshot {
  return {
    snapshotId: "snap-1",
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    worktreeId: "wt-1",
    policyVersion: "1.0.0",
    grantedCapabilities: ["*"],
    allowedPaths: [],
    expiresAt: "2099-01-01T00:00:00.000Z",
    ...overrides
  };
}

function context(overrides: Partial<DocuFlowToolContext> = {}): DocuFlowToolContext {
  return {
    worktreePath: process.cwd(),
    policy: () => policy(),
    propose: patch => proposeChange(patch),
    apply: async () => ({ status: "denied", proposal: proposeChange(""), reasons: ["not stubbed"] }),
    evaluationChecks: () => [],
    evaluate: async () => {
      throw new Error("not stubbed");
    },
    ...overrides
  };
}

function tool(name: string, ctx: DocuFlowToolContext) {
  const found = createDocuFlowTools(ctx).find(item => item.name === name);
  assert.ok(found, `tool ${name} exists`);
  return found;
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

test("read_file returns the content of a file inside the worktree", async () => {
  const dir = await worktree();
  const read = tool("read_file", context({ worktreePath: dir }));

  const result = await read.execute("call-1", { path: "readme.md" });
  assert.equal(result.content[0]!.text, "hello\n");
  assert.equal(result.details["bytes"], 6);
});

test("read_file denies a path outside the worktree", async () => {
  const dir = await worktree();
  const read = tool("read_file", context({ worktreePath: dir }));

  const absolute = await read.execute("call-1", { path: join(dir, "..", "..", "secrets.txt") });
  assert.equal(absolute.details["denied"], true);
  assert.equal(absolute.details["reason"], "outside_worktree");

  const traversal = await read.execute("call-2", { path: "../../secrets.txt" });
  assert.equal(traversal.details["denied"], true, "a traversal is resolved before the check");
});

test("read_file denies what the policy does not grant", async () => {
  const dir = await worktree();
  const read = tool(
    "read_file",
    context({ worktreePath: dir, policy: () => policy({ grantedCapabilities: ["modify_worktree"] }) })
  );

  const result = await read.execute("call-1", { path: "readme.md" });
  assert.equal(result.details["denied"], true);
  assert.match(String(result.details["reason"]), /not granted/);
});

test("read_file denies when the policy snapshot expired", async () => {
  const dir = await worktree();
  const read = tool(
    "read_file",
    context({ worktreePath: dir, policy: () => policy({ expiresAt: "2020-01-01T00:00:00.000Z" }) })
  );

  const result = await read.execute("call-1", { path: "readme.md" });
  assert.equal(result.details["denied"], true);
  assert.match(String(result.details["reason"]), /expired/);
});

test("request_diff inspects a patch without writing anything", async () => {
  let applied = false;
  const diff = tool(
    "request_diff",
    context({
      apply: async () => {
        applied = true;
        throw new Error("must not be called");
      }
    })
  );

  const result = await diff.execute("call-1", { patch: PATCH });
  assert.equal(applied, false, "no write happened");
  assert.deepEqual(result.details["files"], ["app.ts"]);
  assert.equal(result.details["requiresApproval"], false);
});

test("request_apply_diff reports an applied change", async () => {
  const proposal = proposeChange(PATCH);
  const outcome: ApplyOutcome = {
    status: "applied",
    proposal,
    record: {
      id: "chg-1",
      projectId: "proj-1",
      taskId: "task-1",
      executionId: "exec-1",
      operation: "apply",
      gitBaseSha: "a".repeat(40),
      diffHash: proposal.diffHash,
      createdAt: "2026-11-09T00:00:00.000Z"
    },
    commitSha: "b".repeat(40)
  };
  const apply = tool("request_apply_diff", context({ apply: async () => outcome }));

  const result = await apply.execute("call-1", { patch: PATCH });
  assert.equal(result.details["applied"], true);
  assert.equal(result.details["changeRecordId"], "chg-1");
  assert.equal(result.details["commitSha"], "b".repeat(40));
});

test("request_apply_diff reports a change that waits for a human", async () => {
  const proposal = proposeChange(PATCH);
  const apply = tool(
    "request_apply_diff",
    context({
      apply: async () => ({ status: "awaiting_approval", proposal, reasons: ["modifies package.json"] })
    })
  );

  const result = await apply.execute("call-1", { patch: PATCH });
  assert.equal(result.details["applied"], false);
  assert.equal(result.details["awaitingApproval"], true);
});

test("request_apply_diff reports a denied change", async () => {
  const proposal = proposeChange(PATCH);
  const apply = tool(
    "request_apply_diff",
    context({
      apply: async () => ({ status: "denied", proposal, reasons: ["path escapes the worktree"] })
    })
  );

  const result = await apply.execute("call-1", { patch: PATCH });
  assert.equal(result.details["denied"], true);
  assert.deepEqual(result.details["reasons"], ["path escapes the worktree"]);
});

test("evaluate_changes runs the declared checks", async () => {
  const checks: EvaluationCheckSpec[] = [
    { id: "build", name: "Build", command: "npm", args: ["run", "build"] },
    { id: "lint", name: "Lint", command: "npm", args: ["run", "lint"] }
  ];
  let ran: readonly EvaluationCheckSpec[] = [];
  const evaluate = tool(
    "evaluate_changes",
    context({
      evaluationChecks: () => checks,
      evaluate: async selected => {
        ran = selected;
        const result: EvaluationResult = {
          id: "eval-1",
          executionId: "exec-1",
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
          summary: "2/2 checks passed"
        };
        return result;
      }
    })
  );

  const result = await evaluate.execute("call-1", {});
  assert.equal(result.details["verdict"], "passed");
  assert.equal(ran.length, 2, "every declared check ran");
});

test("evaluate_changes may narrow the checks but not extend them", async () => {
  const checks: EvaluationCheckSpec[] = [
    { id: "build", name: "Build", command: "npm", args: ["run", "build"] },
    { id: "lint", name: "Lint", command: "npm", args: ["run", "lint"] }
  ];
  let ran: readonly EvaluationCheckSpec[] = [];
  const evaluate = tool(
    "evaluate_changes",
    context({
      evaluationChecks: () => checks,
      evaluate: async selected => {
        ran = selected;
        throw new Error("should not run");
      }
    })
  );

  const unknown = await evaluate.execute("call-1", { checkIds: ["rm-rf"] });
  assert.equal(unknown.details["denied"], true);
  assert.equal(unknown.details["reason"], "unknown_check");

  await evaluate.execute("call-2", { checkIds: ["build"] }).catch(() => undefined);
  assert.deepEqual(ran.map(check => check.id), ["build"]);
});

test("evaluate_changes without declared checks reports no evidence", async () => {
  const evaluate = tool("evaluate_changes", context({ evaluationChecks: () => [] }));

  const result = await evaluate.execute("call-1", {});
  assert.equal(result.details["verdict"], "blocked");
  assert.equal(result.details["reason"], "no_checks_declared");
});
