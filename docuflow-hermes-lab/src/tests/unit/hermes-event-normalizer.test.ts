import test from "node:test";
import assert from "node:assert/strict";
import { normalizeHermesEvent } from "../../adapters/hermes-event-normalizer.js";

const context = {
  executionId: "exec-1",
  projectId: "proj-1",
  taskId: "task-1",
  sequence: 3,
  timestamp: "2026-11-09T00:00:00.000Z"
};

test("maps a known Hermes event and keeps the original name for audit", () => {
  const event = normalizeHermesEvent(
    { event: "run.completed", data: { run_id: "run-1", output: "done" }, id: "evt-9" },
    context
  );

  assert.equal(event.eventType, "execution.completed");
  assert.equal(event.externalEventId, "evt-9");
  assert.equal(event.sequence, 3);
  assert.equal(event.eventId, "exec-1:3");
  assert.equal(event.payload["hermesEvent"], "run.completed");
  assert.equal(event.payload["output"], "done");
  assert.equal(event.projectId, "proj-1");
  assert.equal(event.taskId, "task-1");
});

test("resolves the event name from the payload when the SSE frame uses the default name", () => {
  const event = normalizeHermesEvent({ event: "message", data: { type: "tool.started", tool: "read" } }, context);

  assert.equal(event.eventType, "tool.started");
  assert.equal(event.payload["hermesEvent"], "tool.started");
  assert.equal(event.externalEventId, undefined);
});

test("preserves unknown events instead of dropping them", () => {
  const event = normalizeHermesEvent(
    { event: "run.something_new", data: { anything: 1 } },
    context
  );

  assert.equal(event.eventType, "runtime.unknown");
  assert.equal(event.payload["hermesEvent"], "run.something_new");
  assert.deepEqual(event.payload["raw"], { anything: 1 });
});

test("keeps non-object payloads under the data field", () => {
  const event = normalizeHermesEvent({ event: "message.delta", data: "partial text" }, context);

  assert.equal(event.eventType, "message.delta");
  assert.equal(event.payload["data"], "partial text");
});

test("treats an unmapped default message frame as unknown but keeps the raw frame", () => {
  const event = normalizeHermesEvent({ event: "message", data: "plain" }, context);

  assert.equal(event.eventType, "runtime.unknown");
  assert.equal(event.payload["hermesEvent"], "message");
  assert.equal(event.payload["raw"], "plain");
});
