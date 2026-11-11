import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApplyRecord, type ChangeRecord } from "../../code-engine/change-record.js";
import { ImmutableRecordStore } from "../../persistence/record-store.js";
import type { EvaluationResult } from "../../evaluation/evaluation.js";
import {
  assertContextSnapshot,
  assertPolicySnapshot,
  assertPromptSnapshot,
  type ContextSnapshotRecord,
  type PolicySnapshotRecord,
  type PromptSnapshotRecord
} from "../../persistence/records.js";
import { TraceabilityQuery } from "../../persistence/traceability.js";

const commitSha = "c".repeat(40);

async function setup() {
  const root = await mkdtemp(join(tmpdir(), "codeweaver-trace-"));
  const stores = {
    changeRecords: new ImmutableRecordStore<ChangeRecord>(join(root, "changes"), "changes"),
    contextSnapshots: new ImmutableRecordStore<ContextSnapshotRecord>(root, "contexts", assertContextSnapshot),
    promptSnapshots: new ImmutableRecordStore<PromptSnapshotRecord>(root, "prompts", assertPromptSnapshot),
    policySnapshots: new ImmutableRecordStore<PolicySnapshotRecord>(root, "policies", assertPolicySnapshot),
    evaluations: new ImmutableRecordStore<EvaluationResult>(root, "evaluations")
  };

  await stores.contextSnapshots.put({
    id: "ctx-1",
    projectId: "proj-1",
    taskId: "task-1",
    gitBaseSha: "a".repeat(40),
    documentVersions: { "spec.md": "3" },
    architectureVersion: "1.0",
    selectedFiles: ["src/index.ts"],
    contextHash: "d".repeat(64),
    createdAt: "2026-11-09T00:00:00.000Z"
  });

  await stores.promptSnapshots.put({
    id: "prm-1",
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    templateName: "coding-agent",
    templateVersion: "1.4.2",
    templateContentHash: "e".repeat(64),
    renderedPromptHash: "f".repeat(64),
    contextSnapshotId: "ctx-1",
    modelConfig: { model: "hermes-default", temperature: 0 },
    createdAt: "2026-11-09T00:00:00.000Z"
  });

  await stores.policySnapshots.put({
    id: "pol-1",
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    policyVersion: "1.0.0",
    grantedCapabilities: ["read_file"],
    deniedCapabilities: ["access_production_data"],
    allowedPaths: ["/tmp/wt-1"],
    expiresAt: "2099-01-01T00:00:00.000Z",
    createdAt: "2026-11-09T00:00:00.000Z"
  });

  await stores.changeRecords.put(buildApplyRecord({
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    gitBaseSha: "a".repeat(40),
    diffHash: "1".repeat(64),
    gitCommitSha: commitSha,
    createdAt: "2026-11-09T00:00:02.000Z",
    idFactory: () => "chg-1"
  }));

  return { root, stores, query: new TraceabilityQuery(stores) };
}

test("returns the full trace for an execution, resolving context through the prompt snapshot", async () => {
  const { root, query } = await setup();
  try {
    const trace = await query.byExecution("exec-1");

    assert.equal(trace.executionId, "exec-1");
    assert.deepEqual(trace.changeRecords.map(record => record.id), ["chg-1"]);
    assert.deepEqual(trace.promptSnapshots.map(record => record.id), ["prm-1"]);
    assert.deepEqual(trace.policySnapshots.map(record => record.id), ["pol-1"]);
    assert.deepEqual(trace.contextSnapshots.map(record => record.id), ["ctx-1"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("finds the agent run from a git commit", async () => {
  const { root, query } = await setup();
  try {
    const found = await query.byCommit(commitSha.toUpperCase());

    assert.equal(found?.changeRecord.id, "chg-1");
    assert.equal(found?.trace.executionId, "exec-1");
    assert.deepEqual(found?.trace.contextSnapshots.map(record => record.id), ["ctx-1"]);
    assert.equal(await query.byCommit("9".repeat(40)), undefined);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("groups executions by task and returns nothing for an unknown task", async () => {
  const { root, query } = await setup();
  try {
    const traces = await query.byTask("task-1");
    assert.deepEqual(traces.map(trace => trace.executionId), ["exec-1"]);

    assert.deepEqual(await query.byTask("task-unknown"), []);
    const empty = await query.byExecution("exec-unknown");
    assert.deepEqual(empty, {
      executionId: "exec-unknown",
      changeRecords: [],
      contextSnapshots: [],
      promptSnapshots: [],
      policySnapshots: [],
      evaluations: []
    });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
