import test from "node:test";
import assert from "node:assert/strict";
import { createModels } from "@earendil-works/pi-ai";
import { fauxProvider, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { PiAdapter, type PiAgentFactory } from "../../adapters/pi-adapter.js";
import { createPiAgentFactory } from "../../adapters/pi-agent-factory.js";
import type { AgentRuntimeEvent, AgentTaskInput } from "../../types/adapter.js";

/**
 * These tests drive a real `pi-agent-core` agent through Pi's faux provider: no model, no
 * network, and no mock of the runtime itself. A claim about Pi's behaviour is only worth
 * something if it came from the runtime, not from a test double.
 */

function makeFactory(responses: unknown[]): PiAgentFactory {
  const faux = fauxProvider({ api: "faux-api", provider: "faux", models: [{ id: "faux-model" }] });
  const models = createModels();
  models.setProvider(faux.provider);
  faux.setResponses(responses as never[]);
  return createPiAgentFactory({ models, model: faux.getModel() });
}

function makeInput(overrides: Partial<AgentTaskInput> = {}): AgentTaskInput {
  return {
    projectId: "proj-1",
    taskId: "task-1",
    runId: "run-1",
    prompt: "do the thing",
    contextSnapshotId: "ctx-1",
    policySnapshotId: "pol-1",
    worktreeId: "wt-1",
    workingDirectory: process.cwd(),
    allowedTools: ["read_file"],
    timeoutSeconds: 60,
    correlationId: "corr-1",
    ...overrides
  };
}

const readFileTool = {
  name: "read_file",
  label: "Read file",
  description: "Read a file",
  parameters: {
    type: "object",
    properties: { path: { type: "string" } },
    required: ["path"]
  },
  execute: async (_toolCallId: string, args: { path: string }) => {
    executed.push(args.path);
    return { content: [{ type: "text", text: "ok" }], details: {} };
  }
};

let executed: string[] = [];

test("the Pi adapter reports the capabilities the runtime actually has", async () => {
  const adapter = new PiAdapter(makeFactory([]));
  const capabilities = await adapter.getCapabilities();
  assert.equal(capabilities.streaming, true);
  assert.equal(capabilities.cancellation, true);
  assert.equal(capabilities.sessionResume, true);
  assert.equal(capabilities.toolEvents, true);
  assert.equal(capabilities.subAgents, false);
  assert.equal(capabilities.skills, false);
});

test("a run streams DocuFlow events and a late subscriber is replayed", async () => {
  executed = [];
  const adapter = new PiAdapter(makeFactory([fauxAssistantMessage("done")]), {
    tools: [readFileTool]
  });
  const execution = await adapter.start(makeInput());
  await new Promise(resolve => setTimeout(resolve, 250));

  const events: AgentRuntimeEvent[] = [];
  const unsubscribe = await adapter.subscribe(execution.executionId, event => events.push(event));
  assert.ok(events.length > 0, "a late subscriber receives the buffered history");
  assert.ok(events.some(event => event.eventType === "execution.started"));
  assert.ok(events.some(event => event.eventType === "message.delta"));
  assert.ok(events.some(event => event.eventType === "execution.completed"));
  assert.equal(await adapter.getStatus(execution.executionId), "succeeded");
  await unsubscribe();
});

test("a permitted tool runs and reports tool events", async () => {
  executed = [];
  const adapter = new PiAdapter(
    makeFactory([fauxAssistantMessage([fauxToolCall("read_file", { path: "README.md" })])]),
    { tools: [readFileTool], permissionHandler: async () => ({ decision: "approve" }) }
  );
  const execution = await adapter.start(makeInput());
  await new Promise(resolve => setTimeout(resolve, 250));

  const events: AgentRuntimeEvent[] = [];
  await adapter.subscribe(execution.executionId, event => events.push(event));
  assert.deepEqual(executed, ["README.md"], "the tool executed exactly once");
  assert.ok(events.some(event => event.eventType === "tool.started"));
  assert.ok(events.some(event => event.eventType === "tool.completed"));
});

test("a denied tool never executes and the reason reaches the model", async () => {
  executed = [];
  const adapter = new PiAdapter(
    makeFactory([fauxAssistantMessage([fauxToolCall("read_file", { path: "/etc/passwd" })])]),
    {
      tools: [readFileTool],
      permissionHandler: async () => ({ decision: "deny", reason: "policy: path outside allowedPaths" })
    }
  );
  const execution = await adapter.start(makeInput());
  await new Promise(resolve => setTimeout(resolve, 250));

  assert.deepEqual(executed, [], "the tool was blocked before execution");
  const events: AgentRuntimeEvent[] = [];
  await adapter.subscribe(execution.executionId, event => events.push(event));
  assert.ok(
    events.some(event => event.eventType === "permission.requested"),
    "the decision is auditable"
  );
});

test("a tool outside the allowed list is blocked even when policy approves", async () => {
  executed = [];
  const writeTool = {
    ...readFileTool,
    name: "write_file",
    execute: async (_toolCallId: string, args: { path: string }) => {
      executed.push(args.path);
      return { content: [{ type: "text", text: "written" }], details: {} };
    }
  };
  const adapter = new PiAdapter(
    makeFactory([fauxAssistantMessage([fauxToolCall("write_file", { path: "x.txt" })])]),
    { tools: [writeTool], permissionHandler: async () => ({ decision: "approve" }) }
  );
  const execution = await adapter.start(makeInput({ allowedTools: ["read_file"] }));
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.deepEqual(executed, [], "an unlisted tool is blocked");
});

test("without a policy handler every tool call is blocked", async () => {
  executed = [];
  const adapter = new PiAdapter(
    makeFactory([fauxAssistantMessage([fauxToolCall("read_file", { path: "README.md" })])]),
    { tools: [readFileTool] }
  );
  const execution = await adapter.start(makeInput());
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.deepEqual(executed, [], "no handler means no execution");
});

test("the tool set is built per execution when tools is a function", async () => {
  executed = [];
  const bound: Array<{ taskId: string; executionId: string }> = [];
  const adapter = new PiAdapter(
    makeFactory([fauxAssistantMessage([fauxToolCall("read_file", { path: "README.md" })])]),
    {
      tools: (input, executionId) => {
        bound.push({ taskId: input.taskId, executionId });
        return [readFileTool];
      },
      permissionHandler: async () => ({ decision: "approve" })
    }
  );
  const execution = await adapter.start(makeInput());
  await new Promise(resolve => setTimeout(resolve, 250));

  assert.equal(bound.length, 1, "the binder ran once for this execution");
  assert.equal(bound[0]!.taskId, "task-1");
  assert.equal(bound[0]!.executionId, execution.executionId, "the binder knows the execution id");
  assert.deepEqual(executed, ["README.md"], "the bound tool really ran");
});

test("a self-enforced tool runs without a second permission decision", async () => {
  executed = [];
  let handlerCalls = 0;
  const adapter = new PiAdapter(
    makeFactory([fauxAssistantMessage([fauxToolCall("read_file", { path: "README.md" })])]),
    {
      tools: [readFileTool],
      // The tool checks the policy itself, so the adapter must not gate it a second time.
      selfEnforcedTools: ["read_file"],
      permissionHandler: async () => {
        handlerCalls += 1;
        return { decision: "deny", reason: "the handler should not have been asked" };
      }
    }
  );
  const execution = await adapter.start(makeInput());
  await new Promise(resolve => setTimeout(resolve, 250));

  assert.equal(handlerCalls, 0, "the permission handler was not consulted");
  assert.deepEqual(executed, ["README.md"], "the self-enforced tool ran");
});

test("cancel stops the run and records a single terminal event", async () => {
  const slowTool = {
    name: "slow_tool",
    label: "Slow tool",
    description: "Waits forever",
    parameters: { type: "object", properties: {}, required: [] },
    execute: async (_toolCallId: string, _args: unknown, signal?: AbortSignal) => {
      await new Promise((_resolve, reject) => {
        const timer = setTimeout(() => _resolve(undefined), 10_000);
        signal?.addEventListener("abort", () => {
          clearTimeout(timer);
          reject(new Error("aborted"));
        });
      });
      return { content: [{ type: "text", text: "never" }], details: {} };
    }
  };
  const adapter = new PiAdapter(
    makeFactory([
      fauxAssistantMessage([fauxToolCall("slow_tool", {})]),
      fauxAssistantMessage("interrupted")
    ]),
    { tools: [slowTool], permissionHandler: async () => ({ decision: "approve" }) }
  );
  const execution = await adapter.start(makeInput({ allowedTools: ["slow_tool"] }));
  await new Promise(resolve => setTimeout(resolve, 200));
  await adapter.cancel(execution.executionId);
  await new Promise(resolve => setTimeout(resolve, 200));

  assert.equal(await adapter.getStatus(execution.executionId), "cancelled");
  const events: AgentRuntimeEvent[] = [];
  await adapter.subscribe(execution.executionId, event => events.push(event));
  const terminal = events.filter(
    event => event.eventType === "execution.cancelled" || event.eventType === "execution.completed"
  );
  assert.equal(terminal.length, 1, "exactly one terminal event");
});
