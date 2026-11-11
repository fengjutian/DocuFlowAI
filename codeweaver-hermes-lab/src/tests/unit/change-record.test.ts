import test from "node:test";
import assert from "node:assert/strict";
import { buildApplyRecord, buildRevertRecord } from "../../code-engine/change-record.js";

const sha = (char: string) => char.repeat(40);

const approval = {
  approvedBy: "alice",
  reason: "reviewed the diff",
  approvedAt: "2026-11-09T00:00:00.000Z"
};

test("builds a frozen apply record with the snapshot base SHA", () => {
  const record = buildApplyRecord({
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    gitBaseSha: sha("A"),
    diffHash: sha("B"),
    gitCommitSha: sha("C"),
    approval,
    createdAt: "2026-11-09T01:00:00.000Z",
    idFactory: () => "chg_1"
  });

  assert.equal(record.id, "chg_1");
  assert.equal(record.operation, "apply");
  assert.equal(record.gitBaseSha, sha("a"));
  assert.equal(record.gitCommitSha, sha("c"));
  assert.equal(record.approval?.approvedBy, "alice");
  assert.equal(Object.isFrozen(record), true);
  assert.equal(Object.isFrozen(record.approval), true);
  assert.throws(() => {
    (record as unknown as { operation: string }).operation = "revert";
  }, TypeError);
});

test("builds a revert record that points at the change it undoes", () => {
  const applied = buildApplyRecord({
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    gitBaseSha: sha("A"),
    diffHash: sha("B"),
    gitCommitSha: sha("C"),
    createdAt: "2026-11-09T01:00:00.000Z",
    idFactory: () => "chg_apply"
  });

  const revert = buildRevertRecord({
    original: applied,
    diffHash: sha("D"),
    gitCommitSha: sha("E"),
    approval,
    createdAt: "2026-11-09T02:00:00.000Z",
    idFactory: () => "chg_revert"
  });

  assert.equal(revert.operation, "revert");
  assert.equal(revert.revertsChangeRecordId, "chg_apply");
  assert.equal(revert.revertsCommitSha, sha("c"));
  assert.equal(revert.gitBaseSha, sha("c"));
  assert.equal(revert.gitCommitSha, sha("e"));
});

test("refuses to revert a change that never produced a commit", () => {
  const applied = buildApplyRecord({
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    gitBaseSha: sha("A"),
    diffHash: sha("B"),
    createdAt: "2026-11-09T01:00:00.000Z",
    idFactory: () => "chg_apply"
  });

  assert.throws(
    () => buildRevertRecord({ original: applied, diffHash: sha("D"), gitCommitSha: sha("E"), approval }),
    /never produced a commit/
  );
});

test("rejects malformed identifiers instead of recording unverifiable evidence", () => {
  assert.throws(() => buildApplyRecord({
    projectId: "",
    taskId: "task-1",
    executionId: "exec-1",
    gitBaseSha: sha("A"),
    diffHash: sha("B")
  }), /projectId/);

  assert.throws(() => buildApplyRecord({
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    gitBaseSha: "not-a-sha",
    diffHash: sha("B")
  }), /gitBaseSha/);
});
