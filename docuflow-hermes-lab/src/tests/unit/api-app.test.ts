import test from "node:test";
import assert from "node:assert/strict";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import { createApiServer, type ApiDependencies } from "../../api/app.js";
import { EventBus } from "../../api/event-bus.js";
import { InMemoryJournalStore } from "../../journal/journal.js";
import { ExecutionCoordinator } from "../../orchestrator/execution-coordinator.js";
import type { TaskExecutionService, StartTaskRequest } from "../../tasks/task-execution-service.js";
import {
  NO_CAPABILITIES,
  type AgentRuntimeAdapter,
  type RuntimePermissionRequest
} from "../../types/adapter.js";

interface Harness {
  url: string;
  close(): Promise<void>;
  starts: StartTaskRequest[];
  cancelled: string[];
  approvals: Array<{ executionId: string; requestId: string; decision: string }>;
  bus: EventBus;
  coordinator: ExecutionCoordinator;
}

async function harness(options: { token?: string } = {}): Promise<Harness> {
  const starts: StartTaskRequest[] = [];
  const cancelled: string[] = [];
  const approvals: Array<{ executionId: string; requestId: string; decision: string }> = [];
  let pending: RuntimePermissionRequest[] = [
    { requestId: "req-1", executionId: "exec-1", payload: { tool: "shell" } }
  ];

  const adapter: AgentRuntimeAdapter = {
    async getCapabilities() { return NO_CAPABILITIES; },
    async healthCheck() { return true; },
    async start() {
      return { executionId: "exec-1", runtimeType: "hermes", status: "running", externalRunId: "run-1" };
    },
    async getStatus() { return "running"; },
    async cancel(executionId) { cancelled.push(executionId); },
    async resume() { throw new Error("not used"); },
    async subscribe(_id, handler) {
      handler({
        eventId: "exec-1:1",
        executionId: "exec-1",
        projectId: "proj-1",
        taskId: "task-1",
        sequence: 1,
        eventType: "execution.started",
        timestamp: "2026-11-09T00:00:00.000Z",
        payload: { hermesEvent: "run.started" }
      });
      return async () => undefined;
    },
    async terminate() { return undefined; },
    getPendingPermissions() { return pending; },
    async respondToPermission(executionId, requestId, decision) {
      approvals.push({ executionId, requestId, decision: decision.decision });
      pending = pending.filter(item => item.requestId !== requestId);
    }
  };

  const journal = new InMemoryJournalStore();
  const coordinator = new ExecutionCoordinator({ adapter, journal });
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter, version: "0.23.0" });
  const bus = new EventBus();

  const taskService = {
    async startTask(request: StartTaskRequest) {
      starts.push(request);
      const execution = await coordinator.start({
        projectId: request.projectId,
        taskId: request.taskId,
        runId: "run-1",
        prompt: request.prompt,
        contextSnapshotId: "ctx-1",
        policySnapshotId: "pol-1",
        worktreeId: "wt-1",
        workingDirectory: request.worktreePath,
        allowedTools: request.allowedTools,
        timeoutSeconds: request.timeoutSeconds,
        correlationId: request.correlationId
      });
      return {
        execution,
        taskInput: {
          projectId: request.projectId,
          taskId: request.taskId,
          runId: "run-1",
          prompt: request.prompt,
          contextSnapshotId: "ctx-1",
          policySnapshotId: "pol-1",
          worktreeId: "wt-1",
          workingDirectory: request.worktreePath,
          allowedTools: [],
          timeoutSeconds: 60,
          correlationId: "corr-1"
        },
        contextSnapshot: { id: "ctx-1" },
        policySnapshot: { id: "pol-1" },
        promptSnapshot: { id: "prm-1" }
      } as never;
    },
    async traceCommit(sha: string) {
      return sha === "c".repeat(40)
        ? { changeRecord: { id: "chg-1", executionId: "exec-1" }, trace: { executionId: "exec-1" } }
        : undefined;
    },
    async completionGate() {
      return { allowed: false, reason: "not evaluated yet" };
    }
  } as unknown as TaskExecutionService;

  const deps: ApiDependencies = {
    registry,
    coordinator,
    taskService,
    journal,
    bus,
    ...(options.token ? { token: options.token } : {})
  };
  const server = await createApiServer(deps).listen(0);
  return {
    url: server.url,
    close: server.close,
    starts,
    cancelled,
    approvals,
    bus,
    coordinator
  };
}

function startBody(): Record<string, unknown> {
  return {
    projectId: "proj-1",
    prompt: "implement the feature",
    worktreePath: "/tmp/wt-1",
    documents: ["spec.md"],
    selectedFiles: ["app.ts"],
    architectureVersion: "1.0",
    policy: {
      snapshotId: "snap-1",
      projectId: "proj-1",
      taskId: "task-1",
      executionId: "exec-1",
      worktreeId: "wt-1",
      policyVersion: "1.0.0",
      grantedCapabilities: ["*"],
      allowedPaths: ["/tmp/wt-1"],
      expiresAt: "2099-01-01T00:00:00.000Z"
    },
    template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
    modelConfig: { model: "hermes-default" },
    correlationId: "corr-1",
    timeoutSeconds: 30,
    allowedTools: ["read_file"]
  };
}

test("lists registered runtimes", async () => {
  const app = await harness();
  try {
    const response = await fetch(`${app.url}/api/v1/runtimes`);
    const body = await response.json() as { runtimes: Array<{ id: string; version: string }> };

    assert.equal(response.status, 200);
    assert.deepEqual(body.runtimes.map(item => item.id), ["hermes-local"]);
    assert.equal(body.runtimes[0]!.version, "0.23.0");
  } finally {
    await app.close();
  }
});

test("creates an execution for a task", async () => {
  const app = await harness();
  try {
    const response = await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });
    const body = await response.json() as { execution: { executionId: string }; contextSnapshotId: string };

    assert.equal(response.status, 201);
    assert.equal(body.execution.executionId, "exec-1");
    assert.equal(body.contextSnapshotId, "ctx-1");
    assert.equal(app.starts.length, 1);
    assert.equal(app.starts[0]!.taskId, "task-1");
    assert.deepEqual(app.starts[0]!.allowedTools, ["read_file"]);
  } finally {
    await app.close();
  }
});

test("rejects a malformed task body with a 400", async () => {
  const app = await harness();
  try {
    const response = await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ prompt: "x" })
    });
    const body = await response.json() as { error: string };

    assert.equal(response.status, 400);
    assert.equal(body.error, "invalid_body");
    assert.equal(app.starts.length, 0);
  } finally {
    await app.close();
  }
});

test("enforces the bearer token when one is configured", async () => {
  const app = await harness({ token: "s3cret" });
  try {
    const unauthorized = await fetch(`${app.url}/api/v1/runtimes`);
    assert.equal(unauthorized.status, 401);

    const authorized = await fetch(`${app.url}/api/v1/runtimes`, {
      headers: { authorization: "Bearer s3cret" }
    });
    assert.equal(authorized.status, 200);
  } finally {
    await app.close();
  }
});

test("returns the checkpoint and cancels through the coordinator", async () => {
  const app = await harness();
  try {
    await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });

    const detail = await fetch(`${app.url}/api/v1/executions/exec-1`);
    const body = await detail.json() as { checkpoint: { lastSequence: number; eventCount: number }; completion: { allowed: boolean } };
    assert.equal(detail.status, 200);
    assert.ok(body.checkpoint.eventCount >= 1);
    assert.equal(body.completion.allowed, false);

    const cancel = await fetch(`${app.url}/api/v1/executions/exec-1/cancel`, { method: "POST" });
    assert.equal(cancel.status, 202);
    assert.deepEqual(app.cancelled, ["exec-1"]);
  } finally {
    await app.close();
  }
});

test("lists and answers pending approvals", async () => {
  const app = await harness();
  try {
    await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });

    const list = await fetch(`${app.url}/api/v1/executions/exec-1/approvals`);
    const listed = await list.json() as { approvals: Array<{ requestId: string }> };
    assert.deepEqual(listed.approvals.map(item => item.requestId), ["req-1"]);

    const answer = await fetch(`${app.url}/api/v1/executions/exec-1/approvals/req-1`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ decision: "deny", reason: "no shell in this workspace" })
    });
    assert.equal(answer.status, 200);
    assert.deepEqual(app.approvals, [{ executionId: "exec-1", requestId: "req-1", decision: "deny" }]);
  } finally {
    await app.close();
  }
});

test("reports an unsupported resume instead of pretending", async () => {
  const app = await harness();
  try {
    await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });

    const response = await fetch(`${app.url}/api/v1/executions/exec-1/resume`, { method: "POST" });
    const body = await response.json() as { error: string };

    assert.equal(response.status, 501);
    assert.equal(body.error, "resume_unsupported");
  } finally {
    await app.close();
  }
});

test("returns 404 for an unknown route and for an unknown commit", async () => {
  const app = await harness();
  try {
    assert.equal((await fetch(`${app.url}/api/v1/nope`)).status, 404);
    assert.equal((await fetch(`${app.url}/api/v1/trace/commits/${"9".repeat(40)}`)).status, 404);

    const found = await fetch(`${app.url}/api/v1/trace/commits/${"c".repeat(40)}`);
    assert.equal(found.status, 200);
  } finally {
    await app.close();
  }
});

test("streams live events as SSE with the design-13 envelope", async () => {
  const app = await harness();
  const controller = new AbortController();
  try {
    await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });

    const response = await fetch(`${app.url}/api/v1/events?executionId=exec-1`, {
      signal: controller.signal
    });
    assert.equal(response.status, 200);
    assert.match(response.headers.get("content-type") ?? "", /text\/event-stream/);

    const reader = response.body!.getReader();
    const decoder = new TextDecoder();
    let received = "";
    while (!received.includes("agent.execution.started")) {
      const chunk = await reader.read();
      if (chunk.done) break;
      received += decoder.decode(chunk.value, { stream: true });
    }
    assert.match(received, /event: agent\.execution\.started/);
    assert.match(received, /"executionId":"exec-1"/);

    app.bus.publish({
      id: "e2",
      type: "agent.message.delta",
      projectId: "proj-1",
      executionId: "exec-1",
      sequence: 2,
      timestamp: "2026-11-09T00:00:01.000Z",
      data: { text: "hi" }
    });
    while (!received.includes("agent.message.delta")) {
      const chunk = await reader.read();
      if (chunk.done) break;
      received += decoder.decode(chunk.value, { stream: true });
    }
    assert.match(received, /event: agent\.message\.delta/);
  } finally {
    controller.abort();
    await app.close();
  }
});
