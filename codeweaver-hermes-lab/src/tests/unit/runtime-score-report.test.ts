import test from "node:test";
import assert from "node:assert/strict";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import {
  collectRuntimeScores,
  exercisedCapabilities,
  observedCapabilities
} from "../../adapters/runtime-score-report.js";
import { observationsFromJournal } from "../../adapters/runtime-scoring.js";
import { InMemoryJournalStore, type JournalEntry } from "../../journal/journal.js";
import {
  NO_CAPABILITIES,
  type AgentCapability,
  type AgentRuntimeAdapter
} from "../../types/adapter.js";

/**
 * The collector reads the journal and the registry; nothing else. These tests therefore build a
 * journal by hand and a registry with stub adapters, and check that what comes out is what the
 * records say.
 */

function adapterWith(capabilities: AgentCapability): AgentRuntimeAdapter {
  return {
    async getCapabilities() { return capabilities; },
    async healthCheck() { return true; },
    async start() {
      return { executionId: "exec-x", runtimeType: "native", status: "running" };
    },
    async getStatus() { return "succeeded"; },
    async cancel() {},
    async resume() { throw new Error("not used"); },
    async subscribe() { return async () => undefined; },
    async terminate() {}
  };
}

function runtimeEvent(
  executionId: string,
  sequence: number,
  category: string,
  payload: Record<string, unknown> = {},
  recordedAt = "2026-11-11T00:00:00.000Z"
): JournalEntry {
  return {
    executionId,
    sequence,
    eventSequence: sequence,
    kind: "runtime.event",
    category,
    recordedAt,
    payload
  };
}

function auditFact(
  executionId: string,
  sequence: number,
  category: string,
  payload: Record<string, unknown> = {},
  recordedAt = "2026-11-11T00:00:00.000Z"
): JournalEntry {
  return { executionId, sequence, kind: "audit", category, recordedAt, payload };
}

/** One finished run of `runtimeId` that streamed, used a tool and succeeded. */
function aSuccessfulRun(executionId: string, runtimeId: string): JournalEntry[] {
  return [
    auditFact(executionId, 1, "execution.runtime", { runtimeId }),
    auditFact(executionId, 2, "execution.input", { allowedTools: ["read_file"] }),
    runtimeEvent(executionId, 3, "execution.started", {}, "2026-11-11T00:00:01.000Z"),
    runtimeEvent(executionId, 4, "message.delta", { delta: "hi" }, "2026-11-11T00:00:02.000Z"),
    runtimeEvent(executionId, 5, "tool.started", { toolName: "read_file" }, "2026-11-11T00:00:03.000Z"),
    runtimeEvent(executionId, 6, "execution.completed", {}, "2026-11-11T00:00:09.000Z")
  ];
}

test("a runtime that ran and succeeded scores above one that never ran", async () => {
  const journal = new InMemoryJournalStore();
  for (const entry of aSuccessfulRun("exec-1", "pi-local")) await journal.append(entry);

  const registry = new AgentRuntimeRegistry();
  registry.register({
    id: "pi-local",
    runtimeType: "native",
    adapter: adapterWith({ ...NO_CAPABILITIES, streaming: true, toolEvents: true }),
    version: "MiniMax-M3"
  });
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter: adapterWith({ ...NO_CAPABILITIES }) });

  const report = await collectRuntimeScores({ registry, journal });

  const pi = report.runtimes.find(item => item.runtimeId === "pi-local");
  const hermes = report.runtimes.find(item => item.runtimeId === "hermes-local");
  assert.ok(pi);
  assert.ok(hermes);
  assert.equal(pi.metrics.executions, 1);
  assert.equal(pi.metrics.succeeded, 1);
  assert.equal(pi.metrics.successRate, 1);
  assert.ok((pi.score ?? 0) > 0);
  // A runtime that never ran has no evidence, so it has no score — not a zero.
  assert.equal(hermes.score, null);
  assert.equal(hermes.metrics.executions, 0);
  // The comparison ranks the proven runtime first.
  assert.equal(report.comparison.rows[0]?.runtimeId, "pi-local");
  assert.equal(report.comparison.rows[0]?.version, "MiniMax-M3");
});

test("a capability the task never exercised is not held against the runtime", async () => {
  const journal = new InMemoryJournalStore();
  await journal.append(auditFact("exec-1", 1, "execution.runtime", { runtimeId: "pi-local" }));
  // The task bound no tools, so toolEvents never had a chance to show up.
  await journal.append(auditFact("exec-1", 2, "execution.input", { allowedTools: [] }));
  await journal.append(runtimeEvent("exec-1", 3, "execution.started", {}, "2026-11-11T00:00:01.000Z"));
  await journal.append(runtimeEvent("exec-1", 4, "message.delta", { delta: "hi" }, "2026-11-11T00:00:02.000Z"));
  await journal.append(runtimeEvent("exec-1", 5, "execution.completed", {}, "2026-11-11T00:00:05.000Z"));

  const registry = new AgentRuntimeRegistry();
  registry.register({
    id: "pi-local",
    runtimeType: "native",
    adapter: adapterWith({ ...NO_CAPABILITIES, streaming: true, toolEvents: true })
  });

  const report = await collectRuntimeScores({ registry, journal });
  const pi = report.runtimes.find(item => item.runtimeId === "pi-local");

  assert.deepEqual(pi?.metrics.declaredCapabilities.sort(), ["streaming", "toolEvents"]);
  // toolEvents was never put in play, so it is not part of the realization ratio.
  assert.deepEqual(pi?.metrics.realizedCapabilities, ["streaming"]);
  assert.equal(pi?.dimensions.find(item => item.key === "capabilityRealization")?.score, 1);
});

test("a capability the runtime showed but never declared is reported", async () => {
  const journal = new InMemoryJournalStore();
  for (const entry of aSuccessfulRun("exec-1", "hermes-local")) await journal.append(entry);

  const registry = new AgentRuntimeRegistry();
  registry.register({
    id: "hermes-local",
    runtimeType: "hermes",
    adapter: adapterWith({ ...NO_CAPABILITIES, streaming: true })
  });

  const report = await collectRuntimeScores({ registry, journal });
  const hermes = report.runtimes.find(item => item.runtimeId === "hermes-local");

  assert.deepEqual(hermes?.metrics.observedCapabilities.sort(), ["streaming", "toolEvents"]);
  assert.deepEqual(hermes?.metrics.observedNotDeclared, ["toolEvents"]);
});

test("executions that named no runtime are counted but attributed to nobody", async () => {
  const journal = new InMemoryJournalStore();
  await journal.append(auditFact("exec-1", 1, "execution.runtime", { runtimeId: "pi-local" }));
  await journal.append(runtimeEvent("exec-1", 2, "execution.started", {}, "2026-11-11T00:00:01.000Z"));
  await journal.append(runtimeEvent("exec-1", 3, "execution.completed", {}, "2026-11-11T00:00:04.000Z"));
  await journal.append(runtimeEvent("exec-2", 1, "execution.started", {}, "2026-11-11T00:00:05.000Z"));
  await journal.append(runtimeEvent("exec-2", 2, "execution.completed", {}, "2026-11-11T00:00:06.000Z"));

  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "pi-local", runtimeType: "native", adapter: adapterWith({ ...NO_CAPABILITIES }) });

  const report = await collectRuntimeScores({ registry, journal });

  assert.equal(report.executionsInJournal, 2);
  assert.equal(report.unattributedExecutions, 1);
  const pi = report.runtimes.find(item => item.runtimeId === "pi-local");
  assert.equal(pi?.metrics.executions, 1);
});

test("a runtime the journal remembers but the registry does not have is still reported", async () => {
  const journal = new InMemoryJournalStore();
  for (const entry of aSuccessfulRun("exec-1", "old-runtime")) await journal.append(entry);

  const registry = new AgentRuntimeRegistry();
  const report = await collectRuntimeScores({ registry, journal });

  const old = report.runtimes.find(item => item.runtimeId === "old-runtime");
  assert.ok(old);
  assert.equal(old.runtimeType, "external");
  assert.equal(old.metrics.executions, 1);
});

test("a runtime that cannot answer the capability question claims nothing", async () => {
  const journal = new InMemoryJournalStore();
  const registry = new AgentRuntimeRegistry();
  registry.register({
    id: "broken-local",
    runtimeType: "external",
    adapter: {
      ...adapterWith({ ...NO_CAPABILITIES }),
      async getCapabilities() { throw new Error("the runtime is gone"); }
    }
  });

  const report = await collectRuntimeScores({ registry, journal });
  const broken = report.runtimes.find(item => item.runtimeId === "broken-local");

  assert.deepEqual(broken?.metrics.declaredCapabilities, []);
});

test("what the workload exercised is read from CodeWeaver's own records", () => {
  const entries: JournalEntry[] = [
    auditFact("exec-1", 1, "execution.input", { allowedTools: ["read_file", "some_runtime_tool"] }),
    auditFact("exec-1", 2, "skill.bound", { skillId: "greeting-protocol" }),
    auditFact("exec-1", 3, "subagent.spawned", { childExecutionId: "exec-2" }),
    auditFact("exec-1", 4, "execution.cancel_requested", {}),
    runtimeEvent("exec-1", 5, "execution.started", {}, "2026-11-11T00:00:01.000Z"),
    runtimeEvent("exec-1", 6, "execution.started", {}, "2026-11-11T00:00:02.000Z")
  ];

  const exercised = exercisedCapabilities(entries);

  assert.equal(exercised.streaming, true);
  assert.equal(exercised.toolEvents, true);
  // `some_runtime_tool` is not one of CodeWeaver's own tools, so the policy had to gate it.
  assert.equal(exercised.permissionEvents, true);
  assert.equal(exercised.skills, true);
  assert.equal(exercised.subAgents, true);
  assert.equal(exercised.cancellation, true);
  // A second `execution.started` for the same execution is a resume.
  assert.equal(exercised.sessionResume, true);
});

test("a self-enforced tool alone cannot raise a permission request", () => {
  const exercised = exercisedCapabilities([
    auditFact("exec-1", 1, "execution.input", { allowedTools: ["read_file"] })
  ]);

  assert.equal(exercised.toolEvents, true);
  assert.equal(exercised.permissionEvents, undefined);
});

test("observed capabilities come from what the runtime emitted", () => {
  const observations = observationsFromJournal([
    auditFact("exec-1", 1, "execution.runtime", { runtimeId: "pi-local" }),
    runtimeEvent("exec-1", 2, "message.delta", { delta: "hi" }),
    runtimeEvent("exec-1", 3, "tool.started", { toolName: "read_file" }),
    runtimeEvent("exec-1", 4, "permission.requested", { requestId: "perm-1" }),
    runtimeEvent("exec-1", 5, "execution.cancelled", { runtimeConfirmed: true, runtimeStopReason: "aborted" })
  ]);

  const observed = observedCapabilities(observations);

  assert.equal(observed.streaming, true);
  assert.equal(observed.toolEvents, true);
  assert.equal(observed.permissionEvents, true);
  assert.equal(observed.cancellation, true);
  assert.equal(observed.sessionResume, undefined);
});
