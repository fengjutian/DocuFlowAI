import test from "node:test";
import assert from "node:assert/strict";
import { InMemoryJournalStore, type JournalEntry } from "../../journal/journal.js";
import { recordAudit, recordRuntimeEvent } from "../../journal/event-journal.js";
import {
  AuditTimelineService,
  buildAuditTimeline,
  mergeAuditStreams
} from "../../persistence/audit-timeline.js";

/**
 * The audit timeline has to be able to catch the things it exists for.
 *
 * Most of these cases are the ones that have to fail: a timeline that drops an event, one that
 * invents one, one that reorders an execution because a timestamp disagreed, one that hides a hole
 * in the journal, one that shows a delegated child as unrelated to the task that caused it.
 */

function entry(
  executionId: string,
  sequence: number,
  category: string,
  recordedAt: string,
  payload: Record<string, unknown> = {},
  kind: JournalEntry["kind"] = "audit"
): JournalEntry {
  return { executionId, sequence, kind, category, recordedAt, payload };
}

function parentRun(): JournalEntry[] {
  return [
    entry("exec-parent", 1, "execution.input", "2026-11-12T00:00:00.000Z", { taskId: "task-1" }),
    entry("exec-parent", 2, "execution.runtime", "2026-11-12T00:00:01.000Z", { runtimeId: "pi-local" }),
    entry("exec-parent", 3, "execution.started", "2026-11-12T00:00:02.000Z", {}, "runtime.event"),
    entry("exec-parent", 4, "tool.started", "2026-11-12T00:00:03.000Z", { toolName: "read_file" }, "runtime.event"),
    entry("exec-parent", 5, "tool.completed", "2026-11-12T00:00:04.000Z", { toolName: "read_file" }, "runtime.event"),
    entry("exec-parent", 6, "change.applied", "2026-11-12T00:00:05.000Z", { gitCommitSha: "c".repeat(40) }),
    entry("exec-parent", 7, "evaluation.completed", "2026-11-12T00:00:06.000Z", { verdict: "passed" }),
    entry("exec-parent", 8, "execution.completed", "2026-11-12T00:00:07.000Z", {}, "runtime.event")
  ];
}

function childRun(): JournalEntry[] {
  return [
    entry("exec-child", 1, "execution.input", "2026-11-12T00:00:08.000Z", { taskId: "task-child" }),
    entry("exec-child", 2, "subagent.parent", "2026-11-12T00:00:09.000Z", { parentExecutionId: "exec-parent" }),
    entry("exec-child", 3, "execution.completed", "2026-11-12T00:00:10.000Z", {}, "runtime.event")
  ];
}

test("the timeline holds every recorded event of every execution, once each", () => {
  const timeline = buildAuditTimeline([...parentRun(), ...childRun()]);
  assert.equal(timeline.items.length, 11);
  assert.equal(
    timeline.items.filter(item => item.executionId === "exec-parent").length,
    8
  );
  assert.equal(
    timeline.items.filter(item => item.executionId === "exec-child").length,
    3
  );
  assert.deepEqual(
    timeline.items.filter(item => item.executionId === "exec-parent").map(item => item.sequence),
    [1, 2, 3, 4, 5, 6, 7, 8]
  );
});

test("the merge interleaves two executions by the recorded time", () => {
  const merged = mergeAuditStreams([
    [
      entry("exec-a", 1, "one", "2026-11-12T00:00:01.000Z"),
      entry("exec-a", 2, "two", "2026-11-12T00:00:03.000Z")
    ],
    [
      entry("exec-b", 1, "three", "2026-11-12T00:00:02.000Z"),
      entry("exec-b", 2, "four", "2026-11-12T00:00:04.000Z")
    ]
  ]);
  assert.deepEqual(
    merged.map(item => `${item.executionId}:${item.sequence}`),
    ["exec-a:1", "exec-b:1", "exec-a:2", "exec-b:2"]
  );
});

test("an execution's own order is never broken by a disagreeing timestamp", () => {
  // Two clocks: the runtime's timestamps run backwards against the journal order.
  const timeline = buildAuditTimeline([
    entry("exec-parent", 1, "execution.input", "2026-11-12T00:00:05.000Z", { taskId: "task-1" }),
    entry("exec-parent", 2, "tool.started", "2026-11-12T00:00:01.000Z", {}, "runtime.event")
  ]);
  assert.deepEqual(timeline.items.map(item => item.sequence), [1, 2]);
  const execution = timeline.executions[0]!;
  assert.equal(execution.timestampsMonotonic, false);
});

test("a hole in the journal is reported instead of being smoothed away", () => {
  const timeline = buildAuditTimeline([
    entry("exec-parent", 1, "execution.input", "2026-11-12T00:00:01.000Z", { taskId: "task-1" }),
    entry("exec-parent", 3, "tool.started", "2026-11-12T00:00:03.000Z", {}, "runtime.event")
  ]);
  assert.deepEqual(timeline.executions[0]!.gaps, [2]);
  assert.deepEqual(timeline.gaps, [{ executionId: "exec-parent", sequences: [2] }]);
});

test("the stages of a run are reconstructed from the recorded events", () => {
  const timeline = buildAuditTimeline(parentRun());
  assert.deepEqual(timeline.executions[0]!.phases, [
    "started",
    "tool",
    "change",
    "evaluation",
    "completed"
  ]);
});

test("the parent's spawn record names the child and the child's record names the parent", () => {
  const timeline = buildAuditTimeline([
    ...parentRun(),
    entry("exec-parent", 9, "subagent.spawned", "2026-11-12T00:00:07.500Z", {
      childExecutionId: "exec-child"
    }),
    ...childRun()
  ]);
  const parent = timeline.executions.find(item => item.executionId === "exec-parent")!;
  const child = timeline.executions.find(item => item.executionId === "exec-child")!;
  assert.deepEqual(parent.childExecutionIds, ["exec-child"]);
  assert.equal(child.parentExecutionId, "exec-parent");
});

test("asking for a task returns the delegated child even though it carries its own task id", async () => {
  const journal = new InMemoryJournalStore();
  for (const item of [...parentRun(), entry("exec-parent", 9, "subagent.spawned", "2026-11-12T00:00:07.500Z", { childExecutionId: "exec-child" }), ...childRun()]) {
    await journal.append(item);
  }
  const timeline = await new AuditTimelineService(journal).build({ taskId: "task-1" });
  const ids = timeline.executions.map(item => item.executionId).sort();
  assert.deepEqual(ids, ["exec-child", "exec-parent"]);
});

test("asking for an execution returns its parent and its children", async () => {
  const journal = new InMemoryJournalStore();
  for (const item of [
    ...parentRun(),
    entry("exec-parent", 9, "subagent.spawned", "2026-11-12T00:00:07.500Z", { childExecutionId: "exec-child" }),
    ...childRun()
  ]) {
    await journal.append(item);
  }
  const timeline = await new AuditTimelineService(journal).build({ executionId: "exec-child" });
  const ids = timeline.executions.map(item => item.executionId).sort();
  assert.deepEqual(ids, ["exec-child", "exec-parent"]);
});

test("a category filter keeps only that category and still reports the executions", async () => {
  const journal = new InMemoryJournalStore();
  for (const item of parentRun()) await journal.append(item);
  const timeline = await new AuditTimelineService(journal).build({ category: "change.applied" });
  assert.equal(timeline.items.length, 1);
  assert.equal(timeline.items[0]!.category, "change.applied");
  assert.equal(timeline.executions.length, 1);
  assert.equal(timeline.executions[0]!.eventCount, 8);
});

test("a limit truncates the items and says so", async () => {
  const journal = new InMemoryJournalStore();
  for (const item of parentRun()) await journal.append(item);
  const timeline = await new AuditTimelineService(journal).build({ limit: 3 });
  assert.equal(timeline.items.length, 3);
  assert.equal(timeline.truncated, true);
});

test("a time filter keeps the events recorded at or after it", async () => {
  const journal = new InMemoryJournalStore();
  for (const item of parentRun()) await journal.append(item);
  const timeline = await new AuditTimelineService(journal).build({ since: "2026-11-12T00:00:06.000Z" });
  assert.deepEqual(timeline.items.map(item => item.sequence), [7, 8]);
});

test("a timeline that was never recorded is not answered with an empty timeline", async () => {
  const journal = new InMemoryJournalStore();
  const timeline = await new AuditTimelineService(journal).build({ executionId: "exec-never-recorded" });
  // The service has no executions to answer with; the API layer is what refuses the id.
  assert.equal(timeline.items.length, 0);
  assert.equal(timeline.executions.length, 0);
});

/**
 * The P0-022 probe found this on a real run: the runtime's first event is emitted *before* the
 * facts that precede it in the journal are written, so a journal that stored the emit time put an
 * event before the facts recorded before it — and its own timestamps contradicted its own order.
 */
test("the journal records the time it was written, not the time the runtime emitted the event", async () => {
  const store = new InMemoryJournalStore();
  await recordRuntimeEvent(
    store,
    {
      executionId: "exec-1",
      projectId: "proj-1",
      taskId: "task-1",
      sequence: 1,
      eventId: "evt-1",
      eventType: "execution.started",
      timestamp: "2026-11-12T00:00:00.000Z",
      payload: {}
    },
    2,
    "2026-11-12T00:00:05.000Z"
  );
  const entries = await store.read("exec-1");
  assert.equal(entries[0]!.recordedAt, "2026-11-12T00:00:05.000Z");
  // The runtime's own time is not lost: it is kept where it belongs, in the payload.
  assert.equal(entries[0]!.payload["eventTimestamp"], "2026-11-12T00:00:00.000Z");
});

test("a timeline stays ordered when the runtime emitted an event before the facts before it", async () => {
  const store = new InMemoryJournalStore();
  await recordAudit(store, {
    executionId: "exec-1",
    sequence: 1,
    category: "execution.input",
    payload: { taskId: "task-1" },
    recordedAt: "2026-11-12T00:00:05.000Z"
  });
  // Emitted at 00:00:00, recorded after the fact above.
  await recordRuntimeEvent(
    store,
    {
      executionId: "exec-1",
      projectId: "proj-1",
      taskId: "task-1",
      sequence: 1,
      eventId: "evt-1",
      eventType: "execution.started",
      timestamp: "2026-11-12T00:00:00.000Z",
      payload: {}
    },
    2,
    "2026-11-12T00:00:06.000Z"
  );
  const timeline = buildAuditTimeline(await store.read("exec-1"));
  assert.deepEqual(timeline.items.map(item => item.recordedAt), [
    "2026-11-12T00:00:05.000Z",
    "2026-11-12T00:00:06.000Z"
  ]);
  assert.equal(timeline.executions[0]!.timestampsMonotonic, true);
});
