import test from "node:test";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import {
  evaluatePermission,
  isWithin,
  type PolicySnapshot
} from "../../policy/permission-policy.js";

const now = new Date("2026-11-09T00:00:00.000Z");
const worktree = resolve("worktrees", "task-1");
const inside = resolve(worktree, "src", "index.ts");
const outside = resolve("somewhere-else", "secret.txt");

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
    expiresAt: "2026-11-10T00:00:00.000Z",
    ...overrides
  };
}

test("allows read-only P0 operations by default", () => {
  const result = evaluatePermission({ operation: "read_file", paths: [inside] }, snapshot(), now);

  assert.equal(result.decision, "allow");
  assert.equal(result.level, "P0");
});

test("denies everything once the policy snapshot has expired", () => {
  const result = evaluatePermission(
    { operation: "read_file" },
    snapshot({ expiresAt: "2026-11-08T00:00:00.000Z" }),
    now
  );

  assert.equal(result.decision, "deny");
  assert.match(result.reason, /expired/);
});

test("denies unknown operations instead of defaulting to allow", () => {
  const result = evaluatePermission({ operation: "exfiltrate_repo" }, snapshot(), now);

  assert.equal(result.decision, "deny");
  assert.equal(result.level, "unknown");
  assert.match(result.reason, /Unknown operation/);
});

test("denies an operation the snapshot does not grant", () => {
  const result = evaluatePermission(
    { operation: "delete_file", paths: [inside] },
    snapshot({ grantedCapabilities: ["read_file"] }),
    now
  );

  assert.equal(result.decision, "deny");
  assert.match(result.reason, /not granted/);
});

test("allows diff generation and scoped worktree writes", () => {
  assert.equal(evaluatePermission({ operation: "generate_diff" }, snapshot(), now).decision, "allow");

  const write = evaluatePermission(
    { operation: "modify_worktree", paths: [inside], worktreePath: worktree },
    snapshot(),
    now
  );
  assert.equal(write.decision, "allow");
  assert.equal(write.level, "P1");
});

test("denies a worktree write outside the authorized paths", () => {
  const result = evaluatePermission(
    { operation: "modify_worktree", paths: [outside], worktreePath: worktree },
    snapshot(),
    now
  );

  assert.equal(result.decision, "deny");
  assert.match(result.reason, /outside every authorized path/);
});

test("denies path traversal even when the prefix matches", () => {
  const traversal = resolve(worktree, "..", "..", "etc", "passwd");
  const result = evaluatePermission(
    { operation: "modify_worktree", paths: [traversal], worktreePath: worktree },
    snapshot(),
    now
  );

  assert.equal(result.decision, "deny");
});

test("requires approval when a worktree write names no paths", () => {
  const result = evaluatePermission({ operation: "modify_worktree" }, snapshot(), now);

  assert.equal(result.decision, "require_approval");
  assert.equal(result.requiresApproval, true);
});

test("requires human approval for P2 operations", () => {
  const result = evaluatePermission({ operation: "delete_file", paths: [inside] }, snapshot(), now);

  assert.equal(result.decision, "require_approval");
  assert.equal(result.level, "P2");
});

test("honors a policy-level authorization for P2", () => {
  const result = evaluatePermission(
    { operation: "install_dependency" },
    snapshot({ autoApprovedOperations: ["install_dependency"] }),
    now
  );

  assert.equal(result.decision, "allow");
  assert.match(result.reason, /pre-authorized/);
});

test("requires approval for release actions and denies production data access", () => {
  assert.equal(evaluatePermission({ operation: "git_merge" }, snapshot(), now).decision, "require_approval");
  assert.equal(evaluatePermission({ operation: "deploy_production" }, snapshot(), now).decision, "require_approval");

  const production = evaluatePermission({ operation: "access_production_data" }, snapshot(), now);
  assert.equal(production.decision, "deny");
  assert.match(production.reason, /Production data access/);
});

test("treats a wildcard grant as granting every known operation", () => {
  const result = evaluatePermission(
    { operation: "search_code" },
    snapshot({ grantedCapabilities: ["*"] }),
    now
  );

  assert.equal(result.decision, "allow");
});

test("isWithin accepts the parent itself and rejects siblings", () => {
  assert.equal(isWithin(worktree, worktree), true);
  assert.equal(isWithin(inside, worktree), true);
  assert.equal(isWithin(outside, worktree), false);
  assert.equal(isWithin(resolve(worktree, "..", "task-2"), worktree), false);
});
