import test from "node:test";
import assert from "node:assert/strict";
import { buildCheckpoint, planRecovery } from "../../journal/checkpoint.js";
import { InMemoryJournalStore, type JournalEntry } from "../../journal/journal.js";
import { recordAudit, recordRuntimeEvent } from "../../journal/event-journal.js";
import type { AgentRuntimeEvent } from "../../types/adapter.js";

function entry(sequence: number, category: string, payload: Record<string, unknown> = {}): JournalEntry {
  return {
    executionId: "exec-1",
    sequence,
    kind: "runtime.event",
    category,
    recordedAt: "2026-11-09T00:00:00.000Z",
    payload
  };
}

test("summarizes a running execution with the checkpoint fields from design 14.3", () => {
  const checkpoint = buildCheckpoint(
    "exec-1",
    [
      entry(1, "execution.started"),
      entry(2, "message.delta"),
      entry(3, "change.applied", { changeRecordId: "chg_1" }),
      entry(4, "permission.requested", { requestId: "req-1" }),
      entry(5, "permission.resolved", { requestId: "req-1" })
    ],
    {
      context: {
        contextSnapshotId: "ctx-1",
        policySnapshotId: "pol-1",
        gitBaseSha: "a".repeat(40),
        worktreeId: "wt-1",
        externalSessionId: "sess-1"
      },
      now: () => new Date("2026-11-09T01:00:00.000Z")
    }
  );

  assert.equal(checkpoint.lastSequence, 5);
  assert.equal(checkpoint.eventCount, 5);
  assert.equal(checkpoint.terminal, false);
  assert.equal(checkpoint.lastEventType, "permission.resolved");
  assert.deepEqual(checkpoint.appliedChangeRecordIds, ["chg_1"]);
  assert.deepEqual(checkpoint.pendingPermissionRequestIds, []);
  assert.deepEqual(checkpoint.gaps, []);
  assert.equal(checkpoint.gitBaseSha, "a".repeat(40));
  assert.equal(checkpoint.createdAt, "2026-11-09T01:00:00.000Z");
});

test("marks the execution terminal and keeps unresolved permission requests", () => {
  const checkpoint = buildCheckpoint("exec-1", [
    entry(1, "execution.started"),
    entry(2, "permission.requested", { requestId: "req-9" }),
    entry(3, "execution.failed")
  ]);

  assert.equal(checkpoint.terminal, true);
  assert.equal(checkpoint.terminalEventType, "execution.failed");
  assert.deepEqual(checkpoint.pendingPermissionRequestIds, ["req-9"]);
});

test("reports gaps so recovery compensates before resuming", () => {
  const checkpoint = buildCheckpoint("exec-1", [entry(1, "execution.started"), entry(4, "message.delta")]);

  assert.deepEqual(checkpoint.gaps, [2, 3]);
});

test("stops recovery for an execution that already finished", () => {
  const checkpoint = buildCheckpoint("exec-1", [entry(1, "execution.started"), entry(2, "execution.completed")]);
  const plan = planRecovery(checkpoint, { capabilities: { sessionResume: true }, sessionResumable: true });

  assert.equal(plan.action, "stop");
  assert.match(plan.reason, /terminal/);
});

test("compensates missing events before deciding how to continue", () => {
  const checkpoint = buildCheckpoint("exec-1", [entry(1, "execution.started"), entry(5, "message.delta")]);
  const plan = planRecovery(checkpoint, { capabilities: { sessionResume: false } });

  assert.equal(plan.action, "compensate");
  assert.equal(plan.fromSequence, 2);
  assert.match(plan.reason, /missing sequence/);
});

test("resumes only when the runtime advertises and confirms resumability", () => {
  const checkpoint = buildCheckpoint("exec-1", [entry(1, "execution.started"), entry(2, "message.delta")]);

  const resumed = planRecovery(checkpoint, { capabilities: { sessionResume: true }, sessionResumable: true });
  assert.equal(resumed.action, "resume");
  assert.equal(resumed.fromSequence, 3);

  const lost = planRecovery(checkpoint, { capabilities: { sessionResume: true }, sessionResumable: false });
  assert.equal(lost.action, "rebuild");
  assert.match(lost.reason, /session is gone/);

  const unsupported = planRecovery(checkpoint, { capabilities: { sessionResume: false } });
  assert.equal(unsupported.action, "rebuild");
  assert.match(unsupported.reason, /cannot resume/);
});

test("records runtime events and audit facts into one execution timeline", async () => {
  const store = new InMemoryJournalStore();
  const event: AgentRuntimeEvent = {
    eventId: "exec-1:1",
    executionId: "exec-1",
    projectId: "proj-1",
    taskId: "task-1",
    sequence: 1,
    eventType: "execution.started",
    timestamp: "2026-11-09T00:00:00.000Z",
    payload: { externalRunId: "run-1" },
    externalEventId: "sse-1"
  };

  await recordRuntimeEvent(store, event, 1);
  await recordAudit(store, {
    executionId: "exec-1",
    sequence: 2,
    category: "change.applied",
    payload: { changeRecordId: "chg_1" },
    recordedAt: "2026-11-09T00:00:01.000Z"
  });

  const read = await store.read("exec-1");
  assert.deepEqual(read.map(item => [item.sequence, item.kind, item.category]), [
    [1, "runtime.event", "execution.started"],
    [2, "audit", "change.applied"]
  ]);
  assert.equal(read[0]!.payload["externalEventId"], "sse-1");
  assert.equal(read[0]!.payload["externalRunId"], "run-1");

  const checkpoint = buildCheckpoint("exec-1", read);
  assert.deepEqual(checkpoint.appliedChangeRecordIds, ["chg_1"]);
});
