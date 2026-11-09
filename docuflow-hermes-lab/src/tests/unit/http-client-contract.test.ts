import test from "node:test";
import assert from "node:assert/strict";
import { HermesHttpClient, HermesHttpError } from "../../clients/http/hermes-http-client.js";
import { executeHttpPhase0 } from "../../runners/http-phase0.js";
import { startMockHermes } from "../support/mock-hermes-server.js";

test("contract: the Phase 0 HTTP runner passes against a conforming server", async () => {
  const server = await startMockHermes();
  try {
    const client = new HermesHttpClient({ baseUrl: server.url });
    const results = await executeHttpPhase0(client);

    assert.deepEqual(results.map(item => [item.id, item.status]), [
      ["P0-001", "passed"],
      ["P0-002", "passed"]
    ]);
    assert.ok(results[0]!.evidence.some(item => /run_mock_1/.test(item)));
    assert.equal(results[1]!.evidence.length, 4);
    assert.match(results[1]!.evidence[3]!, /run\.completed/);
    assert.ok(server.requests.some(request => request.path === "/v1/capabilities"));
    assert.ok(server.requests.some(request => request.method === "POST" && request.path === "/v1/runs"));
  } finally {
    await server.close();
  }
});

test("contract: the client surfaces HTTP errors with status and body", async () => {
  const server = await startMockHermes();
  try {
    const client = new HermesHttpClient({ baseUrl: server.url });

    await assert.rejects(client.getRun("run_missing"), (error: unknown) => {
      assert.ok(error instanceof HermesHttpError);
      assert.equal(error.status, 404);
      assert.match(error.responseBody, /run_not_found/);
      return true;
    });
  } finally {
    await server.close();
  }
});

test("contract: SSE events keep their order, ids and terminal frame", async () => {
  const server = await startMockHermes();
  try {
    const client = new HermesHttpClient({ baseUrl: server.url });
    const run = await client.createRun({ input: "test" });
    const seen: Array<{ event: string; id?: string }> = [];
    for await (const event of client.streamRunEvents(run.run_id)) {
      seen.push({ event: event.event, ...(event.id ? { id: event.id } : {}) });
    }

    assert.deepEqual(seen.map(item => item.event), [
      "run.started",
      "message.delta",
      "message.delta",
      "run.completed"
    ]);
    assert.deepEqual(seen.map(item => item.id), ["1", "2", "3", "4"]);
  } finally {
    await server.close();
  }
});

test("contract: an abort stops the stream instead of hanging", async () => {
  const server = await startMockHermes({
    events: [
      { event: "run.started", data: {}, id: "1" },
      { event: "message.delta", data: { text: "slow" }, id: "2", delayMs: 2_000 }
    ]
  });
  try {
    const client = new HermesHttpClient({ baseUrl: server.url });
    const run = await client.createRun({ input: "test" });
    const controller = new AbortController();
    const seen: string[] = [];

    await assert.rejects(async () => {
      for await (const event of client.streamRunEvents(run.run_id, controller.signal)) {
        seen.push(event.event);
        controller.abort();
      }
    });

    assert.deepEqual(seen, ["run.started"]);
  } finally {
    await server.close();
  }
});

test("contract: stop and approval reach the runtime with the expected payloads", async () => {
  const server = await startMockHermes();
  try {
    const client = new HermesHttpClient({ baseUrl: server.url });
    const run = await client.createRun({ input: "test" });

    await client.stopRun(run.run_id);
    await client.approveRun(run.run_id, { request_id: "req-1", decision: "deny" });

    assert.equal(server.runs.get(run.run_id)?.status, "cancelled");
    assert.deepEqual(server.approvals, [{ request_id: "req-1", decision: "deny" }]);
  } finally {
    await server.close();
  }
});

test("contract: missing capabilities fail Phase 0 before a run is created", async () => {
  const server = await startMockHermes({ capabilities: { run_submission: true, run_events_sse: false } });
  try {
    const client = new HermesHttpClient({ baseUrl: server.url });
    const results = await executeHttpPhase0(client);

    assert.equal(results[0]!.status, "failed");
    assert.match(results[0]!.notes ?? "", /run_events_sse/);
    assert.equal(results[1]!.status, "blocked");
    assert.equal(server.requests.some(request => request.method === "POST" && request.path === "/v1/runs"), false);
  } finally {
    await server.close();
  }
});

test("contract: bearer auth is sent and a rejected token is reported", async () => {
  const server = await startMockHermes({ token: "s3cret" });
  try {
    const authorized = new HermesHttpClient({ baseUrl: server.url, apiKey: "s3cret" });
    const capabilities = await authorized.capabilities();
    assert.equal(capabilities.platform, "mock-hermes");

    const anonymous = new HermesHttpClient({ baseUrl: server.url });
    await assert.rejects(anonymous.capabilities(), (error: unknown) => {
      assert.ok(error instanceof HermesHttpError);
      assert.equal(error.status, 401);
      return true;
    });
  } finally {
    await server.close();
  }
});
