import test from "node:test";
import assert from "node:assert/strict";
import { executeHttpPhase0, type HttpPhase0Client } from "../../runners/http-phase0.js";
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
    { event: "run.started", data: { run_id: "run-1" } }
  ]));

  assert.deepEqual(results.map(item => [item.id, item.status]), [
    ["P0-001", "passed"],
    ["P0-002", "passed"]
  ]);
  assert.ok(results[0]!.evidence.some(item => /run-1/.test(item)));
  assert.match(results[1]!.evidence[0]!, /run\.started/);
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
  assert.match(results[1]!.notes ?? "", /without any events/);
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
