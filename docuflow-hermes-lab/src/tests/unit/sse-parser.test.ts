import test from "node:test";
import assert from "node:assert/strict";
import { parseSseBlock } from "../../clients/http/hermes-http-client.js";

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
