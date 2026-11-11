import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApplyRecord, type ChangeRecord } from "../../code-engine/change-record.js";
import { InMemoryJournalStore, type JournalEntry } from "../../journal/journal.js";
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
  const journal = new InMemoryJournalStore();
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

  return { root, stores, journal, query: new TraceabilityQuery(stores, journal) };
}

/** The timeline of a run that read a file, applied a change and had it evaluated. */
async function journalOfARun(journal: InMemoryJournalStore): Promise<void> {
  const entries: JournalEntry[] = [
    audit(1, "execution.input", { projectId: "proj-1", taskId: "task-1" }),
    audit(2, "execution.runtime", { runtimeId: "pi-local" }),
    runtime(3, "execution.started", { externalRunId: "run-42" }),
    runtime(4, "tool.started", { toolCallId: "call-1", toolName: "read_file", args: { path: "a.ts" } }),
    runtime(5, "tool.completed", { toolCallId: "call-1", toolName: "read_file" }),
    runtime(6, "permission.requested", { requestId: "perm-1", tool: "request_apply_diff", toolCallId: "call-2" }),
    runtime(7, "permission.resolved", {
      requestId: "perm-1",
      tool: "request_apply_diff",
      toolCallId: "call-2",
      decision: "approve"
    }),
    runtime(8, "tool.started", { toolCallId: "call-2", toolName: "request_apply_diff" }),
    runtime(9, "tool.completed", { toolCallId: "call-2", toolName: "request_apply_diff" }),
    audit(10, "change.applied", { changeRecordId: "chg-1", gitCommitSha: commitSha })
  ];
  for (const entry of entries) await journal.append(entry);
}

function audit(sequence: number, category: string, payload: Record<string, unknown>): JournalEntry {
  return {
    executionId: "exec-1",
    sequence,
    kind: "audit",
    category,
    recordedAt: "2026-11-09T00:00:00.000Z",
    payload
  };
}

function runtime(sequence: number, category: string, payload: Record<string, unknown>): JournalEntry {
  return {
    executionId: "exec-1",
    sequence,
    eventSequence: sequence,
    kind: "runtime.event",
    category,
    recordedAt: "2026-11-09T00:00:00.000Z",
    payload
  };
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
      taskIds: [],
      toolCalls: [],
      permissions: [],
      gitCommits: [],
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

test("reads the task, the run and the tool calls off the timeline", async () => {
  const { root, stores, journal, query } = await setup();
  try {
    await journalOfARun(journal);
    const trace = await query.byExecution("exec-1");

    assert.deepEqual(trace.taskIds, ["task-1"]);
    assert.equal(trace.runtimeId, "pi-local");
    assert.equal(trace.externalRunId, "run-42");
    assert.deepEqual(trace.gitCommits, [commitSha]);
    assert.deepEqual(
      trace.toolCalls.map(call => [call.toolName, call.status]),
      [
        ["read_file", "completed"],
        ["request_apply_diff", "completed"]
      ]
    );
    // Design 15 asks what a tool call was answered with; the decision travels with the call.
    assert.equal(trace.toolCalls[1]?.permission?.decision, "approve");
    assert.equal(trace.toolCalls[0]?.permission, undefined, "an ungated call has no decision to report");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("finds an execution by its task even when no record store links them yet", async () => {
  const { root, journal, query } = await setup();
  try {
    await journal.append(audit(1, "execution.input", { projectId: "proj-1", taskId: "task-2" }));

    const traces = await query.byTask("task-2");
    assert.deepEqual(traces.map(trace => trace.executionId), ["exec-1"]);
    assert.deepEqual(traces[0]?.taskIds, ["task-2"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("keeps a decision that no tool call can be paired with", async () => {
  const { root, journal, query } = await setup();
  try {
    await journal.append(runtime(1, "permission.requested", { requestId: "perm-9", tool: "write_file" }));
    await journal.append(runtime(2, "permission.resolved", { requestId: "perm-9", tool: "write_file", decision: "deny" }));

    const trace = await query.byExecution("exec-1");
    assert.equal(trace.permissions.length, 1);
    assert.equal(trace.permissions[0]?.decision, "deny");
    assert.equal(trace.toolCalls.length, 0, "a denied call that never ran is not a tool call");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
