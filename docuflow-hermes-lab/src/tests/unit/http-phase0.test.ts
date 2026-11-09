import test from "node:test";
import assert from "node:assert/strict";
import { executeHttpPhase0, isTerminalRunEvent, type HttpPhase0Client } from "../../runners/http-phase0.js";
import type { HermesSseEvent } from "../../types/hermes.js";

function clientWith(events: HermesSseEvent[]): HttpPhase0Client {
  return {
    async capabilities() {
      return { platform: "hermes-agent", features: { run_submission: true, run_events_sse: true } };
    },
    async createRun() { return { run_id: "run-1", status: "queued" }; },
    async getRun(runId) { return { run_id: runId, status: "running" }; },
    async *streamRunEvents() {
      for (const event of events) yield event;
    }
  };
}

test("records successful HTTP and SSE Phase 0 checks", async () => {
  const results = await executeHttpPhase0(clientWith([
    { event: "run.started", data: { run_id: "run-1" } },
    { event: "run.completed", data: { output: "ok", completed: true } }
  ]));

  assert.deepEqual(results.map(item => [item.id, item.status]), [
    ["P0-001", "passed"],
    ["P0-002", "passed"]
  ]);
  assert.ok(results[0]!.evidence.some(item => /run-1/.test(item)));
  assert.match(results[1]!.evidence[0]!, /run\.started/);
});

test("recognizes a terminal event whose name only travels in the payload", () => {
  // The real 0.23.0 API server leaves the SSE frame name at `message` and puts the
  // runtime event name in `data.event`.
  assert.equal(isTerminalRunEvent({ event: "message", data: { event: "run.completed" } }), true);
  assert.equal(isTerminalRunEvent({ event: "message", data: { event: "run.failed" } }), true);
  assert.equal(isTerminalRunEvent({ event: "message", data: { event: "message.delta" } }), false);
  assert.equal(isTerminalRunEvent({ event: "run.completed", data: {} }), true);
});

test("passes SSE verification when the terminal event arrives inside the payload", async () => {
  const results = await executeHttpPhase0(clientWith([
    { event: "message", data: { event: "message.delta", delta: "ok" } },
    { event: "message", data: { event: "run.completed", output: "ok", completed: true } }
  ]));

  assert.equal(results[1]!.status, "passed");
});

test("fails SSE verification when the stream never reaches a terminal event", async () => {
  const results = await executeHttpPhase0(clientWith([
    { event: "run.started", data: { run_id: "run-1" } }
  ]));

  assert.equal(results[1]!.status, "failed");
  assert.match(results[1]!.notes ?? "", /without a terminal run event/);
});

test("blocks SSE verification when run creation fails", async () => {
  const client = clientWith([]);
  client.createRun = async () => { throw new Error("connection refused"); };

  const results = await executeHttpPhase0(client);

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /connection refused/);
  assert.equal(results[1]!.status, "blocked");
});

test("fails SSE verification when the stream has no events", async () => {
  const results = await executeHttpPhase0(clientWith([]));

  assert.equal(results[0]!.status, "passed");
  assert.equal(results[1]!.status, "failed");
  assert.match(results[1]!.notes ?? "", /No data event within/);
});

test("fails before run creation when required capabilities are absent", async () => {
  const client = clientWith([]);
  client.capabilities = async () => ({
    platform: "hermes-agent",
    features: { run_submission: false, run_events_sse: false }
  });

  const results = await executeHttpPhase0(client);

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /run_submission/);
  assert.equal(results[1]!.status, "blocked");
});
