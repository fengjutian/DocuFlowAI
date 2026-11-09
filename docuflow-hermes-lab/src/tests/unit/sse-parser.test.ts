import test from "node:test";
import assert from "node:assert/strict";
import { HermesHttpClient, parseSseBlock } from "../../clients/http/hermes-http-client.js";

test("parses JSON SSE event with event name and id", () => {
  const result = parseSseBlock('id: 17\nevent: run.started\ndata: {"run_id":"r1"}');
  assert.deepEqual(result, { id: "17", event: "run.started", data: { run_id: "r1" } });
});

test("joins multiline data and preserves non-JSON payload", () => {
  const result = parseSseBlock("data: first\ndata: second");
  assert.deepEqual(result, { event: "message", data: "first\nsecond" });
});

test("ignores heartbeat/comment blocks", () => {
  assert.equal(parseSseBlock(": keep-alive"), null);
  assert.equal(parseSseBlock(""), null);
});

test("omits an absent abort signal from the SSE request", async () => {
  let requestInit: RequestInit | undefined;
  const fetchImpl: typeof fetch = async (_input, init) => {
    requestInit = init;
    return new Response("", { status: 200 });
  };
  const client = new HermesHttpClient({ baseUrl: "http://localhost:3000", fetchImpl });

  for await (const _event of client.streamRunEvents("run-1")) {
    // The empty response produces no events.
  }

  assert.equal(Object.hasOwn(requestInit ?? {}, "signal"), false);
});
