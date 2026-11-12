import test from "node:test";
import assert from "node:assert/strict";
import {
  judgeAuditTimeline,
  type AuditTimelineItemView,
  type AuditTimelineView
} from "../../runners/audit-timeline-phase0.js";

/**
 * The verdict logic has to be able to fail.
 *
 * Every case below breaks one link of a healthy timeline and asserts that exactly that link turns
 * red. A verdict that cannot go red is not evidence of anything.
 */

const taskId = "task-1";
const firstExecutionId = "exec-parent";
const secondExecutionId = "exec-child";

function item(
  executionId: string,
  sequence: number,
  category: string,
  recordedAt: string,
  payload: Record<string, unknown> = {}
): AuditTimelineItemView {
  const taskId = category === "execution.input" ? "task-1" : undefined;
  return {
    executionId,
    sequence,
    kind: category.startsWith("execution.") ? "runtime.event" : "audit",
    category,
    recordedAt,
    ...(taskId ? { taskId } : {}),
    payload
  };
}

function healthyTimeline(): AuditTimelineView {
  return {
    items: [
      item(firstExecutionId, 1, "execution.input", "2026-11-12T00:00:00.000Z"),
      item(firstExecutionId, 2, "execution.started", "2026-11-12T00:00:01.000Z"),
      item(firstExecutionId, 3, "tool.started", "2026-11-12T00:00:02.000Z"),
      item(firstExecutionId, 4, "change.applied", "2026-11-12T00:00:03.000Z", {
        gitCommitSha: "c".repeat(40)
      }),
      item(firstExecutionId, 5, "evaluation.completed", "2026-11-12T00:00:04.000Z", {
        verdict: "passed"
      }),
      item(firstExecutionId, 6, "execution.completed", "2026-11-12T00:00:05.000Z"),
      item(secondExecutionId, 1, "execution.input", "2026-11-12T00:00:06.000Z"),
      item(secondExecutionId, 2, "execution.completed", "2026-11-12T00:00:07.000Z")
    ],
    executions: [
      {
        executionId: firstExecutionId,
        taskIds: [taskId],
        childExecutionIds: [],
        eventCount: 6,
        firstRecordedAt: "2026-11-12T00:00:00.000Z",
        lastRecordedAt: "2026-11-12T00:00:05.000Z",
        gaps: [],
        phases: ["started", "tool", "change", "evaluation", "completed"],
        timestampsMonotonic: true
      },
      {
        executionId: secondExecutionId,
        taskIds: [taskId],
        childExecutionIds: [],
        eventCount: 2,
        firstRecordedAt: "2026-11-12T00:00:06.000Z",
        lastRecordedAt: "2026-11-12T00:00:07.000Z",
        gaps: [],
        phases: ["started", "completed"],
        timestampsMonotonic: true
      }
    ],
    taskIds: [taskId],
    categories: {},
    gaps: [],
    truncated: false
  };
}

function judge(overrides: Partial<Parameters<typeof judgeAuditTimeline>[0]> = {}) {
  const timeline = healthyTimeline();
  return judgeAuditTimeline({
    taskId,
    firstExecutionId,
    secondExecutionId,
    timeline,
    taskTimeline: timeline,
    journalCounts: { [firstExecutionId]: 6, [secondExecutionId]: 2 },
    missingTimelineStatus: 404,
    ...overrides
  });
}

function link(judgement: ReturnType<typeof judge>, name: string) {
  return judgement.links.find(entry => entry.name === name)!;
}

test("a healthy timeline passes every link", () => {
  const judgement = judge();
  assert.equal(judgement.ok, true);
  assert.equal(judgement.links.length, 7);
});

test("a timeline that drops a recorded event fails the coverage link", () => {
  const timeline = healthyTimeline();
  timeline.items = timeline.items.slice(0, 5);
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "journal-covered").ok, false);
  assert.equal(judgement.ok, false);
});

test("a timeline that invents an event fails the coverage link", () => {
  const timeline = healthyTimeline();
  timeline.items = [...timeline.items, item(firstExecutionId, 99, "tool.started", "2026-11-12T00:00:08.000Z")];
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "journal-covered").ok, false);
});

test("a timeline that repeats an event fails the coverage link", () => {
  const timeline = healthyTimeline();
  timeline.items = [...timeline.items, timeline.items[0]!];
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "journal-covered").ok, false);
});

test("a timeline whose events run backwards in time fails the order link", () => {
  const timeline = healthyTimeline();
  const items = [...timeline.items];
  const swap = items[2]!;
  items[2] = items[3]!;
  items[3] = swap;
  timeline.items = items;
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "ordered").ok, false);
});

test("a timeline that reorders the events of one execution fails the order link", () => {
  const timeline = healthyTimeline();
  const own = timeline.items.filter(item => item.executionId === firstExecutionId);
  own.reverse();
  timeline.items = [...own, ...timeline.items.filter(item => item.executionId !== firstExecutionId)];
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "ordered").ok, false);
});

test("a timeline that leaves out one of the task's runs fails the correlation link", () => {
  const timeline = healthyTimeline();
  timeline.executions = timeline.executions.filter(item => item.executionId !== secondExecutionId);
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "correlated").ok, false);
});

test("a timeline whose executions do not name the task fails the correlation link", () => {
  const timeline = healthyTimeline();
  timeline.executions[0]!.taskIds = ["another-task"];
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "correlated").ok, false);
});

test("a timeline that cannot reconstruct the stages fails the process link", () => {
  const timeline = healthyTimeline();
  timeline.executions[0]!.phases = ["started", "tool", "completed"];
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "process").ok, false);
});

test("a timeline without the commit fails the evidence link", () => {
  const timeline = healthyTimeline();
  timeline.items = timeline.items.map(item =>
    item.category === "change.applied" ? { ...item, payload: {} } : item
  );
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "evidence").ok, false);
});

test("a timeline without a passing verdict fails the evidence link", () => {
  const timeline = healthyTimeline();
  timeline.items = timeline.items.map(item =>
    item.category === "evaluation.completed" ? { ...item, payload: { verdict: "failed" } } : item
  );
  const judgement = judge({ timeline });
  assert.equal(link(judgement, "evidence").ok, false);
});

test("a task timeline that holds only one of the runs fails the task-scope link", () => {
  const timeline = healthyTimeline();
  const taskTimeline = healthyTimeline();
  taskTimeline.executions = taskTimeline.executions.filter(
    item => item.executionId !== secondExecutionId
  );
  const judgement = judge({ taskTimeline });
  assert.equal(link(judgement, "task-scope").ok, false);
});

test("a timeline that answers an unknown execution instead of refusing it fails the not-found link", () => {
  const judgement = judge({ missingTimelineStatus: 200 });
  assert.equal(link(judgement, "not-found").ok, false);
});
