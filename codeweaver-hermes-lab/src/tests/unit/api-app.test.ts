import test from "node:test";
import assert from "node:assert/strict";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import { createApiServer, type ApiDependencies } from "../../api/app.js";
import { EventBus } from "../../api/event-bus.js";
import { UsageLedger } from "../../accounting/usage.js";
import { SkillRegistry } from "../../skills/skill-registry.js";
import { SkillBindingStore } from "../../skills/skill-bindings.js";
import { skillContentChecksum } from "../../skills/skill.js";
import { InMemoryJournalStore, type JournalStore } from "../../journal/journal.js";
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
  followUps: Array<{ executionId: string; prompt: string }>;
  bus: EventBus;
  coordinator: ExecutionCoordinator;
}

async function harness(options: {
  token?: string;
  usage?: UsageLedger;
  skills?: { registry: SkillRegistry; bindings: SkillBindingStore };
  /**
   * An existing journal, so a test can hand the next harness the history of a previous one.
   * That is what a server restart looks like from the outside: same records, empty memory.
   */
  journal?: JournalStore;
  /** Whether the runtime can continue a finished turn on the same session. */
  canFollowUp?: boolean;
} = {}): Promise<Harness> {
  const starts: StartTaskRequest[] = [];
  const cancelled: string[] = [];
  const approvals: Array<{ executionId: string; requestId: string; decision: string }> = [];
  const followUps: Array<{ executionId: string; prompt: string }> = [];
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
    // A runtime that cannot continue a finished turn omits the method, exactly like the Hermes
    // adapter does. The API has to refuse the follow-up rather than start a fresh run.
    ...(options.canFollowUp
      ? {
          async followUp(executionId: string, prompt: string) {
            followUps.push({ executionId, prompt });
            return { executionId, runtimeType: "hermes" as const, status: "running" as const };
          }
        }
      : {}),
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

  const journal = options.journal ?? new InMemoryJournalStore();
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
    async trace(executionId: string) {
      return { executionId, taskIds: [], toolCalls: [], permissions: [], gitCommits: [] } as never;
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
    ...(options.token ? { token: options.token } : {}),
    ...(options.usage ? { usage: options.usage } : {}),
    ...(options.skills ? { skills: options.skills } : {})
  };
  const server = await createApiServer(deps).listen(0);
  return {
    url: server.url,
    close: server.close,
    starts,
    cancelled,
    approvals,
    followUps,
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
    const body = await response.json() as { runtimes: Array<{ id: string; version: string; enabled: boolean }> };

    assert.equal(response.status, 200);
    assert.deepEqual(body.runtimes.map(item => item.id), ["hermes-local"]);
    assert.equal(body.runtimes[0]!.version, "0.23.0");
    assert.equal(body.runtimes[0]!.enabled, true);
  } finally {
    await app.close();
  }
});

test("disables and re-enables a registered runtime", async () => {
  const app = await harness();
  try {
    const disabled = await fetch(`${app.url}/api/v1/runtimes/hermes-local/disable`, { method: "POST" });
    assert.equal(disabled.status, 200);
    // The answer says what the disable left running: disabling stops new work and keeps the
    // sessions that are already running (DF-0303).
    assert.deepEqual(await disabled.json(), { id: "hermes-local", enabled: false, runningExecutions: [] });

    const list = await fetch(`${app.url}/api/v1/runtimes`);
    const body = await list.json() as { runtimes: Array<{ enabled: boolean }> };
    assert.equal(body.runtimes[0]!.enabled, false);

    const enabled = await fetch(`${app.url}/api/v1/runtimes/hermes-local/enable`, { method: "POST" });
    assert.equal(enabled.status, 200);
    assert.deepEqual(await enabled.json(), { id: "hermes-local", enabled: true });
  } finally {
    await app.close();
  }
});

test("scores the runtimes from what the journal recorded", async () => {
  const journal = new InMemoryJournalStore();
  await journal.append({
    executionId: "exec-1",
    sequence: 1,
    kind: "audit",
    category: "execution.runtime",
    recordedAt: "2026-11-11T00:00:00.000Z",
    payload: { runtimeId: "hermes-local" }
  });
  await journal.append({
    executionId: "exec-1",
    sequence: 2,
    eventSequence: 1,
    kind: "runtime.event",
    category: "execution.started",
    recordedAt: "2026-11-11T00:00:01.000Z",
    payload: {}
  });
  await journal.append({
    executionId: "exec-1",
    sequence: 3,
    eventSequence: 2,
    kind: "runtime.event",
    category: "execution.completed",
    recordedAt: "2026-11-11T00:00:04.000Z",
    payload: {}
  });

  const app = await harness({ journal });
  try {
    const response = await fetch(`${app.url}/api/v1/runtimes/scores`);
    const body = await response.json() as {
      runtimes: Array<{ runtimeId: string; metrics: { executions: number; succeeded: number } }>;
      comparison: { rows: Array<{ runtimeId: string }> };
      executionsInJournal: number;
    };

    assert.equal(response.status, 200);
    assert.equal(body.executionsInJournal, 1);
    const hermes = body.runtimes.find(item => item.runtimeId === "hermes-local");
    assert.equal(hermes?.metrics.executions, 1);
    assert.equal(hermes?.metrics.succeeded, 1);
    assert.equal(body.comparison.rows[0]?.runtimeId, "hermes-local");
  } finally {
    await app.close();
  }
});

test("a runtime id of 'scores' is not mistaken for a runtime name", async () => {
  const app = await harness();
  try {
    const response = await fetch(`${app.url}/api/v1/runtimes/scores`);
    assert.equal(response.status, 200);

    // The literal route wins over the `:id` wildcard, and a real name still resolves as a runtime.
    const named = await fetch(`${app.url}/api/v1/runtimes/hermes-local`);
    assert.equal(named.status, 200);
  } finally {
    await app.close();
  }
});

test("continues a finished turn on the session the execution already used", async () => {
  const app = await harness({ canFollowUp: true });
  try {
    // The follow-up needs the live session: the transcript the runtime keeps lives in this process.
    const started = await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });
    assert.equal(started.status, 201);

    const response = await fetch(`${app.url}/api/v1/executions/exec-1/follow-up`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ prompt: "now tell me B" })
    });

    assert.equal(response.status, 202);
    assert.deepEqual(app.followUps, [{ executionId: "exec-1", prompt: "now tell me B" }]);
  } finally {
    await app.close();
  }
});

test("refuses a follow-up on a runtime that cannot continue a turn", async () => {
  const app = await harness();
  try {
    const started = await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });
    assert.equal(started.status, 201);

    const response = await fetch(`${app.url}/api/v1/executions/exec-1/follow-up`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ prompt: "now tell me B" })
    });
    const body = await response.json() as { error?: string; message?: string };

    assert.equal(response.status, 501);
    assert.equal(body.error, "unsupported_capability");
    assert.equal(app.followUps.length, 0);
  } finally {
    await app.close();
  }
});

test("refuses a follow-up on an execution this process does not hold", async () => {
  const app = await harness({ canFollowUp: true });
  try {
    const response = await fetch(`${app.url}/api/v1/executions/exec-unknown/follow-up`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ prompt: "now tell me B" })
    });

    assert.equal(response.status, 409);
    assert.equal(app.followUps.length, 0);
  } finally {
    await app.close();
  }
});

test("rejects a follow-up without a prompt", async () => {
  const app = await harness({ canFollowUp: true });
  try {
    const response = await fetch(`${app.url}/api/v1/executions/exec-1/follow-up`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ prompt: "   " })
    });

    assert.equal(response.status, 400);
    assert.equal(app.followUps.length, 0);
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

test("answers an approval through the design's approval path", async () => {
  const app = await harness();
  try {
    await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });

    const approve = await fetch(`${app.url}/api/v1/approvals/req-1/approve`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ reason: "reviewed by a human" })
    });
    assert.equal(approve.status, 200);
    assert.deepEqual(await approve.json(), {
      executionId: "exec-1",
      requestId: "req-1",
      decision: "approve"
    });
    assert.deepEqual(app.approvals, [{ executionId: "exec-1", requestId: "req-1", decision: "approve" }]);
  } finally {
    await app.close();
  }
});

test("rejects an approval for a request nobody is waiting on", async () => {
  const app = await harness();
  try {
    const reject = await fetch(`${app.url}/api/v1/approvals/req-unknown/reject`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({})
    });
    assert.equal(reject.status, 404);
    assert.deepEqual(app.approvals, []);
  } finally {
    await app.close();
  }
});

test("carries the named runtime and the declared checks into the start request", async () => {
  const app = await harness();
  try {
    await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        ...startBody(),
        runtimeId: "pi-local",
        evaluationChecks: [{ id: "build", name: "Build", command: "npm", args: ["run", "build"] }]
      })
    });
    assert.equal(app.starts[0]!.runtimeId, "pi-local");
    assert.deepEqual(app.starts[0]!.evaluationChecks, [
      { id: "build", name: "Build", command: "npm", args: ["run", "build"] }
    ]);
  } finally {
    await app.close();
  }
});

test("refuses a malformed acceptance check before the task starts", async () => {
  const app = await harness();
  try {
    const response = await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ...startBody(), evaluationChecks: [{ id: "build" }] })
    });
    assert.equal(response.status, 400);
    assert.equal(app.starts.length, 0, "nothing started");
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

test("the trace of an execution that was never recorded is a 404, not a chain of empty links", async () => {
  const app = await harness();
  try {
    // A mistyped id has to be told apart from an execution that ran and recorded nothing: the
    // second one is a chain with holes in it, the first one has no chain at all.
    const missing = await fetch(`${app.url}/api/v1/executions/exec-unknown/trace`);
    const missingBody = await missing.json() as { error: string };
    assert.equal(missing.status, 404);
    assert.equal(missingBody.error, "execution_not_found");

    await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(startBody())
    });
    const found = await fetch(`${app.url}/api/v1/executions/exec-1/trace`);
    assert.equal(found.status, 200);
  } finally {
    await app.close();
  }
});

test("reports usage when accounting is wired and 501 when it is not", async () => {
  const bare = await harness();
  try {
    const response = await fetch(`${bare.url}/api/v1/executions/exec-1/usage`);
    assert.equal(response.status, 501);
  } finally {
    await bare.close();
  }

  const usage = new UsageLedger();
  usage.record({
    id: "u1",
    executionId: "exec-1",
    model: "hermes-small",
    promptTokens: 10,
    completionTokens: 5,
    totalTokens: 15,
    durationMs: 2_000
  });
  const app = await harness({ usage });
  try {
    const one = await fetch(`${app.url}/api/v1/executions/exec-1/usage`);
    const body = await one.json() as { usage: { totalTokens: number; model: string } };
    assert.equal(one.status, 200);
    assert.equal(body.usage.totalTokens, 15);
    assert.equal(body.usage.model, "hermes-small");

    assert.equal((await fetch(`${app.url}/api/v1/executions/exec-2/usage`)).status, 404);

    const summary = await fetch(`${app.url}/api/v1/usage`);
    const summaryBody = await summary.json() as { summary: { executions: number; totalTokens: number } };
    assert.equal(summaryBody.summary.totalTokens, 15);
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

test("registers and lists skills, and reports bindings per execution", async () => {
  const skills = { registry: new SkillRegistry(), bindings: new SkillBindingStore() };
  const app = await harness({ skills });
  const content = "# React development\n\nPrefer function components and hooks.\n";
  const manifest = {
    skillId: "react-dev",
    version: "1.0.0",
    name: "React development",
    kind: "engineering",
    permissions: ["read_file"],
    allowedRuntimes: ["hermes-local"],
    riskLevel: "low",
    approvalPolicy: "auto",
    content,
    checksum: skillContentChecksum(content),
    createdAt: "2026-11-09T00:00:00.000Z"
  };
  try {
    const empty = await fetch(`${app.url}/api/v1/skills`);
    assert.deepEqual((await empty.json() as { skills: unknown[] }).skills, []);

    const created = await fetch(`${app.url}/api/v1/skills`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(manifest)
    });
    assert.equal(created.status, 201);

    const listed = await fetch(`${app.url}/api/v1/skills`);
    const body = await listed.json() as { skills: Array<{ skillId: string; deprecated: boolean; content: string }> };
    assert.equal(body.skills.length, 1);
    assert.equal(body.skills[0]!.deprecated, false);
    // The body comes back with the manifest: a skill the console cannot read is a skill it cannot show.
    assert.equal(body.skills[0]!.content, content);

    const duplicate = await fetch(`${app.url}/api/v1/skills`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(manifest)
    });
    assert.equal(duplicate.status, 409);

    // A body that does not match its checksum is refused: the checksum is only evidence if it is
    // checked against something.
    const tampered = await fetch(`${app.url}/api/v1/skills`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        ...manifest,
        skillId: "other",
        version: "1.0.0",
        content: `${content}\nsneaky\n`
      })
    });
    assert.equal(tampered.status, 400);

    const bindings = await fetch(`${app.url}/api/v1/executions/exec-1/skills`);
    assert.deepEqual((await bindings.json() as { bindings: unknown[] }).bindings, []);
  } finally {
    await app.close();
  }
});

test("reports 501 for skills when the server has no skill surface", async () => {
  const app = await harness();
  try {
    assert.equal((await fetch(`${app.url}/api/v1/skills`)).status, 501);
  } finally {
    await app.close();
  }
});

/**
 * A journal left behind by an earlier server process.
 *
 * The records are what survive a restart; the live session is not. The runtime id is one of those
 * records, because the registry alone would name the server's default runtime instead.
 */
async function journalOfAnEarlierProcess(): Promise<JournalStore> {
  const journal = new InMemoryJournalStore();
  await journal.append({
    executionId: "exec-restarted",
    sequence: 1,
    eventSequence: 1,
    kind: "runtime.event",
    category: "execution.started",
    recordedAt: "2026-11-11T00:00:00.000Z",
    payload: { runtime: "pi" }
  });
  await journal.append({
    executionId: "exec-restarted",
    sequence: 2,
    kind: "audit",
    category: "execution.runtime",
    recordedAt: "2026-11-11T00:00:00.000Z",
    payload: { runtimeId: "pi-local" }
  });
  await journal.append({
    executionId: "exec-restarted",
    sequence: 3,
    eventSequence: 2,
    kind: "runtime.event",
    category: "execution.cancelled",
    recordedAt: "2026-11-11T00:00:01.000Z",
    payload: { runtimeConfirmed: true, runtimeStopReason: "aborted" }
  });
  return journal;
}

test("describes a recorded execution after the server process restarted", async () => {
  const app = await harness({ journal: await journalOfAnEarlierProcess() });
  try {
    const response = await fetch(`${app.url}/api/v1/executions/exec-restarted`);
    const body = await response.json() as {
      status: string;
      runtimeId?: string;
      live?: boolean;
      checkpoint: { terminal: boolean; terminalEventType?: string };
    };

    assert.equal(response.status, 200);
    // The status comes from the journal's terminal event, not from a runtime that is gone.
    assert.equal(body.status, "cancelled");
    assert.equal(body.checkpoint.terminal, true);
    assert.equal(body.checkpoint.terminalEventType, "execution.cancelled");
    // The runtime that ran it is the one that was journaled, not the server's default.
    assert.equal(body.runtimeId, "pi-local");
    // Nothing about it runs here any more, and the page has to be able to say so.
    assert.equal(body.live, false);
  } finally {
    await app.close();
  }
});

test("refuses to cancel an execution this process no longer holds", async () => {
  const app = await harness({ journal: await journalOfAnEarlierProcess() });
  try {
    const response = await fetch(`${app.url}/api/v1/executions/exec-restarted/cancel`, { method: "POST" });
    const body = await response.json() as { error: string; message: string };

    assert.equal(response.status, 409);
    assert.equal(body.error, "execution_not_live");
  } finally {
    await app.close();
  }
});
