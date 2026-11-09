import test from "node:test";
import assert from "node:assert/strict";
import { executeGatewayPhase0, type GatewayPhase0Client } from "../../runners/gateway-phase0.js";

test("records a successful Gateway JSON-RPC exchange", async () => {
  const client: GatewayPhase0Client = {
    async request(method, params) { return { session_id: "s1", method, params }; }
  };

  const result = await executeGatewayPhase0(client, {
    method: "session.create",
    params: { prompt: "hello" }
  });

  assert.equal(result.status, "passed");
  assert.deepEqual(result.evidence.slice(0, 1), ["method=session.create"]);
  assert.match(result.evidence[1]!, /session_id/);
});

test("records a failed Gateway JSON-RPC exchange", async () => {
  const client: GatewayPhase0Client = {
    async request() { throw new Error("Method not found"); }
  };

  const result = await executeGatewayPhase0(client, { method: "unknown" });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /Method not found/);
});
