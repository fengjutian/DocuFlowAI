import test from "node:test";
import assert from "node:assert/strict";
import { ExecutionCoordinator, renderCheckpoint } from "../../orchestrator/execution-coordinator.js";
import { InMemoryJournalStore } from "../../journal/journal.js";
import {
  NO_CAPABILITIES,
  type AgentCapability,
  type AgentExecution,
  type AgentRuntimeAdapter,
  type AgentRuntimeEvent,
  type AgentTaskInput
} from "../../types/adapter.js";

const input: AgentTaskInput = {
  projectId: "proj-1",
  taskId: "task-1",
  runId: "run-1",
  prompt: "implement the feature",
  contextSnapshotId: "ctx-1",
  policySnapshotId: "pol-1",
  worktreeId: "wt-1",
  workingDirectory: "/tmp/wt-1",
  allowedTools: ["read_file"],
  timeoutSeconds: 30,
  correlationId: "corr-1"
};

function event(sequence: number, eventType: AgentRuntimeEvent["eventType"]): AgentRuntimeEvent {
  return {
    eventId: `evt-${sequence}`,
    executionId: "placeholder",
    projectId: "proj-1",
    taskId: "task-1",
    sequence,
    eventType,
    timestamp: "2026-11-09T00:00:00.000Z",
    payload: { hermesEvent: `run.${sequence}` }
  };
}

function makeAdapter(options: {
  events?: AgentRuntimeEvent[];
  capabilities?: Partial<AgentCapability>;
} = {}): AgentRuntimeAdapter & { starts: AgentTaskInput[]; resumed: string[]; terminated: string[] } {
  const starts: AgentTaskInput[] = [];
  const resumed: string[] = [];
  const terminated: string[] = [];
  let counter = 0;
  return {
    starts,
    resumed,
    terminated,
    async getCapabilities() {
      return { ...NO_CAPABILITIES, ...options.capabilities };
    },
    async healthCheck() {
      return true;
    },
    async start(taskInput: AgentTaskInput): Promise<AgentExecution> {
      starts.push(taskInput);
      counter += 1;
      return {
        executionId: `exec-${counter}`,
        runtimeType: "hermes",
        status: "running",
        externalRunId: `run-${counter}`
      };
    },
    async getStatus() {
      return "running";
    },
    async cancel() {
      return undefined;
    },
    async resume(executionId: string) {
      resumed.push(executionId);
      return { executionId: "exec-resumed", runtimeType: "hermes", status: "running" };
    },
    async subscribe(executionId, handler) {
      for (const item of options.events ?? []) handler({ ...item, executionId });
      return async () => undefined;
    },
    async terminate(executionId: string) {
      terminated.push(executionId);
    }
  };
}

test("journals replayed runtime events with contiguous positions and their event sequences", async () => {
  const journal = new InMemoryJournalStore();
  const coordinator = new ExecutionCoordinator({
    adapter: makeAdapter({ events: [event(1, "execution.started"), event(2, "message.delta")] }),
    journal
  });

  const execution = await coordinator.start(input);
  await coordinator.flush(execution.executionId);
  const entries = await journal.read(execution.executionId);

  assert.deepEqual(entries.map(item => item.sequence), [1, 2]);
  assert.deepEqual(entries.map(item => item.eventSequence), [1, 2]);
  assert.deepEqual(entries.map(item => item.kind), ["runtime.event", "runtime.event"]);
});

test("audit facts share the journal without looking like missing runtime events", async () => {
  const journal = new InMemoryJournalStore();
  const coordinator = new ExecutionCoordinator({
    adapter: makeAdapter({ events: [event(1, "execution.started"), event(2, "message.delta")] }),
    journal
  });

  const execution = await coordinator.start(input);
  await coordinator.recordAuditFact(execution.executionId, "change.applied", { changeRecordId: "chg_1" });

  const checkpoint = await coordinator.getCheckpoint(execution.executionId);
  const entries = await journal.read(execution.executionId);

  assert.deepEqual(entries.map(item => item.sequence), [1, 2, 3]);
  assert.equal(entries[2]!.eventSequence, undefined);
  assert.deepEqual(checkpoint.appliedChangeRecordIds, ["chg_1"]);
  assert.deepEqual(checkpoint.gaps, []);
});

test("stops recovery for an execution that already finished", async () => {
  const coordinator = new ExecutionCoordinator({
    adapter: makeAdapter({
      events: [event(1, "execution.started"), event(2, "execution.completed")]
    }),
    journal: new InMemoryJournalStore()
  });

  const execution = await coordinator.start(input);
  const result = await coordinator.recover(execution.executionId);

  assert.equal(result.plan.action, "stop");
  assert.equal(result.execution, undefined);
});

test("rebuilds a lost session with the checkpoint injected into a fresh run", async () => {
  const journal = new InMemoryJournalStore();
  const adapter = makeAdapter({ events: [event(1, "execution.started"), event(2, "message.delta")] });
  const coordinator = new ExecutionCoordinator({ adapter, journal });

  const first = await coordinator.start(input);
  const result = await coordinator.recover(first.executionId);

  assert.equal(result.plan.action, "rebuild");
  assert.equal(adapter.starts.length, 2);
  const rebuilt = adapter.starts[1]!;
  assert.notEqual(rebuilt.runId, input.runId);
  assert.match(rebuilt.prompt, /\[DOCUFLOW CHECKPOINT\]/);
  assert.match(rebuilt.prompt, new RegExp(first.executionId));

  const audit = (await journal.read(result.execution!.executionId)).find(
    entry => entry.category === "recovery.rebuild"
  );
  assert.equal(audit?.payload["previousExecutionId"], first.executionId);
});

test("compensates a journal gap before rebuilding", async () => {
  const journal = new InMemoryJournalStore();
  const adapter = makeAdapter({ events: [event(1, "execution.started"), event(3, "message.delta")] });
  let currentExecutionId = "";
  const coordinator = new ExecutionCoordinator({
    adapter,
    journal,
    compensationSource: async () => [
      { ...event(2, "message.delta"), executionId: currentExecutionId }
    ]
  });

  const execution = await coordinator.start(input);
  currentExecutionId = execution.executionId;
  const result = await coordinator.recover(execution.executionId);

  assert.equal(result.plan.action, "compensate");
  assert.equal(result.compensated, 1);
  const entries = await journal.read(execution.executionId);
  assert.deepEqual(entries.map(item => item.sequence), [1, 2, 3, 4]);
  assert.deepEqual(
    entries.map(item => item.eventSequence).filter((seq): seq is number => seq !== undefined).sort((a, b) => a - b),
    [1, 2, 3]
  );

  // After compensation the journal has no gap, so the next decision is about the session itself.
  const second = await coordinator.recover(execution.executionId);
  assert.equal(second.plan.action, "rebuild");
});

test("resumes only when the adapter advertises and confirms a live session", async () => {
  const adapter = makeAdapter({
    events: [event(1, "execution.started")],
    capabilities: { sessionResume: true }
  });
  const coordinator = new ExecutionCoordinator({
    adapter,
    journal: new InMemoryJournalStore(),
    isSessionResumable: () => true
  });

  const execution = await coordinator.start(input);
  const result = await coordinator.recover(execution.executionId);

  assert.equal(result.plan.action, "resume");
  assert.deepEqual(adapter.resumed, [execution.executionId]);
});

test("renders a checkpoint that forbids re-applying recorded changes", () => {
  const rendered = renderCheckpoint({
    executionId: "exec-1",
    lastSequence: 7,
    eventCount: 7,
    terminal: false,
    appliedChangeRecordIds: ["chg_1"],
    pendingPermissionRequestIds: ["req-1"],
    gaps: [],
    createdAt: "2026-11-09T00:00:00.000Z",
    gitBaseSha: "a".repeat(40)
  });

  assert.match(rendered, /\[DOCUFLOW CHECKPOINT\]/);
  assert.match(rendered, /chg_1/);
  assert.match(rendered, /Do not re-apply/);
});
