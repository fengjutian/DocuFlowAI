import test from "node:test";
import assert from "node:assert/strict";
import { PassThrough } from "node:stream";
import { HermesAdapter } from "../../adapters/hermes-adapter.js";
import { createDocuFlowAcpBackend } from "../../acp/docuflow-backend.js";
import { AcpAgentServer } from "../../acp/server.js";
import { HermesHttpClient } from "../../clients/http/hermes-http-client.js";
import { InMemoryJournalStore } from "../../journal/journal.js";
import { ExecutionCoordinator } from "../../orchestrator/execution-coordinator.js";
import { startMockHermes } from "../support/mock-hermes-server.js";

async function waitFor(condition: () => boolean, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for condition");
    await new Promise(resolve => setTimeout(resolve, 10));
  }
}

test("e2e: Zed-style ACP prompt reaches mock Hermes and streams updates back", async () => {
  const hermes = await startMockHermes({
    events: [
      { event: "run.started", data: {}, id: "1" },
      { event: "message.delta", data: { text: "hello " }, id: "2" },
      { event: "tool.started", data: { tool: "read_file", toolCallId: "call-1" }, id: "3" },
      { event: "tool.completed", data: { tool: "read_file", toolCallId: "call-1" }, id: "4" },
      { event: "run.completed", data: { output: "done" }, id: "5" }
    ]
  });
  const input = new PassThrough();
  const output = new PassThrough();
  const frames: Array<Record<string, unknown>> = [];
  output.on("data", (chunk: Buffer) => {
    for (const line of chunk.toString("utf8").split("\n")) {
      if (line.trim()) frames.push(JSON.parse(line) as Record<string, unknown>);
    }
  });

  const journal = new InMemoryJournalStore();
  const coordinator = new ExecutionCoordinator({
    adapter: new HermesAdapter(new HermesHttpClient({ baseUrl: hermes.url })),
    journal
  });
  const backend = createDocuFlowAcpBackend({
    coordinator,
    defaults: {
      projectId: "zed",
      contextSnapshotId: "ctx-zed",
      policySnapshotId: "pol-zed",
      timeoutSeconds: 10
    },
    idFactory: () => "sess-1",
    runIdFactory: () => "run-acp-1"
  });
  const server = new AcpAgentServer(backend, { input, output });

  try {
    server.start();
    input.write(`${JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: 1 } })}\n`);
    await waitFor(() => frames.some(frame => frame["id"] === 1));

    input.write(`${JSON.stringify({ jsonrpc: "2.0", id: 2, method: "session/new", params: { cwd: "C:\\repo" } })}\n`);
    await waitFor(() => frames.some(frame => frame["id"] === 2));
    const session = (frames.find(frame => frame["id"] === 2) as { result: { sessionId: string } }).result;
    assert.equal(session.sessionId, "sess-1");

    input.write(`${JSON.stringify({
      jsonrpc: "2.0",
      id: 3,
      method: "session/prompt",
      params: { sessionId: "sess-1", prompt: [{ type: "text", text: "say hello" }] }
    })}\n`);

    await waitFor(() => frames.some(frame => frame["id"] === 3));
    const promptResponse = frames.find(frame => frame["id"] === 3) as { result: { stopReason: string } };
    assert.equal(promptResponse.result.stopReason, "end_turn");

    const updates = frames
      .filter(frame => frame["method"] === "session/update")
      .map(frame => (frame["params"] as { update: { sessionUpdate: string } }).update.sessionUpdate);
    assert.ok(updates.includes("agent_message_chunk"));
    assert.ok(updates.includes("tool_call"));
    assert.ok(updates.includes("tool_call_update"));

    // The editor-driven run is a first-class execution: it is journaled like any other.
    const executions = await journal.listExecutions();
    assert.equal(executions.length, 1);
    const entries = await journal.read(executions[0]!);
    assert.ok(entries.some(entry => entry.category === "execution.completed"));
    assert.ok(entries.some(entry => entry.category === "message.delta"));
  } finally {
    server.stop();
    await coordinator.stopAll();
    await hermes.close();
  }
});
