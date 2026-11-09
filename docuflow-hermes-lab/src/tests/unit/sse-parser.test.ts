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

test("parses CRLF fields and a valid retry delay", () => {
  const result = parseSseBlock("event: reconnect\r\nretry: 1500\r\ndata: later");
  assert.deepEqual(result, { event: "reconnect", data: "later", retry: 1500 });
});

test("streams CRLF-delimited events across response chunks", async () => {
  const encoder = new TextEncoder();
  const chunks = ["event: run.started\r\ndata: {\"run_id\":\"r1\"}\r", "\n\r", "\n"];
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
      controller.close();
    }
  });
  const client = new HermesHttpClient({
    baseUrl: "http://localhost:3000",
    fetchImpl: async () => new Response(body, { status: 200 })
  });

  const events = [];
  for await (const event of client.streamRunEvents("run-1")) events.push(event);

  assert.deepEqual(events, [{ event: "run.started", data: { run_id: "r1" } }]);
});

test("ignores heartbeat/comment blocks", () => {
  assert.equal(parseSseBlock(": keep-alive"), null);
  assert.equal(parseSseBlock(""), null);
});

test("keeps data fields that follow a comment in the same block", () => {
  assert.deepEqual(parseSseBlock(": metadata\ndata: ready"), {
    event: "message",
    data: "ready"
  });
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
