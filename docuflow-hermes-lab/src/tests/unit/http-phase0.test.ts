import test from "node:test";
import assert from "node:assert/strict";
import { executeHttpPhase0, type HttpPhase0Client } from "../../runners/http-phase0.js";
import type { HermesSseEvent } from "../../types/hermes.js";

function clientWith(events: HermesSseEvent[]): HttpPhase0Client {
  return {
    async createRun() { return { id: "run-1", status: "queued" }; },
    async getRun(runId) { return { id: runId, status: "running" }; },
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
  assert.match(results[0]!.evidence[0]!, /run-1/);
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
