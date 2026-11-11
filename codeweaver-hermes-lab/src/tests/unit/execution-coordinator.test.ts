import test from "node:test";
import assert from "node:assert/strict";
import { ExecutionCoordinator, renderCheckpoint } from "../../orchestrator/execution-coordinator.js";
import { journaledRuntimeId, journaledTaskInput } from "../../server/runtime.js";
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
  idPrefix?: string;
  /** Whether this adapter still holds the session behind an execution. */
  sessionResumable?: boolean;
} = {}): AgentRuntimeAdapter & { starts: AgentTaskInput[]; resumed: string[]; terminated: string[]; cancelled: string[] } {
  const starts: AgentTaskInput[] = [];
  const resumed: string[] = [];
  const terminated: string[] = [];
  const cancelled: string[] = [];
  const prefix = options.idPrefix ?? "exec";
  let counter = 0;
  return {
    starts,
    resumed,
    terminated,
    cancelled,
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
        executionId: `${prefix}-${counter}`,
        runtimeType: "hermes",
        status: "running",
        externalRunId: `run-${counter}`
      };
    },
    async getStatus() {
      return "running";
    },
    async cancel(executionId: string) {
      cancelled.push(executionId);
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
    },
    isSessionResumable() {
      return options.sessionResumable === true;
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
  const runtimeEvents = entries.filter(entry => entry.kind === "runtime.event");

  // The journal also carries the request the execution was started with, so the runtime events
  // start after it; what has to hold is that they are contiguous and keep their own numbering.
  assert.deepEqual(runtimeEvents.map(item => item.eventSequence), [1, 2]);
  const positions = runtimeEvents.map(item => item.sequence);
  assert.deepEqual(positions, positions.map((_, index) => positions[0]! + index));
  assert.deepEqual(entries.map(item => item.sequence), entries.map((_, index) => index + 1));
  assert.equal(entries.some(entry => entry.category === "execution.input"), true);
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
  const change = entries.find(entry => entry.category === "change.applied");

  assert.deepEqual(entries.map(item => item.sequence), entries.map((_, index) => index + 1));
  assert.equal(change?.eventSequence, undefined);
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
  assert.match(rebuilt.prompt, /\[CODEWEAVER CHECKPOINT\]/);
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
  assert.deepEqual(entries.map(item => item.sequence), entries.map((_, index) => index + 1));
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

test("asks the runtime that owns the execution whether its session survived", async () => {
  const alive = makeAdapter({
    events: [event(1, "execution.started")],
    capabilities: { sessionResume: true },
    sessionResumable: true
  });
  const coordinator = new ExecutionCoordinator({ adapter: alive, journal: new InMemoryJournalStore() });

  const execution = await coordinator.start(input);
  const result = await coordinator.recover(execution.executionId);

  // No injected answer: the owning adapter is the one that can say whether its session is there.
  assert.equal(result.plan.action, "resume");
  assert.deepEqual(alive.resumed, [execution.executionId]);
});

test("rebuilds when the owning runtime no longer holds the session", async () => {
  const gone = makeAdapter({
    events: [event(1, "execution.started")],
    capabilities: { sessionResume: true },
    sessionResumable: false
  });
  const coordinator = new ExecutionCoordinator({ adapter: gone, journal: new InMemoryJournalStore() });

  const execution = await coordinator.start(input);
  const result = await coordinator.recover(execution.executionId);

  assert.equal(result.plan.action, "rebuild");
  assert.deepEqual(gone.resumed, []);
});

test("journals the runtime and the request so a later worker can rebuild", async () => {
  const journal = new InMemoryJournalStore();
  const coordinator = new ExecutionCoordinator({
    adapter: makeAdapter(),
    journal,
    runtimeIdOf: () => "pi-local"
  });

  const execution = await coordinator.start(input);
  const entries = await journal.read(execution.executionId);
  const runtimeFact = entries.find(entry => entry.category === "execution.runtime");
  const inputFact = entries.find(entry => entry.category === "execution.input");

  assert.equal(runtimeFact?.payload["runtimeId"], "pi-local");
  assert.equal(inputFact?.payload["prompt"], input.prompt);
  assert.equal(inputFact?.payload["taskId"], input.taskId);
  assert.equal(inputFact?.payload["workingDirectory"], input.workingDirectory);
});

test("a restarted worker rebuilds on the runtime that ran the task", async () => {
  const journal = new InMemoryJournalStore();
  // The runtime that ran the task, and the server's default runtime: they are not the same.
  const owning = makeAdapter({ idPrefix: "owning", events: [event(1, "execution.started")] });
  const fallback = makeAdapter({ idPrefix: "fallback" });
  const runtimes = new Map<string, AgentRuntimeAdapter>([
    ["owning-local", owning],
    ["hermes-local", fallback]
  ]);

  const wireRecovery = (defaultAdapter: AgentRuntimeAdapter) =>
    new ExecutionCoordinator({
      adapter: defaultAdapter,
      journal,
      runtimeIdOf: adapter => (adapter === owning ? "owning-local" : "hermes-local"),
      recoveringAdapter: async executionId => {
        const runtimeId = await journaledRuntimeId(journal, executionId);
        return runtimeId ? runtimes.get(runtimeId) : undefined;
      },
      recoveryInput: checkpoint => journaledTaskInput(journal, checkpoint.executionId)
    });

  // The first worker starts the task on the owning runtime and then dies.
  const first = wireRecovery(fallback);
  const execution = await first.start(input, owning);

  // The restarted worker has the journal and nothing else.
  const restarted = wireRecovery(fallback);
  const result = await restarted.recover(execution.executionId);

  assert.equal(result.plan.action, "rebuild");
  // The rebuild continues on the runtime that ran the original task, not on the default one.
  assert.equal(owning.starts.length, 2);
  assert.equal(fallback.starts.length, 0);
  const rebuilt = owning.starts[1]!;
  assert.match(rebuilt.prompt, /\[CODEWEAVER CHECKPOINT\]/);
  assert.match(rebuilt.prompt, new RegExp(execution.executionId));
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

  assert.match(rendered, /\[CODEWEAVER CHECKPOINT\]/);
  assert.match(rendered, /chg_1/);
  assert.match(rendered, /Do not re-apply/);
});

test("an execution runs on the adapter that started it", async () => {
  const journal = new InMemoryJournalStore();
  const defaultAdapter = makeAdapter({ events: [event(1, "execution.started")], idPrefix: "default" });
  const otherAdapter = makeAdapter({ events: [event(1, "execution.started")], idPrefix: "other" });
  const coordinator = new ExecutionCoordinator({ adapter: defaultAdapter, journal });

  const first = await coordinator.start(input);
  const second = await coordinator.start(input, otherAdapter);

  assert.equal(coordinator.adapterFor(first.executionId), defaultAdapter);
  assert.equal(coordinator.adapterFor(second.executionId), otherAdapter);
  assert.equal(coordinator.getAdapter(), defaultAdapter, "the default adapter is unchanged");
  assert.equal(
    coordinator.adapterFor("exec-unknown"),
    defaultAdapter,
    "an unknown execution falls back to the default adapter"
  );
});

test("a cancel reaches the runtime that owns the execution", async () => {
  const journal = new InMemoryJournalStore();
  const defaultAdapter = makeAdapter({ idPrefix: "default" });
  const otherAdapter = makeAdapter({ idPrefix: "other" });
  const coordinator = new ExecutionCoordinator({ adapter: defaultAdapter, journal });

  const execution = await coordinator.start(input, otherAdapter);
  await coordinator.cancel(execution.executionId);

  assert.deepEqual(otherAdapter.cancelled, [execution.executionId], "the owner got the cancel");
  assert.deepEqual(defaultAdapter.cancelled, [], "the other runtime was not touched");
});

test("finds the execution that owns a pending approval request", async () => {
  const journal = new InMemoryJournalStore();
  const owner = makeAdapter({ idPrefix: "owner" });
  const bystander = makeAdapter({ idPrefix: "bystander" });
  (owner as AgentRuntimeAdapter & { getPendingPermissions?: unknown }).getPendingPermissions = () => [
    { requestId: "req-1", executionId: "owner-1", payload: {} }
  ];
  const coordinator = new ExecutionCoordinator({ adapter: bystander, journal });

  const execution = await coordinator.start(input, owner);
  const found = await coordinator.findByPermissionRequest("req-1");

  assert.equal(found, execution.executionId);
  assert.equal(await coordinator.findByPermissionRequest("req-unknown"), undefined);
});
