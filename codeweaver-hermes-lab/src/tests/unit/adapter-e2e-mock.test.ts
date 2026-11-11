import test from "node:test";
import assert from "node:assert/strict";
import { HermesAdapter } from "../../adapters/hermes-adapter.js";
import { UsageLedger } from "../../accounting/usage.js";
import { HermesHttpClient } from "../../clients/http/hermes-http-client.js";
import { InMemoryJournalStore } from "../../journal/journal.js";
import { ExecutionCoordinator } from "../../orchestrator/execution-coordinator.js";
import { startMockHermes } from "../support/mock-hermes-server.js";
import type { AgentRuntimeEvent, AgentTaskInput } from "../../types/adapter.js";

function input(runId: string, prompt = "say hello"): AgentTaskInput {
  return {
    projectId: "proj-1",
    taskId: "task-1",
    runId,
    prompt,
    contextSnapshotId: "ctx-1",
    policySnapshotId: "pol-1",
    worktreeId: "wt-1",
    workingDirectory: "C:\\tmp\\wt",
    allowedTools: ["read_file"],
    timeoutSeconds: 5,
    correlationId: "corr-1"
  };
}

async function waitFor(condition: () => boolean, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for condition");
    await new Promise(resolve => setTimeout(resolve, 10));
  }
}

test("e2e: adapter → mock HTTP → SSE → journal → checkpoint → usage", async () => {
  const server = await startMockHermes();
  try {
    const client = new HermesHttpClient({ baseUrl: server.url });
    const adapter = new HermesAdapter(client);
    const journal = new InMemoryJournalStore();
    const usage = new UsageLedger();
    const events: AgentRuntimeEvent[] = [];
    const coordinator = new ExecutionCoordinator({
      adapter,
      journal,
      onEvent: event => {
        events.push(event);
        if (event.eventType === "execution.completed") {
          void usage.recordFromEvent(event, { durationMs: 42 });
        }
      }
    });

    const execution = await coordinator.start(input("run-e2e-1"));
    assert.equal(execution.externalRunId, "run_mock_1");

    await waitFor(() => events.some(event => event.eventType === "execution.completed"));
    await coordinator.flush(execution.executionId);

    const entries = await journal.read(execution.executionId);
    assert.deepEqual(
      entries.map(entry => entry.category),
      // The journal opens with the request the execution was started with, so a worker that dies
      // can still rebuild it; then the runtime's own events follow in order.
      ["execution.input", "execution.started", "execution.started", "message.delta", "message.delta", "execution.completed"]
    );
    assert.deepEqual(
      entries.filter(entry => entry.eventSequence !== undefined).map(entry => entry.eventSequence),
      [1, 2, 3, 4, 5]
    );

    const checkpoint = await coordinator.getCheckpoint(execution.executionId);
    assert.equal(checkpoint.terminal, true);
    assert.equal(checkpoint.terminalEventType, "execution.completed");
    // Every journal entry counts, including the request the execution was started with.
    assert.equal(checkpoint.eventCount, 6);

    // A terminal execution stops recovery instead of restarting it.
    assert.equal((await coordinator.recover(execution.executionId)).plan.action, "stop");

    await waitFor(() => usage.forExecution(execution.executionId) !== undefined);
    assert.equal(usage.forExecution(execution.executionId)?.totalTokens, 15);
    assert.equal(usage.forExecution(execution.executionId)?.model, "mock-model");
  } finally {
    await server.close();
  }
});

test("e2e: cancel reaches the runtime once cancellation is declared verified", async () => {
  const server = await startMockHermes({
    // Keep the stream open so the execution is still running when cancel is called.
    events: [
      { event: "run.started", data: {}, id: "1" },
      { event: "message.delta", data: { text: "slow" }, id: "2", delayMs: 5_000 }
    ],
    runStatus: "running"
  });
  try {
    const client = new HermesHttpClient({ baseUrl: server.url });
    const adapter = new HermesAdapter(client, { verifiedCapabilities: { cancellation: true } });
    const journal = new InMemoryJournalStore();
    const coordinator = new ExecutionCoordinator({ adapter, journal });

    const execution = await coordinator.start(input("run-e2e-2"));
    await coordinator.cancel(execution.executionId);

    assert.equal(server.runs.get("run_mock_1")?.status, "cancelled");
    await coordinator.flush(execution.executionId);
    const categories = (await journal.read(execution.executionId)).map(entry => entry.category);
    assert.ok(categories.includes("execution.cancel_requested"));

    await coordinator.terminate(execution.executionId);
  } finally {
    await server.close();
  }
});
