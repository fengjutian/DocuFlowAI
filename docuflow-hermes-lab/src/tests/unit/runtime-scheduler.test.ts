import test from "node:test";
import assert from "node:assert/strict";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import { RuntimeScheduler } from "../../adapters/runtime-scheduler.js";
import {
  AgentAdapterError,
  NO_CAPABILITIES,
  type AgentCapability,
  type AgentRuntimeAdapter
} from "../../types/adapter.js";

function adapter(options: {
  healthy?: boolean;
  capabilities?: Partial<AgentCapability>;
  capabilityThrows?: boolean;
} = {}): AgentRuntimeAdapter {
  return {
    async getCapabilities() {
      if (options.capabilityThrows) throw new Error("capabilities unavailable");
      return { ...NO_CAPABILITIES, ...options.capabilities };
    },
    async healthCheck() { return options.healthy ?? true; },
    async start() { throw new Error("not used"); },
    async getStatus() { throw new Error("not used"); },
    async cancel() { throw new Error("not used"); },
    async resume() { throw new Error("not used"); },
    async subscribe() { throw new Error("not used"); },
    async terminate() { throw new Error("not used"); }
  };
}

function registryWith(entries: Array<{ id: string; adapter: AgentRuntimeAdapter }>): AgentRuntimeRegistry {
  const registry = new AgentRuntimeRegistry();
  for (const entry of entries) {
    registry.register({ id: entry.id, runtimeType: "hermes", adapter: entry.adapter });
  }
  return registry;
}

test("picks the runtime with the least load and tracks busy/idle", async () => {
  const registry = registryWith([
    { id: "a", adapter: adapter() },
    { id: "b", adapter: adapter() }
  ]);
  const scheduler = new RuntimeScheduler(registry, { strategy: "least_loaded" });

  const first = await scheduler.select();
  scheduler.markBusy(first.runtimeId);
  scheduler.markBusy(first.runtimeId);

  const second = await scheduler.select();
  assert.notEqual(second.runtimeId, first.runtimeId);
  assert.equal(scheduler.loadOf(first.runtimeId), 2);

  scheduler.markIdle(first.runtimeId);
  scheduler.markIdle(first.runtimeId);
  assert.equal(scheduler.loadOf(first.runtimeId), 0);
});

test("rotates through candidates in round-robin order", async () => {
  const registry = registryWith([
    { id: "a", adapter: adapter() },
    { id: "b", adapter: adapter() },
    { id: "c", adapter: adapter() }
  ]);
  const scheduler = new RuntimeScheduler(registry, { strategy: "round_robin" });

  const picked: string[] = [];
  for (let index = 0; index < 4; index += 1) {
    picked.push((await scheduler.select()).runtimeId);
  }

  assert.deepEqual(picked, ["a", "b", "c", "a"]);
});

test("skips unhealthy runtimes", async () => {
  const registry = registryWith([
    { id: "sick", adapter: adapter({ healthy: false }) },
    { id: "well", adapter: adapter() }
  ]);
  const scheduler = new RuntimeScheduler(registry, { strategy: "first_healthy" });

  assert.equal((await scheduler.select()).runtimeId, "well");
});

test("requires the requested capabilities", async () => {
  const registry = registryWith([
    { id: "plain", adapter: adapter() },
    { id: "streamer", adapter: adapter({ capabilities: { streaming: true } }) }
  ]);
  const scheduler = new RuntimeScheduler(registry, { strategy: "first_healthy" });

  const selected = await scheduler.select({ requiredCapabilities: { streaming: true } });
  assert.equal(selected.runtimeId, "streamer");

  await assert.rejects(
    scheduler.select({ requiredCapabilities: { sessionResume: true } }),
    (error: unknown) => error instanceof AgentAdapterError && error.code === "unsupported_capability"
  );
});

test("reports why no runtime can serve the request", async () => {
  const empty = new RuntimeScheduler(new AgentRuntimeRegistry());
  await assert.rejects(empty.select(), /No runtime is registered/);

  const registry = registryWith([{ id: "a", adapter: adapter({ capabilityThrows: true }) }]);
  const scheduler = new RuntimeScheduler(registry);
  await assert.rejects(
    scheduler.select({ requiredCapabilities: { streaming: true } }),
    (error: unknown) => error instanceof AgentAdapterError && error.code === "unsupported_capability"
  );
});

test("fails closed when every candidate is unhealthy", async () => {
  const registry = registryWith([
    { id: "a", adapter: adapter({ healthy: false }) },
    { id: "b", adapter: adapter({ healthy: false }) }
  ]);
  const scheduler = new RuntimeScheduler(registry);

  await assert.rejects(
    scheduler.select(),
    (error: unknown) => error instanceof AgentAdapterError && error.code === "runtime_unreachable"
  );
});

test("filters by runtime type before scheduling", async () => {
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "h", runtimeType: "hermes", adapter: adapter() });
  registry.register({ id: "n", runtimeType: "native", adapter: adapter() });
  const scheduler = new RuntimeScheduler(registry, { strategy: "first_healthy" });

  assert.equal((await scheduler.select({ runtimeType: "native" })).runtimeId, "n");
  await assert.rejects(scheduler.select({ runtimeType: "acp" }), /No runtime is registered/);
});
