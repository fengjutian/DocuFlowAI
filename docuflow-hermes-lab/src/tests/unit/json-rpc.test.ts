import test from "node:test";
import assert from "node:assert/strict";
import { JsonRpcPendingRequests } from "../../clients/gateway/json-rpc.js";

test("resolves a pending JSON-RPC request", async () => {
  const tracker = new JsonRpcPendingRequests();
  const resultPromise = tracker.register<{ ok: boolean }>(1);
  assert.equal(tracker.settle({ jsonrpc: "2.0", id: 1, result: { ok: true } }), true);
  assert.deepEqual(await resultPromise, { ok: true });
  assert.equal(tracker.size, 0);
});

test("rejects JSON-RPC errors", async () => {
  const tracker = new JsonRpcPendingRequests();
  const resultPromise = tracker.register(2);
  tracker.settle({ jsonrpc: "2.0", id: 2, error: { code: -32601, message: "Method not found" } });
  await assert.rejects(resultPromise, /Method not found/);
});

test("rejects duplicate ids", () => {
  const tracker = new JsonRpcPendingRequests();
  void tracker.register(3);
  assert.throws(() => tracker.register(3), /Duplicate/);
});
