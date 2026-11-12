import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import { RuntimeScheduler } from "../../adapters/runtime-scheduler.js";
import { RuntimeConfigService } from "../../adapters/runtime-config-service.js";
import {
  RuntimeConfigError,
  RuntimeConfigStore,
  parseRuntimeConfig
} from "../../persistence/runtime-config.js";
import { RuntimeConfigAuditLog } from "../../persistence/runtime-config-audit.js";
import { NO_CAPABILITIES, type AgentRuntimeAdapter } from "../../types/adapter.js";

/**
 * The persistent runtime configuration (V0.3.0).
 *
 * These tests hold the three claims the feature makes: the configuration survives a restart, a
 * broken configuration stops the server instead of being half-applied, and disabling a runtime
 * stops new work without cutting the sessions that are already running.
 */

function adapter(label: string): AgentRuntimeAdapter {
  return {
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() {
      return { executionId: `${label}-exec`, runtimeType: "native" as const, status: "running" as const };
    },
    async getStatus() { return "running" as const; },
    async cancel() {},
    async resume() {
      return { executionId: `${label}-exec`, runtimeType: "native" as const, status: "running" as const };
    },
    async subscribe() { return async () => undefined; },
    async terminate() {}
  };
}

async function workspace(): Promise<string> {
  return mkdtemp(join(tmpdir(), "codeweaver-runtime-config-"));
}

function serviceFor(root: string, registry: AgentRuntimeRegistry): RuntimeConfigService {
  return new RuntimeConfigService({
    registry,
    store: new RuntimeConfigStore(root),
    audit: new RuntimeConfigAuditLog(root)
  });
}

// --- The file on disk ------------------------------------------------------------------------------

test("a missing configuration is an empty configuration, not an error", async () => {
  const root = await workspace();
  try {
    const store = new RuntimeConfigStore(root);
    const document = await store.load();
    assert.deepEqual(document, { version: 1, runtimes: {} });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a saved configuration is read back exactly", async () => {
  const root = await workspace();
  try {
    const store = new RuntimeConfigStore(root);
    await store.save({
      version: 1,
      defaultRuntimeId: "pi-local",
      runtimes: { "pi-local": { enabled: false, priority: 7, tags: ["gpu", "fast"] } }
    });
    const read = await store.load();
    assert.equal(read.defaultRuntimeId, "pi-local");
    assert.deepEqual(read.runtimes["pi-local"], { enabled: false, priority: 7, tags: ["gpu", "fast"] });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("the write leaves no temporary file behind", async () => {
  const root = await workspace();
  try {
    const store = new RuntimeConfigStore(root);
    await store.save({ version: 1, runtimes: {} });
    const files = await readdir(join(root, "config"));
    assert.deepEqual(files, ["runtimes.json"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a corrupt configuration fails with the path and the reason", async () => {
  const root = await workspace();
  try {
    const store = new RuntimeConfigStore(root);
    await mkdir(join(root, "config"), { recursive: true });
    const path = join(root, "config", "runtimes.json");
    await writeFile(path, "{ this is not json", "utf8");

    await assert.rejects(
      () => store.load(),
      (error: unknown) => {
        assert.ok(error instanceof RuntimeConfigError);
        assert.equal(error.reason, "invalid_json");
        assert.equal(error.path, path);
        // The diagnostic has to name the file, or it is not a diagnosis.
        assert.match(error.message, /runtimes\.json/);
        return true;
      }
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a configuration with an unsupported version is refused", async () => {
  const root = await workspace();
  try {
    const store = new RuntimeConfigStore(root);
    await mkdir(join(root, "config"), { recursive: true });
    await writeFile(join(root, "config", "runtimes.json"), JSON.stringify({ version: 99, runtimes: {} }), "utf8");
    await assert.rejects(
      () => store.load(),
      (error: unknown) => {
        assert.ok(error instanceof RuntimeConfigError);
        assert.equal(error.reason, "unsupported_version");
        return true;
      }
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a wrongly typed field is refused instead of being corrected", async () => {
  const path = "/tmp/runtimes.json";
  assert.throws(
    () => parseRuntimeConfig(JSON.stringify({ version: 1, runtimes: { a: { enabled: "yes", priority: 0, tags: [] } } }), path),
    (error: unknown) => {
      assert.ok(error instanceof RuntimeConfigError);
      assert.equal(error.reason, "invalid_enabled");
      assert.match(error.message, /'a'/);
      return true;
    }
  );
  assert.throws(
    () => parseRuntimeConfig(JSON.stringify({ version: 1, runtimes: { a: { enabled: true, priority: "high", tags: [] } } }), path),
    (error: unknown) => {
      assert.ok(error instanceof RuntimeConfigError);
      assert.equal(error.reason, "invalid_priority");
      return true;
    }
  );
  assert.throws(
    () => parseRuntimeConfig(JSON.stringify({ version: 1, runtimes: { a: { enabled: true, priority: 0, tags: [1] } } }), path),
    (error: unknown) => {
      assert.ok(error instanceof RuntimeConfigError);
      assert.equal(error.reason, "invalid_tags");
      return true;
    }
  );
});

// --- The configuration and the registry ------------------------------------------------------------

test("the configuration applies to a runtime registered before it", async () => {
  const root = await workspace();
  try {
    const registry = new AgentRuntimeRegistry();
    registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes") });

    const service = serviceFor(root, registry);
    await service.update("hermes-local", { enabled: false, priority: 3, tags: ["remote"] });

    const entry = registry.registrationOf("hermes-local")!;
    assert.equal(entry.enabled, false);
    assert.equal(entry.priority, 3);
    assert.deepEqual(entry.tags, ["remote"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("the configuration applies to a runtime registered after it", async () => {
  const root = await workspace();
  try {
    // The stored configuration names a runtime this server has not registered yet: Pi registers
    // after the configuration is restored.
    await new RuntimeConfigStore(root).save({
      version: 1,
      runtimes: { "pi-local": { enabled: true, priority: 9, tags: ["local"] } }
    });

    const registry = new AgentRuntimeRegistry();
    const service = serviceFor(root, registry);
    await service.load();

    registry.register({ id: "pi-local", runtimeType: "native", adapter: adapter("pi") });
    const entry = registry.registrationOf("pi-local")!;
    assert.equal(entry.priority, 9);
    assert.deepEqual(entry.tags, ["local"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a disabled runtime keeps its adapter for the sessions that are already running", async () => {
  const registry = new AgentRuntimeRegistry();
  const runtime = adapter("hermes");
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: runtime });

  registry.setEnabled("hermes-local", false);

  // New work is refused...
  assert.equal(registry.get("hermes-local"), undefined);
  assert.throws(() => registry.require("hermes-local"), /disabled/);
  // ...but the session that is already running still has its runtime.
  assert.equal(registry.adapterOf("hermes-local"), runtime);
  // The runtime stays visible.
  assert.equal(registry.list().length, 1);
  assert.equal(registry.list()[0]!.enabled, false);
});

test("removing a configuration entry gives the runtime back its registered values", async () => {
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes"), priority: 5 });

  registry.setConfigOverlay({ "hermes-local": { enabled: false, priority: 0 } });
  assert.equal(registry.registrationOf("hermes-local")!.enabled, false);

  registry.setConfigOverlay({});
  assert.equal(registry.registrationOf("hermes-local")!.enabled, true);
  assert.equal(registry.registrationOf("hermes-local")!.priority, 5);
});

// --- Priority and the default runtime ---------------------------------------------------------------

test("only the highest priority tier is eligible", async () => {
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes"), priority: 1 });
  registry.register({ id: "pi-local", runtimeType: "native", adapter: adapter("pi"), priority: 5 });

  const scheduler = new RuntimeScheduler(registry, { strategy: "first_healthy" });
  const selection = await scheduler.select();
  assert.equal(selection.runtimeId, "pi-local");
});

test("the default runtime wins over the priority tiers", async () => {
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes"), priority: 1 });
  registry.register({ id: "pi-local", runtimeType: "native", adapter: adapter("pi"), priority: 5 });

  const scheduler = new RuntimeScheduler(registry, {
    strategy: "first_healthy",
    defaultRuntimeId: () => "hermes-local"
  });
  const selection = await scheduler.select();
  assert.equal(selection.runtimeId, "hermes-local");
});

test("a disabled default runtime falls back to the tiers", async () => {
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes"), priority: 1 });
  registry.register({ id: "pi-local", runtimeType: "native", adapter: adapter("pi"), priority: 5 });
  registry.setEnabled("hermes-local", false);

  const scheduler = new RuntimeScheduler(registry, {
    strategy: "first_healthy",
    defaultRuntimeId: () => "hermes-local"
  });
  const selection = await scheduler.select();
  assert.equal(selection.runtimeId, "pi-local");
});

// --- The service ------------------------------------------------------------------------------------

test("disabling a runtime is written down and audited", async () => {
  const root = await workspace();
  try {
    const registry = new AgentRuntimeRegistry();
    registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes") });
    const service = serviceFor(root, registry);

    await service.setEnabled("hermes-local", false, "console");

    // On disk...
    const stored = await new RuntimeConfigStore(root).load();
    assert.equal(stored.runtimes["hermes-local"]!.enabled, false);
    // ...in the registry...
    assert.equal(registry.registrationOf("hermes-local")!.enabled, false);
    // ...and in the audit log.
    const history = await service.history();
    assert.equal(history.length, 1);
    assert.equal(history[0]!.action, "disable");
    assert.equal(history[0]!.actor, "console");
    assert.equal(history[0]!.runtimeId, "hermes-local");
    assert.deepEqual(history[0]!.changes, [{ field: "enabled", from: true, to: false }]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("changing the priority and the tags is written down and audited", async () => {
  const root = await workspace();
  try {
    const registry = new AgentRuntimeRegistry();
    registry.register({ id: "pi-local", runtimeType: "native", adapter: adapter("pi") });
    const service = serviceFor(root, registry);

    await service.update("pi-local", { priority: 4, tags: ["fast"] }, "api");

    const stored = await new RuntimeConfigStore(root).load();
    assert.equal(stored.runtimes["pi-local"]!.priority, 4);
    assert.deepEqual(stored.runtimes["pi-local"]!.tags, ["fast"]);
    const history = await service.history();
    assert.equal(history.length, 1);
    assert.deepEqual(
      history[0]!.changes.map(change => change.field).sort(),
      ["priority", "tags"]
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a change that changes nothing is not recorded", async () => {
  const root = await workspace();
  try {
    const registry = new AgentRuntimeRegistry();
    registry.register({ id: "pi-local", runtimeType: "native", adapter: adapter("pi") });
    const service = serviceFor(root, registry);

    await service.update("pi-local", { enabled: true }, "api");

    // The runtime was already enabled; an audit entry claiming a change would be noise.
    assert.equal((await service.history()).length, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("the default runtime must be a registered runtime", async () => {
  const root = await workspace();
  try {
    const registry = new AgentRuntimeRegistry();
    registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes") });
    const service = serviceFor(root, registry);

    await assert.rejects(() => service.setDefault("no-such-runtime", "api"), /not registered/);

    await service.setDefault("hermes-local", "console");
    assert.equal(service.defaultRuntimeId, "hermes-local");
    const stored = await new RuntimeConfigStore(root).load();
    assert.equal(stored.defaultRuntimeId, "hermes-local");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("the configuration survives a restart", async () => {
  const root = await workspace();
  try {
    const first = new AgentRuntimeRegistry();
    first.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes") });
    const firstService = serviceFor(root, first);
    await firstService.setEnabled("hermes-local", false, "console");
    await firstService.update("hermes-local", { priority: 8, tags: ["remote"] }, "console");
    await firstService.setDefault("hermes-local", "console");

    // A new process: new registry, new service, same data directory.
    const second = new AgentRuntimeRegistry();
    second.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes") });
    const secondService = serviceFor(root, second);
    await secondService.load();

    const entry = second.registrationOf("hermes-local")!;
    assert.equal(entry.enabled, false);
    assert.equal(entry.priority, 8);
    assert.deepEqual(entry.tags, ["remote"]);
    assert.equal(secondService.defaultRuntimeId, "hermes-local");

    // The restore itself is on the record.
    const history = await secondService.history();
    assert.equal(history[0]!.action, "restore");
    assert.equal(history[0]!.actor, "system");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a corrupt configuration is reported instead of being half applied", async () => {
  const root = await workspace();
  try {
    await mkdir(join(root, "config"), { recursive: true });
    await writeFile(join(root, "config", "runtimes.json"), "{ broken", "utf8");

    const registry = new AgentRuntimeRegistry();
    registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapter("hermes") });
    const service = serviceFor(root, registry);

    await assert.rejects(() => service.load(), RuntimeConfigError);
    // Nothing was applied: the runtime keeps its registered values.
    assert.equal(registry.registrationOf("hermes-local")!.enabled, true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("the audit log keeps every entry and reports a broken line by its number", async () => {
  const root = await workspace();
  try {
    const log = new RuntimeConfigAuditLog(root);
    await log.append({
      id: "rcfg_1",
      recordedAt: "2026-11-12T00:00:00.000Z",
      actor: "console",
      action: "disable",
      runtimeId: "hermes-local",
      changes: [{ field: "enabled", from: true, to: false }]
    });
    await log.append({
      id: "rcfg_2",
      recordedAt: "2026-11-12T00:01:00.000Z",
      actor: "console",
      action: "enable",
      runtimeId: "hermes-local",
      changes: [{ field: "enabled", from: false, to: true }]
    });

    const history = await log.list();
    assert.deepEqual(history.map(entry => entry.id), ["rcfg_2", "rcfg_1"]);

    await writeFile(log.path, `${await readFile(log.path, "utf8")}not json\n`, "utf8");
    await assert.rejects(
      () => log.list(),
      (error: unknown) => {
        assert.ok(error instanceof Error);
        assert.match(error.message, /line 3/);
        return true;
      }
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
