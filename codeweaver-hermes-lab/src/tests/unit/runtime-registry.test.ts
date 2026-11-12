import test from "node:test";
import assert from "node:assert/strict";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import { AgentAdapterError, NO_CAPABILITIES, type AgentRuntimeAdapter } from "../../types/adapter.js";

function stubAdapter(healthy: boolean, delayMs = 0): AgentRuntimeAdapter {
  return {
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() {
      if (delayMs > 0) await new Promise(resolve => setTimeout(resolve, delayMs));
      return healthy;
    },
    async start() { throw new Error("not used"); },
    async getStatus() { throw new Error("not used"); },
    async cancel() { throw new Error("not used"); },
    async resume() { throw new Error("not used"); },
    async subscribe() { throw new Error("not used"); },
    async terminate() { throw new Error("not used"); }
  };
}

test("registers runtimes and resolves their adapters", () => {
  const registry = new AgentRuntimeRegistry();
  const adapter = stubAdapter(true);

  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter, version: "0.23.0" });

  assert.equal(registry.has("hermes-local"), true);
  assert.equal(registry.require("hermes-local"), adapter);
  assert.equal(registry.list().length, 1);
  assert.equal(registry.list()[0]!.version, "0.23.0");
});

test("rejects duplicate registrations and empty ids", () => {
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: stubAdapter(true) });

  assert.throws(() => registry.register({
    id: "hermes-local",
    runtimeType: "hermes",
    adapter: stubAdapter(true)
  }), (error: unknown) => error instanceof AgentAdapterError && error.code === "invalid_state");

  assert.throws(() => registry.register({
    id: " ",
    runtimeType: "hermes",
    adapter: stubAdapter(true)
  }), /must not be empty/);
});

test("reports a missing runtime rather than returning an undefined adapter", () => {
  const registry = new AgentRuntimeRegistry();

  assert.throws(() => registry.require("nope"), (error: unknown) => {
    assert.ok(error instanceof AgentAdapterError);
    assert.equal(error.code, "execution_not_found");
    return true;
  });
  assert.equal(registry.unregister("nope"), false);
});

test("keeps disabled runtimes visible while refusing new work and health probes", async () => {
  const registry = new AgentRuntimeRegistry();
  const adapter = stubAdapter(true);
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter });

  registry.setEnabled("hermes-local", false);

  assert.equal(registry.list()[0]!.enabled, false);
  assert.equal(registry.get("hermes-local"), undefined);
  assert.throws(() => registry.require("hermes-local"), /disabled/);
  const report = await registry.healthCheckAll();
  assert.equal(report.runtimes[0]!.healthy, false);
  assert.match(report.runtimes[0]!.error ?? "", /disabled/i);

  registry.setEnabled("hermes-local", true);
  assert.equal(registry.require("hermes-local"), adapter);
});

test("aggregates health across runtimes and marks the report unhealthy if any fails", async () => {
  const registry = new AgentRuntimeRegistry({ now: () => new Date("2026-11-09T00:00:00.000Z") });
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: stubAdapter(true) });
  registry.register({ id: "hermes-ci", runtimeType: "hermes", adapter: stubAdapter(false) });

  const report = await registry.healthCheckAll();

  assert.equal(report.checkedAt, "2026-11-09T00:00:00.000Z");
  assert.equal(report.healthy, false);
  assert.deepEqual(report.runtimes.map(item => [item.id, item.healthy]), [
    ["hermes-local", true],
    ["hermes-ci", false]
  ]);
  assert.equal(report.runtimes[0]!.error, undefined);
  assert.match(String(report.runtimes[1]!.error), /returned false/);
});

test("treats a hung health check as unhealthy instead of hanging the caller", async () => {
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "slow", runtimeType: "hermes", adapter: stubAdapter(true, 500) });

  const report = await registry.healthCheckAll(20);

  assert.equal(report.runtimes[0]!.healthy, false);
  assert.match(String(report.runtimes[0]!.error), /timed out/);
});

test("records the error message when an adapter throws during health checks", async () => {
  const registry = new AgentRuntimeRegistry();
  const adapter = stubAdapter(true);
  adapter.healthCheck = async () => { throw new Error("spawn ENOENT"); };
  registry.register({ id: "broken", runtimeType: "hermes", adapter });

  const report = await registry.healthCheckAll();

  assert.match(String(report.runtimes[0]!.error), /spawn ENOENT/);
});
