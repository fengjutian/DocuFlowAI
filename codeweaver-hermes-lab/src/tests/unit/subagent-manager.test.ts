import test from "node:test";
import assert from "node:assert/strict";
import { InMemoryJournalStore } from "../../journal/journal.js";
import { ExecutionCoordinator } from "../../orchestrator/execution-coordinator.js";
import { SubagentManager, type WorktreeProvider } from "../../orchestrator/subagent-manager.js";
import {
  NO_CAPABILITIES,
  type AgentRuntimeAdapter,
  type AgentTaskInput
} from "../../types/adapter.js";

const parentInput: AgentTaskInput = {
  projectId: "proj-1",
  taskId: "task-1",
  runId: "run-parent",
  prompt: "plan the feature",
  contextSnapshotId: "ctx-1",
  policySnapshotId: "pol-1",
  worktreeId: "wt-1",
  workingDirectory: "/tmp/parent-wt",
  allowedTools: ["read_file"],
  timeoutSeconds: 30,
  correlationId: "corr-1"
};

function childInput(runId: string): AgentTaskInput {
  return { ...parentInput, runId, taskId: "task-child", prompt: "do the work" };
}

function adapter(): AgentRuntimeAdapter & { counter: number } {
  const state = {
    counter: 0,
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() {
      state.counter += 1;
      return {
        executionId: `exec-${state.counter}`,
        runtimeType: "hermes" as const,
        status: "running" as const,
        externalRunId: `run-ext-${state.counter}`
      };
    },
    async getStatus() { return "running" as const; },
    async cancel() { return undefined; },
    async resume() { throw new Error("not used"); },
    async subscribe(_id: string, handler: (event: never) => void) {
      handler({
        eventId: "x",
        executionId: "x",
        projectId: "proj-1",
        taskId: "task-1",
        sequence: 1,
        eventType: "execution.started",
        timestamp: "2026-11-09T00:00:00.000Z",
        payload: {}
      } as never);
      return async () => undefined;
    },
    async terminate() { return undefined; }
  };
  return state as unknown as AgentRuntimeAdapter & { counter: number };
}

function fakeWorktrees(): WorktreeProvider & { count: number; lastBaseSha?: string | undefined } {
  const state = {
    count: 0,
    lastBaseSha: undefined as string | undefined,
    async create(baseSha: string, label?: string) {
      state.count += 1;
      state.lastBaseSha = baseSha;
      return {
        path: `/tmp/sub-${state.count}-${label ?? "x"}`,
        baseSha,
        createdAt: "2026-11-09T00:00:00.000Z"
      };
    }
  };
  return state;
}

async function setup() {
  const journal = new InMemoryJournalStore();
  const coordinator = new ExecutionCoordinator({ adapter: adapter(), journal });
  const parent = await coordinator.start(parentInput);
  return { journal, coordinator, parent };
}

test("spawns an isolated child at the base SHA and records both sides of the lineage", async () => {
  const { journal, coordinator, parent } = await setup();
  const worktrees = fakeWorktrees();
  const manager = new SubagentManager({ coordinator, worktrees });

  const child = await manager.spawn({
    parentExecutionId: parent.executionId,
    taskInput: childInput("run-child-1"),
    worktree: "isolated",
    baseSha: "a".repeat(40),
    label: "explorer"
  });

  assert.equal(child.worktreeMode, "isolated");
  assert.match(child.workingDirectory, /^\/tmp\/sub-1-explorer$/);
  assert.equal(worktrees.lastBaseSha, "a".repeat(40));
  assert.equal(manager.parentOf(child.execution.executionId), parent.executionId);
  assert.equal(manager.rootOf(child.execution.executionId), parent.executionId);
  assert.deepEqual(manager.childrenOf(parent.executionId), [child.execution.executionId]);
  assert.deepEqual(manager.lineageOf(child.execution.executionId), [
    child.execution.executionId,
    parent.executionId
  ]);

  await coordinator.flush(parent.executionId);
  await coordinator.flush(child.execution.executionId);
  const spawned = (await journal.read(parent.executionId)).find(entry => entry.category === "subagent.spawned");
  assert.equal(spawned?.payload["childExecutionId"], child.execution.executionId);
  assert.equal(spawned?.payload["worktreeMode"], "isolated");
  const parentLink = (await journal.read(child.execution.executionId)).find(
    entry => entry.category === "subagent.parent"
  );
  assert.equal(parentLink?.payload["parentExecutionId"], parent.executionId);
});

test("shares the parent worktree only when asked", async () => {
  const { coordinator, parent } = await setup();
  const worktrees = fakeWorktrees();
  const manager = new SubagentManager({ coordinator, worktrees });

  const child = await manager.spawn({
    parentExecutionId: parent.executionId,
    taskInput: childInput("run-child-2"),
    worktree: "shared"
  });

  assert.equal(child.worktreeMode, "shared");
  assert.equal(child.workingDirectory, parentInput.workingDirectory);
  assert.equal(worktrees.count, 0);
});

test("attributes a grandchild to the root task tree", async () => {
  const { coordinator, parent } = await setup();
  const manager = new SubagentManager({ coordinator, worktrees: fakeWorktrees() });

  const child = await manager.spawn({
    parentExecutionId: parent.executionId,
    taskInput: childInput("run-child-3"),
    worktree: "isolated",
    baseSha: "b".repeat(40)
  });
  const grandchild = await manager.spawn({
    parentExecutionId: child.execution.executionId,
    taskInput: { ...childInput("run-grandchild-1"), workingDirectory: child.workingDirectory },
    worktree: "shared"
  });

  assert.equal(manager.rootOf(grandchild.execution.executionId), parent.executionId);
  assert.deepEqual(manager.lineageOf(grandchild.execution.executionId), [
    grandchild.execution.executionId,
    child.execution.executionId,
    parent.executionId
  ]);
});

test("refuses unsafe spawns instead of guessing", async () => {
  const { coordinator, parent } = await setup();
  const manager = new SubagentManager({ coordinator, worktrees: fakeWorktrees() });

  await assert.rejects(
    manager.spawn({ parentExecutionId: "exec-missing", taskInput: childInput("run-x") }),
    /not tracked/
  );
  await assert.rejects(
    manager.spawn({
      parentExecutionId: parent.executionId,
      taskInput: childInput("run-parent"),
      worktree: "shared"
    }),
    /different run id/
  );
  await assert.rejects(
    manager.spawn({
      parentExecutionId: parent.executionId,
      taskInput: childInput("run-y"),
      worktree: "isolated"
    }),
    /base SHA/
  );

  const noWorktrees = new SubagentManager({ coordinator });
  await assert.rejects(
    noWorktrees.spawn({
      parentExecutionId: parent.executionId,
      taskInput: childInput("run-z"),
      worktree: "isolated",
      baseSha: "c".repeat(40)
    }),
    /worktree provider/
  );
});
