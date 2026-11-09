import test from "node:test";
import assert from "node:assert/strict";
import { HermesAdapter, type HermesAdapterClient } from "../../adapters/hermes-adapter.js";
import { AgentAdapterError, type AgentRuntimeEvent, type AgentTaskInput } from "../../types/adapter.js";
import type { HermesRun, HermesRunCreateRequest, HermesSseEvent } from "../../types/hermes.js";

interface FakeClient extends HermesAdapterClient {
  calls: string[];
  lastCreate: HermesRunCreateRequest | undefined;
  runStatus: string;
  streamError: Error | undefined;
}

function makeClient(events: HermesSseEvent[]): FakeClient {
  const client: FakeClient = {
    calls: [],
    lastCreate: undefined,
    runStatus: "running",
    streamError: undefined,
    async capabilities() {
      return { platform: "hermes-agent", features: { run_submission: true, run_events_sse: true } };
    },
    async createRun(payload: HermesRunCreateRequest): Promise<HermesRun> {
      client.lastCreate = payload;
      client.calls.push("createRun");
      return { run_id: "run-1", status: "queued" };
    },
    async getRun(runId: string): Promise<HermesRun> {
      client.calls.push(`getRun:${runId}`);
      return { run_id: runId, status: client.runStatus };
    },
    async stopRun(runId: string) {
      client.calls.push(`stopRun:${runId}`);
      return {};
    },
    async *streamRunEvents(): AsyncGenerator<HermesSseEvent> {
      for (const event of events) yield event;
      if (client.streamError) throw client.streamError;
    }
  };
  return client;
}

const input: AgentTaskInput = {
  projectId: "proj-1",
  taskId: "task-1",
  runId: "run-1",
  prompt: "Reply with DOCUFLOW_OK",
  contextSnapshotId: "ctx-1",
  policySnapshotId: "pol-1",
  worktreeId: "wt-1",
  workingDirectory: "/tmp/wt-1",
  allowedTools: ["read_file"],
  timeoutSeconds: 5,
  correlationId: "corr-1"
};

async function waitFor(condition: () => boolean, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for condition");
    await new Promise(resolve => setTimeout(resolve, 5));
  }
}

test("starts a Hermes run and correlates it with DocuFlow identifiers", async () => {
  const client = makeClient([]);
  const adapter = new HermesAdapter(client, { now: () => new Date("2026-11-09T00:00:00.000Z") });

  const execution = await adapter.start(input);

  assert.equal(execution.runtimeType, "hermes");
  assert.equal(execution.externalRunId, "run-1");
  assert.match(execution.executionId, /^exec_/);
  assert.deepEqual(client.lastCreate, {
    input: "Reply with DOCUFLOW_OK",
    metadata: {
      project_id: "proj-1",
      task_id: "task-1",
      docuflow_run_id: "run-1",
      correlation_id: "corr-1"
    }
  });
  assert.ok(client.calls.includes("createRun"));
});

test("normalizes streamed events in order and reaches the terminal status", async () => {
  const client = makeClient([
    { event: "run.started", data: { run_id: "run-1" }, id: "1" },
    { event: "message.delta", data: { text: "hel" }, id: "2" },
    { event: "tool.started", data: { tool: "read_file" }, id: "3" },
    { event: "run.completed", data: { output: "done" }, id: "4" }
  ]);
  const adapter = new HermesAdapter(client);

  const execution = await adapter.start(input);
  const received: AgentRuntimeEvent[] = [];
  await adapter.subscribe(execution.executionId, event => received.push(event));
  await waitFor(() => received.some(item => item.eventType === "execution.completed"));

  assert.deepEqual(received.map(item => item.sequence), [1, 2, 3, 4, 5]);
  assert.deepEqual(received.map(item => item.eventType), [
    "execution.started",
    "execution.started",
    "message.delta",
    "tool.started",
    "execution.completed"
  ]);
  assert.equal(await adapter.getStatus(execution.executionId), "succeeded");
  assert.equal(received[3]!.payload["tool"], "read_file");
  assert.equal(received.every(item => item.executionId === execution.executionId), true);
});

test("deduplicates redelivered events by their external id", async () => {
  const client = makeClient([
    { event: "message.delta", data: { text: "a" }, id: "dup" },
    { event: "message.delta", data: { text: "a" }, id: "dup" },
    { event: "run.completed", data: {}, id: "done" }
  ]);
  const adapter = new HermesAdapter(client);

  const execution = await adapter.start(input);
  const received: AgentRuntimeEvent[] = [];
  await adapter.subscribe(execution.executionId, event => received.push(event));
  await waitFor(() => received.some(item => item.eventType === "execution.completed"));

  const deltas = received.filter(item => item.eventType === "message.delta");
  assert.equal(deltas.length, 1);
});

test("replays buffered events to a subscriber that joins after completion", async () => {
  const client = makeClient([
    { event: "message.delta", data: { text: "hi" }, id: "1" },
    { event: "run.completed", data: {}, id: "2" }
  ]);
  const adapter = new HermesAdapter(client);
  const execution = await adapter.start(input);

  const first: AgentRuntimeEvent[] = [];
  await adapter.subscribe(execution.executionId, event => first.push(event));
  await waitFor(() => first.some(item => item.eventType === "execution.completed"));

  const replay: AgentRuntimeEvent[] = [];
  const unsubscribe = await adapter.subscribe(execution.executionId, event => replay.push(event));
  await unsubscribe();

  assert.deepEqual(replay.map(item => item.eventId), first.map(item => item.eventId));
});

test("fails the execution when the stream closes without a terminal event", async () => {
  const client = makeClient([{ event: "message.delta", data: { text: "hi" } }]);
  const adapter = new HermesAdapter(client);

  const execution = await adapter.start(input);
  const received: AgentRuntimeEvent[] = [];
  await adapter.subscribe(execution.executionId, event => received.push(event));
  await waitFor(() => received.some(item => item.eventType === "execution.failed"));

  const failure = received.find(item => item.eventType === "execution.failed");
  assert.equal(failure?.payload["reason"], "stream_closed");
  assert.equal(await adapter.getStatus(execution.executionId), "failed");
});

test("fails the execution when the event stream errors mid-run", async () => {
  const client = makeClient([{ event: "message.delta", data: { text: "hi" } }]);
  client.streamError = new Error("connection reset");
  const adapter = new HermesAdapter(client);

  const execution = await adapter.start(input);
  const received: AgentRuntimeEvent[] = [];
  await adapter.subscribe(execution.executionId, event => received.push(event));
  await waitFor(() => received.some(item => item.eventType === "execution.failed"));

  const failure = received.find(item => item.eventType === "execution.failed");
  assert.equal(failure?.payload["reason"], "stream_error");
  assert.match(String(failure?.payload["message"]), /connection reset/);
});

test("refuses to cancel until cancellation is proven by Phase 0 evidence", async () => {
  const client = makeClient([]);
  const adapter = new HermesAdapter(client);
  const execution = await adapter.start(input);

  await assert.rejects(adapter.cancel(execution.executionId), (error: unknown) => {
    assert.ok(error instanceof AgentAdapterError);
    assert.equal(error.code, "unsupported_capability");
    assert.match(error.message, /P0-018/);
    return true;
  });
  assert.equal(client.calls.includes("stopRun:run-1"), false);
});

test("cancels through the runtime once cancellation is declared verified", async () => {
  const client = makeClient([]);
  const adapter = new HermesAdapter(client, { verifiedCapabilities: { cancellation: true } });
  const execution = await adapter.start(input);

  await adapter.cancel(execution.executionId);

  assert.ok(client.calls.includes("stopRun:run-1"));
});

test("reports resume as unsupported instead of faking a session", async () => {
  const client = makeClient([]);
  const adapter = new HermesAdapter(client);
  const execution = await adapter.start(input);

  await assert.rejects(adapter.resume(execution.executionId), (error: unknown) => {
    assert.ok(error instanceof AgentAdapterError);
    assert.equal(error.code, "unsupported_capability");
    return true;
  });
});

test("reports health from the capabilities endpoint", async () => {
  const client = makeClient([]);
  const adapter = new HermesAdapter(client);
  assert.equal(await adapter.healthCheck(), true);

  client.capabilities = async () => { throw new Error("ECONNREFUSED"); };
  assert.equal(await adapter.healthCheck(), false);
  await assert.rejects(adapter.getCapabilities(), (error: unknown) => {
    assert.ok(error instanceof AgentAdapterError);
    assert.equal(error.code, "runtime_unreachable");
    return true;
  });
});

test("rejects a second execution for a run that is still active", async () => {
  const client = makeClient([]);
  const adapter = new HermesAdapter(client);
  await adapter.start(input);

  await assert.rejects(adapter.start(input), (error: unknown) => {
    assert.ok(error instanceof AgentAdapterError);
    assert.equal(error.code, "invalid_state");
    return true;
  });
});

test("surfaces an unknown execution instead of guessing", async () => {
  const client = makeClient([]);
  const adapter = new HermesAdapter(client);

  await assert.rejects(adapter.getStatus("exec_missing"), (error: unknown) => {
    assert.ok(error instanceof AgentAdapterError);
    assert.equal(error.code, "execution_not_found");
    return true;
  });
});
