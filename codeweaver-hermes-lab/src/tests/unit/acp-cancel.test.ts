import test from "node:test";
import assert from "node:assert/strict";
import { createCodeWeaverAcpBackend } from "../../acp/codeweaver-backend.js";
import { InMemoryJournalStore } from "../../journal/journal.js";
import { ExecutionCoordinator } from "../../orchestrator/execution-coordinator.js";
import {
  AgentAdapterError,
  NO_CAPABILITIES,
  type AgentRuntimeAdapter
} from "../../types/adapter.js";

function slowAdapter(behaviour: "cancel" | "unsupported"): AgentRuntimeAdapter & {
  cancelled: string[];
  terminated: string[];
} {
  const state = { cancelled: [] as string[], terminated: [] as string[] };
  return {
    cancelled: state.cancelled,
    terminated: state.terminated,
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() {
      return { executionId: "exec-1", runtimeType: "hermes", status: "running", externalRunId: "run-1" };
    },
    async getStatus() { return "running"; },
    async cancel(executionId) {
      if (behaviour === "unsupported") {
        throw new AgentAdapterError("unsupported_capability", "Cancellation is not verified");
      }
      state.cancelled.push(executionId);
    },
    async resume() { throw new Error("not used"); },
    async subscribe() {
      // Emits nothing terminal: the turn stays open until cancelled.
      return async () => undefined;
    },
    async terminate(executionId) { state.terminated.push(executionId); }
  };
}

async function startPrompt(adapter: AgentRuntimeAdapter, journal: InMemoryJournalStore) {
  const coordinator = new ExecutionCoordinator({ adapter, journal });
  const backend = createCodeWeaverAcpBackend({
    coordinator,
    defaults: { projectId: "zed", contextSnapshotId: "ctx", policySnapshotId: "pol", timeoutSeconds: 30 },
    idFactory: () => "sess-1",
    runIdFactory: () => "run-acp-1"
  });
  const session = await backend.createSession({ cwd: "C:\\repo" });
  const prompt = backend.prompt(session.sessionId, "long task", () => undefined);
  // Let the execution start before cancelling.
  await new Promise(resolve => setTimeout(resolve, 50));
  return { backend, session, prompt, coordinator };
}

test("acp cancel: a supported runtime stops the run and the turn ends immediately", async () => {
  const adapter = slowAdapter("cancel");
  const journal = new InMemoryJournalStore();
  const { backend, session, prompt } = await startPrompt(adapter, journal);

  await backend.cancel(session.sessionId);
  assert.equal(await prompt, "cancelled");
  assert.deepEqual(adapter.cancelled, ["exec-1"]);
  assert.equal(adapter.terminated.length, 0);
});

test("acp cancel: an unsupported runtime still ends the turn and journals why", async () => {
  const adapter = slowAdapter("unsupported");
  const journal = new InMemoryJournalStore();
  const { backend, session, prompt } = await startPrompt(adapter, journal);

  await backend.cancel(session.sessionId);
  assert.equal(await prompt, "cancelled");
  assert.deepEqual(adapter.terminated, ["exec-1"]);

  const categories = (await journal.read("exec-1")).map(entry => entry.category);
  assert.ok(categories.includes("execution.cancel_unsupported"));
});
