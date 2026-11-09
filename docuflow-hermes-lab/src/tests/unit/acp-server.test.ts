import test from "node:test";
import assert from "node:assert/strict";
import { PassThrough } from "node:stream";
import { AcpAgentServer } from "../../acp/server.js";
import { createDocuFlowAcpBackend, toSessionUpdate, toStopReason } from "../../acp/docuflow-backend.js";
import type { AcpBackend } from "../../acp/protocol.js";
import { InMemoryJournalStore } from "../../journal/journal.js";
import { ExecutionCoordinator } from "../../orchestrator/execution-coordinator.js";
import {
  NO_CAPABILITIES,
  type AgentRuntimeAdapter,
  type AgentRuntimeEvent
} from "../../types/adapter.js";

async function waitFor(condition: () => boolean, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for condition");
    await new Promise(resolve => setTimeout(resolve, 5));
  }
}

interface Harness {
  server: AcpAgentServer;
  input: PassThrough;
  frames: Array<Record<string, unknown>>;
  send(message: unknown): void;
  close(): void;
}

function harness(backend: AcpBackend): Harness {
  const input = new PassThrough();
  const output = new PassThrough();
  const frames: Array<Record<string, unknown>> = [];
  output.on("data", (chunk: Buffer) => {
    for (const line of chunk.toString("utf8").split("\n")) {
      if (line.trim()) frames.push(JSON.parse(line) as Record<string, unknown>);
    }
  });
  const server = new AcpAgentServer(backend, { input, output });
  server.start();
  return {
    server,
    input,
    frames,
    send: message => input.write(`${JSON.stringify(message)}\n`),
    close: () => server.stop()
  };
}

function fakeBackend(): AcpBackend & { cancelled: string[] } {
  const cancelled: string[] = [];
  return {
    cancelled,
    async createSession() { return { sessionId: "s1" }; },
    async prompt(_sessionId, text, emit) {
      emit({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: `echo: ${text}` } });
      return "end_turn";
    },
    async cancel(sessionId) { cancelled.push(sessionId); }
  };
}

test("acp: initialize advertises the protocol and agent identity", async () => {
  const app = harness(fakeBackend());
  try {
    app.send({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: 1 } });
    await waitFor(() => app.frames.length === 1);

    const response = app.frames[0] as { id: number; result: { protocolVersion: number; agentInfo: { name: string } } };
    assert.equal(response.id, 1);
    assert.equal(response.result.protocolVersion, 1);
    assert.equal(response.result.agentInfo.name, "docuflow");
  } finally {
    app.close();
  }
});

test("acp: session/new validates cwd and returns a session id", async () => {
  const app = harness(fakeBackend());
  try {
    app.send({ jsonrpc: "2.0", id: 2, method: "session/new", params: { cwd: "/tmp/ws" } });
    await waitFor(() => app.frames.length === 1);
    assert.deepEqual((app.frames[0] as { result: unknown }).result, { sessionId: "s1" });

    app.send({ jsonrpc: "2.0", id: 3, method: "session/new", params: {} });
    await waitFor(() => app.frames.length === 2);
    const error = app.frames[1] as { error: { message: string } };
    assert.match(error.error.message, /cwd/);
  } finally {
    app.close();
  }
});

test("acp: session/prompt streams updates and reports the stop reason", async () => {
  const app = harness(fakeBackend());
  try {
    app.send({
      jsonrpc: "2.0",
      id: 4,
      method: "session/prompt",
      params: { sessionId: "s1", prompt: [{ type: "text", text: "hello" }] }
    });
    await waitFor(() => app.frames.length === 2);

    const update = app.frames[0] as { method: string; params: { update: { sessionUpdate: string } } };
    assert.equal(update.method, "session/update");
    assert.equal(update.params.update.sessionUpdate, "agent_message_chunk");

    const response = app.frames[1] as { result: { stopReason: string } };
    assert.equal(response.result.stopReason, "end_turn");
  } finally {
    app.close();
  }
});

test("acp: cancel is a notification, so it never produces a response", async () => {
  const backend = fakeBackend();
  const app = harness(backend);
  try {
    app.send({ jsonrpc: "2.0", method: "session/cancel", params: { sessionId: "s1" } });
    await waitFor(() => backend.cancelled.length === 1);

    assert.deepEqual(backend.cancelled, ["s1"]);
    assert.equal(app.frames.length, 0);
  } finally {
    app.close();
  }
});

test("acp: unknown methods and malformed frames are reported without crashing", async () => {
  const app = harness(fakeBackend());
  try {
    app.send({ jsonrpc: "2.0", id: 9, method: "session/nope" });
    await waitFor(() => app.frames.length === 1);
    assert.match((app.frames[0] as { error: { message: string } }).error.message, /Unsupported/);

    app.input.write("{not json}\n");
    await waitFor(() => app.frames.length === 2);
    assert.equal((app.frames[1] as { error: { code: number } }).error.code, -32700);
  } finally {
    app.close();
  }
});

function stubAdapter(): AgentRuntimeAdapter & { cancelled: string[] } {
  const state = { cancelled: [] as string[], counter: 0 };
  const events: AgentRuntimeEvent[] = [
    {
      eventId: "e1",
      executionId: "exec-1",
      projectId: "proj-1",
      taskId: "task-1",
      sequence: 1,
      eventType: "message.delta",
      timestamp: "2026-11-09T00:00:00.000Z",
      payload: { text: "hello from hermes" }
    },
    {
      eventId: "e2",
      executionId: "exec-1",
      projectId: "proj-1",
      taskId: "task-1",
      sequence: 2,
      eventType: "tool.started",
      timestamp: "2026-11-09T00:00:01.000Z",
      payload: { tool: "read_file" }
    },
    {
      eventId: "e3",
      executionId: "exec-1",
      projectId: "proj-1",
      taskId: "task-1",
      sequence: 3,
      eventType: "tool.completed",
      timestamp: "2026-11-09T00:00:02.000Z",
      payload: { tool: "read_file" }
    },
    {
      eventId: "e4",
      executionId: "exec-1",
      projectId: "proj-1",
      taskId: "task-1",
      sequence: 4,
      eventType: "execution.completed",
      timestamp: "2026-11-09T00:00:03.000Z",
      payload: {}
    }
  ];
  return {
    cancelled: state.cancelled,
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() {
      state.counter += 1;
      return { executionId: "exec-1", runtimeType: "hermes", status: "running", externalRunId: "run-1" };
    },
    async getStatus() { return "running"; },
    async cancel(executionId) { state.cancelled.push(executionId); },
    async resume() { throw new Error("not used"); },
    async subscribe(_id, handler) {
      for (const event of events) handler(event);
      return async () => undefined;
    },
    async terminate() { return undefined; }
  };
}

test("acp backend: a prompt becomes an execution whose events become ACP updates", async () => {
  const adapter = stubAdapter();
  const coordinator = new ExecutionCoordinator({ adapter, journal: new InMemoryJournalStore() });
  const backend = createDocuFlowAcpBackend({
    coordinator,
    defaults: {
      projectId: "zed",
      contextSnapshotId: "ctx-zed",
      policySnapshotId: "pol-zed",
      timeoutSeconds: 5
    },
    idFactory: () => "sess-1",
    runIdFactory: () => "run-acp-1"
  });

  const session = await backend.createSession({ cwd: "C:\\work\\repo" });
  assert.equal(session.sessionId, "sess-1");

  const updates: Array<{ sessionUpdate: string }> = [];
  const stopReason = await backend.prompt(session.sessionId, "hi", update => updates.push(update as never));

  assert.equal(stopReason, "end_turn");
  assert.deepEqual(updates.map(update => update.sessionUpdate), [
    "agent_message_chunk",
    "tool_call",
    "tool_call_update"
  ]);
});

test("acp mapping: tool and terminal events map to stable update shapes", () => {
  const started = toSessionUpdate({
    eventId: "x",
    executionId: "e",
    projectId: "p",
    taskId: "t",
    sequence: 1,
    eventType: "tool.started",
    timestamp: "2026-11-09T00:00:00.000Z",
    payload: { tool: "search_code", toolCallId: "call-1" }
  });
  assert.deepEqual(started, {
    sessionUpdate: "tool_call",
    toolCallId: "call-1",
    title: "search_code",
    status: "in_progress"
  });

  assert.equal(toStopReason({
    eventId: "y",
    executionId: "e",
    projectId: "p",
    taskId: "t",
    sequence: 2,
    eventType: "execution.cancelled",
    timestamp: "2026-11-09T00:00:01.000Z",
    payload: {}
  }), "cancelled");
});

test("acp: agent requests resolve when the client answers", async () => {
  const app = harness(fakeBackend());
  try {
    const pending = app.server.request("session/request_permission", { sessionId: "s1" }, 2_000);
    await waitFor(() => app.frames.some(frame => frame["method"] === "session/request_permission"));
    const frame = app.frames.find(item => item["method"] === "session/request_permission")!;
    app.send({
      jsonrpc: "2.0",
      id: frame["id"],
      result: { outcome: { outcome: "selected", optionId: "allow" } }
    });

    assert.deepEqual(await pending, { outcome: { outcome: "selected", optionId: "allow" } });
  } finally {
    app.close();
  }
});

function permissionAdapter(options: {
  requestId?: string;
} = {}): AgentRuntimeAdapter & {
  permissions: Array<{ executionId: string; requestId: string; decision: string }>;
} {
  const permissions: Array<{ executionId: string; requestId: string; decision: string }> = [];
  const events: AgentRuntimeEvent[] = [
    {
      eventId: "p1",
      executionId: "exec-1",
      projectId: "proj-1",
      taskId: "task-1",
      sequence: 1,
      eventType: "permission.requested",
      timestamp: "2026-11-09T00:00:00.000Z",
      payload: { request_id: options.requestId ?? "req-1", tool: "shell", command: "rm -rf build" }
    },
    {
      eventId: "p2",
      executionId: "exec-1",
      projectId: "proj-1",
      taskId: "task-1",
      sequence: 2,
      eventType: "execution.completed",
      timestamp: "2026-11-09T00:00:01.000Z",
      payload: {}
    }
  ];
  return {
    permissions,
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() {
      return { executionId: "exec-1", runtimeType: "hermes", status: "running", externalRunId: "run-1" };
    },
    async getStatus() { return "running"; },
    async cancel() { return undefined; },
    async resume() { throw new Error("not used"); },
    async subscribe(_id, handler) {
      for (const event of events) handler(event);
      return async () => undefined;
    },
    async terminate() { return undefined; },
    getPendingPermissions() {
      return [{ requestId: options.requestId ?? "req-1", executionId: "exec-1", payload: {} }];
    },
    async respondToPermission(executionId, requestId, response) {
      permissions.push({ executionId, requestId, decision: response.decision });
    }
  };
}

test("acp backend: a permission event becomes a session/request_permission round-trip", async () => {
  const adapter = permissionAdapter();
  const coordinator = new ExecutionCoordinator({ adapter, journal: new InMemoryJournalStore() });
  const asked: string[] = [];
  const backend = createDocuFlowAcpBackend({
    coordinator,
    defaults: { projectId: "zed", contextSnapshotId: "ctx", policySnapshotId: "pol", timeoutSeconds: 5 },
    requestPermission: async request => {
      asked.push(request.title);
      return { optionId: "allow" };
    }
  });

  const session = await backend.createSession({ cwd: "C:\\work" });
  await backend.prompt(session.sessionId, "run it", () => undefined);

  assert.deepEqual(asked, ["shell"]);
  assert.deepEqual(adapter.permissions, [
    { executionId: "exec-1", requestId: "req-1", decision: "approve" }
  ]);
});

test("acp backend: a cancelled prompt denies instead of leaving the runtime waiting", async () => {
  const adapter = permissionAdapter();
  const coordinator = new ExecutionCoordinator({ adapter, journal: new InMemoryJournalStore() });
  const backend = createDocuFlowAcpBackend({
    coordinator,
    defaults: { projectId: "zed", contextSnapshotId: "ctx", policySnapshotId: "pol", timeoutSeconds: 5 },
    requestPermission: async () => "cancelled"
  });

  const session = await backend.createSession({ cwd: "C:\\work" });
  await backend.prompt(session.sessionId, "run it", () => undefined);

  assert.deepEqual(adapter.permissions, [
    { executionId: "exec-1", requestId: "req-1", decision: "deny" }
  ]);
});

test("acp backend: without a permission handler nothing is auto-approved", async () => {
  const adapter = permissionAdapter();
  const coordinator = new ExecutionCoordinator({ adapter, journal: new InMemoryJournalStore() });
  const backend = createDocuFlowAcpBackend({
    coordinator,
    defaults: { projectId: "zed", contextSnapshotId: "ctx", policySnapshotId: "pol", timeoutSeconds: 5 }
  });

  const session = await backend.createSession({ cwd: "C:\\work" });
  await backend.prompt(session.sessionId, "run it", () => undefined);

  assert.deepEqual(adapter.permissions, []);
});
