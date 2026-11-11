import test from "node:test";
import assert from "node:assert/strict";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import { executeAdapterPhase1 } from "../../runners/adapter-phase1.js";
import {
  NO_CAPABILITIES,
  type AgentExecutionStatus,
  type AgentRuntimeAdapter,
  type AgentRuntimeEvent,
  type AgentTaskInput
} from "../../types/adapter.js";

const input: AgentTaskInput = {
  projectId: "proj-1",
  taskId: "task-1",
  runId: "run-1",
  prompt: "hi",
  contextSnapshotId: "ctx-1",
  policySnapshotId: "pol-1",
  worktreeId: "wt-1",
  workingDirectory: "/tmp/wt-1",
  allowedTools: [],
  timeoutSeconds: 5,
  correlationId: "corr-1"
};

function event(sequence: number, eventType: AgentRuntimeEvent["eventType"]): AgentRuntimeEvent {
  return {
    eventId: `exec-1:${sequence}`,
    executionId: "exec-1",
    projectId: "proj-1",
    taskId: "task-1",
    sequence,
    eventType,
    timestamp: "2026-11-09T00:00:00.000Z",
    payload: { hermesEvent: "run.event" }
  };
}

function stubAdapter(options: {
  events?: AgentRuntimeEvent[];
  healthy?: boolean;
  failStart?: boolean;
  status?: AgentExecutionStatus;
} = {}): AgentRuntimeAdapter {
  const events = options.events ?? [event(1, "execution.started"), event(2, "execution.completed")];
  return {
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return options.healthy ?? true; },
    async start() {
      if (options.failStart) throw new Error("connection refused");
      return { executionId: "exec-1", runtimeType: "hermes", status: "running", externalRunId: "run-1" };
    },
    async getStatus() { return options.status ?? "succeeded"; },
    async cancel() { throw new Error("not used"); },
    async resume() { throw new Error("not used"); },
    async subscribe(_executionId, handler) {
      for (const item of events) handler(item);
      return async () => undefined;
    },
    async terminate() { return undefined; }
  };
}

function registryWith(adapter: AgentRuntimeAdapter): AgentRuntimeRegistry {
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter });
  return registry;
}

test("passes AT-001..AT-004 against a healthy runtime that streams to a terminal state", async () => {
  const results = await executeAdapterPhase1({
    registry: registryWith(stubAdapter()),
    runtimeId: "hermes-local",
    taskInput: input,
    timeoutMs: 500
  });

  assert.deepEqual(results.map(item => [item.id, item.status]), [
    ["AT-001", "passed"],
    ["AT-002", "passed"],
    ["AT-003", "passed"],
    ["AT-004", "passed"]
  ]);
  assert.ok(results[1]!.evidence.some(item => /run-1/.test(item)));
  assert.ok(results[3]!.evidence.some(item => /execution\.completed/.test(item)));
});

test("fails AT-001 when the runtime health check reports unhealthy", async () => {
  const results = await executeAdapterPhase1({
    registry: registryWith(stubAdapter({ healthy: false })),
    runtimeId: "hermes-local",
    taskInput: input,
    timeoutMs: 500
  });

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /returned false/);
  assert.equal(results[1]!.status, "passed");
});

test("blocks the event checks when the runtime cannot start the task", async () => {
  const results = await executeAdapterPhase1({
    registry: registryWith(stubAdapter({ failStart: true })),
    runtimeId: "hermes-local",
    taskInput: input,
    timeoutMs: 200
  });

  assert.deepEqual(results.map(item => [item.id, item.status]), [
    ["AT-001", "passed"],
    ["AT-002", "failed"],
    ["AT-003", "blocked"],
    ["AT-004", "blocked"]
  ]);
  assert.match(results[1]!.notes ?? "", /connection refused/);
});

test("fails the event checks when no runtime event arrives before the timeout", async () => {
  const results = await executeAdapterPhase1({
    registry: registryWith(stubAdapter({ events: [] })),
    runtimeId: "hermes-local",
    taskInput: input,
    timeoutMs: 30
  });

  assert.equal(results[2]!.status, "failed");
  assert.match(results[2]!.notes ?? "", /no runtime event/);
  assert.equal(results[3]!.status, "failed");
  assert.match(results[3]!.notes ?? "", /no terminal event/);
});

test("fails AT-004 when a terminal event arrives but the status is not terminal", async () => {
  const results = await executeAdapterPhase1({
    registry: registryWith(stubAdapter({ status: "running" })),
    runtimeId: "hermes-local",
    taskInput: input,
    timeoutMs: 200
  });

  assert.equal(results[3]!.status, "failed");
  assert.ok(results[3]!.evidence.some(item => /status=running/.test(item)));
});
