import test from "node:test";
import assert from "node:assert/strict";
import {
  EXPECTED_TOOL_ORDER,
  assistantText,
  executeMultiToolTask,
  executeSingleStepTask,
  isSubsequence,
  toolEventsInOrder,
  type PiAgentExecutionClient,
  type PiPhase0Event
} from "../../runners/pi-agent-execution-phase0.js";

/**
 * The verdict logic of the P0-005 / P0-006 probe, without a model.
 *
 * A probe that only knows how to say "passed" is not evidence, so most of these cases are the ones
 * that have to fail: a file that was never read, tools in the wrong order, a change that never
 * landed, an acceptance check that did not pass.
 */

const NONCE = "CODEWEAVER_P0_005_TESTNONCE";

function toolEvent(
  kind: "started" | "completed",
  name: string,
  sequence: number
): PiPhase0Event {
  return {
    type: kind === "started" ? "agent.tool.started" : "agent.tool.completed",
    sequence,
    data: { toolName: name }
  };
}

function delta(text: string, sequence: number): PiPhase0Event {
  return { type: "agent.message.delta", sequence, data: { delta: text } };
}

interface FakeOptions {
  status?: string;
  events?: PiPhase0Event[];
  file?: string;
  changeRecords?: Array<{ id?: string }>;
  evaluations?: Array<{ verdict?: string }>;
  completionAllowed?: boolean;
}

function fakeClient(options: FakeOptions = {}): PiAgentExecutionClient {
  const events = options.events ?? [];
  return {
    start: async () => "exec-1",
    status: async () => options.status ?? "succeeded",
    events: async () => events,
    detail: async () => ({
      status: options.status ?? "succeeded",
      completion: { allowed: options.completionAllowed ?? true, reason: "checked" }
    }),
    trace: async () => ({
      changeRecords: options.changeRecords ?? [{ id: "chg-1" }],
      evaluations: options.evaluations ?? [{ verdict: "passed" }]
    }),
    readWorkspaceFile: async () => options.file ?? "export const value = 2;\n"
  };
}

test("P0-005 passes when the file was read and the answer carries its content", async () => {
  const client = fakeClient({
    events: [toolEvent("started", "read_file", 1), toolEvent("completed", "read_file", 2), delta(`${NONCE}\n`, 3)]
  });

  const result = await executeSingleStepTask(client, { nonce: NONCE });

  assert.equal(result.id, "P0-005");
  assert.equal(result.status, "passed");
  assert.equal(result.notes, undefined);
});

test("P0-005 fails when the file was never read", async () => {
  const client = fakeClient({ events: [delta(`${NONCE}\n`, 1)] });

  const result = await executeSingleStepTask(client, { nonce: NONCE });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /never read/);
});

test("P0-005 fails when the answer does not carry the file's content", async () => {
  const client = fakeClient({
    events: [toolEvent("started", "read_file", 1), toolEvent("completed", "read_file", 2), delta("I read it.\n", 3)]
  });

  const result = await executeSingleStepTask(client, { nonce: NONCE });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /does not contain/);
});

test("P0-005 fails when the single-step task reached for other tools", async () => {
  const client = fakeClient({
    events: [
      toolEvent("started", "read_file", 1),
      toolEvent("started", "request_apply_diff", 2),
      toolEvent("completed", "read_file", 3),
      delta(`${NONCE}\n`, 4)
    ]
  });

  const result = await executeSingleStepTask(client, { nonce: NONCE });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /other tools/);
});

test("P0-005 fails when the execution did not finish", async () => {
  const client = fakeClient({
    status: "failed",
    events: [toolEvent("started", "read_file", 1), delta(`${NONCE}\n`, 2)]
  });

  const result = await executeSingleStepTask(client, { nonce: NONCE });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /ended as 'failed'/);
});

test("P0-006 passes when the tools ran in order and the work landed", async () => {
  const client = fakeClient({
    events: [
      toolEvent("started", "read_file", 1),
      toolEvent("completed", "read_file", 2),
      toolEvent("started", "request_apply_diff", 3),
      toolEvent("completed", "request_apply_diff", 4),
      toolEvent("started", "evaluate_changes", 5),
      toolEvent("completed", "evaluate_changes", 6)
    ]
  });

  const result = await executeMultiToolTask(client);

  assert.equal(result.id, "P0-006");
  assert.equal(result.status, "passed");
  assert.equal(result.notes, undefined);
});

test("P0-006 fails when the tools ran in the wrong order", async () => {
  const client = fakeClient({
    events: [
      toolEvent("started", "evaluate_changes", 1),
      toolEvent("completed", "evaluate_changes", 2),
      toolEvent("started", "read_file", 3),
      toolEvent("completed", "read_file", 4),
      toolEvent("started", "request_apply_diff", 5),
      toolEvent("completed", "request_apply_diff", 6)
    ]
  });

  const result = await executeMultiToolTask(client);

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /did not run in the expected order/);
});

test("P0-006 fails when one of the tools never ran", async () => {
  const client = fakeClient({
    events: [
      toolEvent("started", "read_file", 1),
      toolEvent("completed", "read_file", 2),
      toolEvent("started", "request_apply_diff", 3),
      toolEvent("completed", "request_apply_diff", 4)
    ]
  });

  const result = await executeMultiToolTask(client);

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /evaluate_changes.*never called/);
});

test("P0-006 fails when the change did not land in the file", async () => {
  const client = fakeClient({
    file: "export const value = 1;\n",
    events: [
      toolEvent("started", "read_file", 1),
      toolEvent("completed", "read_file", 2),
      toolEvent("started", "request_apply_diff", 3),
      toolEvent("completed", "request_apply_diff", 4),
      toolEvent("started", "evaluate_changes", 5),
      toolEvent("completed", "evaluate_changes", 6)
    ]
  });

  const result = await executeMultiToolTask(client);

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /did not land/);
});

test("P0-006 fails when no change record was written", async () => {
  const client = fakeClient({
    changeRecords: [],
    events: [
      toolEvent("started", "read_file", 1),
      toolEvent("completed", "read_file", 2),
      toolEvent("started", "request_apply_diff", 3),
      toolEvent("completed", "request_apply_diff", 4),
      toolEvent("started", "evaluate_changes", 5),
      toolEvent("completed", "evaluate_changes", 6)
    ]
  });

  const result = await executeMultiToolTask(client);

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /no change record/);
});

test("P0-006 fails when the acceptance check did not pass", async () => {
  const client = fakeClient({
    evaluations: [{ verdict: "failed" }],
    events: [
      toolEvent("started", "read_file", 1),
      toolEvent("completed", "read_file", 2),
      toolEvent("started", "request_apply_diff", 3),
      toolEvent("completed", "request_apply_diff", 4),
      toolEvent("started", "evaluate_changes", 5),
      toolEvent("completed", "evaluate_changes", 6)
    ]
  });

  const result = await executeMultiToolTask(client);

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /acceptance check did not pass/);
});

test("P0-006 fails when the completion gate stayed closed", async () => {
  const client = fakeClient({
    completionAllowed: false,
    events: [
      toolEvent("started", "read_file", 1),
      toolEvent("completed", "read_file", 2),
      toolEvent("started", "request_apply_diff", 3),
      toolEvent("completed", "request_apply_diff", 4),
      toolEvent("started", "evaluate_changes", 5),
      toolEvent("completed", "evaluate_changes", 6)
    ]
  });

  const result = await executeMultiToolTask(client);

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /completion gate is not open/);
});

test("tool events are read in the order the runtime emitted them", () => {
  const calls = toolEventsInOrder([
    toolEvent("completed", "read_file", 9),
    toolEvent("started", "read_file", 8),
    { type: "agent.message.delta", sequence: 10, data: { delta: "x" } }
  ]);

  assert.deepEqual(
    calls.map(call => `${call.kind}:${call.name}:${call.sequence}`),
    ["started:read_file:8", "completed:read_file:9"]
  );
});

test("the assistant answer is assembled from the streamed deltas", () => {
  const text = assistantText([delta("Hello ", 1), delta("world", 2), toolEvent("started", "read_file", 3)]);
  assert.equal(text, "Hello world");
});

test("a subsequence allows extra tools in between", () => {
  assert.equal(isSubsequence(["read_file", "request_diff", "request_apply_diff", "evaluate_changes"], [...EXPECTED_TOOL_ORDER]), true);
  assert.equal(isSubsequence(["evaluate_changes", "read_file", "request_apply_diff"], [...EXPECTED_TOOL_ORDER]), false);
});
