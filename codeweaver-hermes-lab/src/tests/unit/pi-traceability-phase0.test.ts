import test from "node:test";
import assert from "node:assert/strict";
import {
  executeTraceabilityChain,
  judgeTrace,
  type PiTraceabilityClient,
  type TraceResponse
} from "../../runners/pi-traceability-phase0.js";

/**
 * The verdict logic of the P0-024 probe, without a model.
 *
 * A probe that only knows how to say "passed" is not evidence, so most of these cases are the ones
 * that have to fail: a chain with no task, no runtime, a missing tool, a change that never became
 * a commit, a check that did not pass, and — the two that matter most — a chain that cannot be
 * walked back from the task or from the commit.
 */

const EXECUTION_ID = "exec-1";
const TASK_ID = "task-p0-024";
const COMMIT_SHA = "c".repeat(40);

function completeTrace(overrides: TraceResponse = {}): TraceResponse {
  return {
    executionId: EXECUTION_ID,
    taskIds: [TASK_ID],
    runtimeId: "pi-local",
    externalRunId: "run-7",
    toolCalls: [
      { toolCallId: "call-1", toolName: "read_file", status: "completed" },
      { toolCallId: "call-2", toolName: "request_apply_diff", status: "completed" },
      { toolCallId: "call-3", toolName: "evaluate_changes", status: "completed" }
    ],
    permissions: [{ requestId: "perm-1", decision: "approve" }],
    gitCommits: [COMMIT_SHA],
    changeRecords: [{ id: "chg-1", gitCommitSha: COMMIT_SHA, diffHash: "d".repeat(64) }],
    evaluations: [{ verdict: "passed" }],
    ...overrides
  };
}

function fakeClient(
  trace: TraceResponse,
  options: { status?: string; missingTraceStatus?: number } = {}
): PiTraceabilityClient {
  return {
    start: async () => EXECUTION_ID,
    status: async () => options.status ?? "succeeded",
    trace: async () => trace,
    traceStatus: async () => options.missingTraceStatus ?? 404,
    commitTrace: async sha => (sha === COMMIT_SHA ? { executionId: EXECUTION_ID } : undefined),
    taskTrace: async () => [{ executionId: EXECUTION_ID }]
  };
}

test("P0-024 passes when every link of the chain holds", async () => {
  const result = await executeTraceabilityChain(fakeClient(completeTrace()));

  assert.equal(result.id, "P0-024");
  assert.equal(result.status, "passed");
  assert.equal(result.notes, undefined);
  const links = result.evidence.filter(line => /:ok /.test(line));
  assert.equal(links.length, 9, "the chain has nine links and every one of them is checked");
});

test("P0-024 fails when the trace does not name the task", async () => {
  const result = await executeTraceabilityChain(fakeClient(completeTrace({ taskIds: ["task-other"] })));

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the task link does not hold/);
});

test("P0-024 fails when the trace does not name the runtime that ran it", async () => {
  const trace = completeTrace();
  delete trace.runtimeId;
  const result = await executeTraceabilityChain(fakeClient(trace));

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the run link does not hold/);
});

test("P0-024 fails when a tool call is missing", async () => {
  const result = await executeTraceabilityChain(
    fakeClient(
      completeTrace({
        toolCalls: [
          { toolCallId: "call-1", toolName: "read_file", status: "completed" },
          { toolCallId: "call-3", toolName: "evaluate_changes", status: "completed" }
        ]
      })
    )
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the tool link does not hold/);
});

test("P0-024 fails when the tool calls came in the wrong order", async () => {
  const result = await executeTraceabilityChain(
    fakeClient(
      completeTrace({
        toolCalls: [
          { toolCallId: "call-3", toolName: "evaluate_changes", status: "completed" },
          { toolCallId: "call-1", toolName: "read_file", status: "completed" },
          { toolCallId: "call-2", toolName: "request_apply_diff", status: "completed" }
        ]
      })
    )
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the tool link does not hold/);
});

test("P0-024 fails when no change was recorded", async () => {
  const result = await executeTraceabilityChain(
    fakeClient(completeTrace({ changeRecords: [], gitCommits: [] }))
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the change link does not hold/);
});

test("P0-024 fails when the change never became a commit", async () => {
  const result = await executeTraceabilityChain(
    fakeClient(
      completeTrace({
        changeRecords: [{ id: "chg-1", diffHash: "d".repeat(64) }],
        gitCommits: []
      })
    )
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the commit link does not hold/);
});

test("P0-024 fails when the acceptance check did not pass", async () => {
  const result = await executeTraceabilityChain(
    fakeClient(completeTrace({ evaluations: [{ verdict: "failed" }] }))
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the test link does not hold/);
});

test("P0-024 fails when the task does not list the execution it produced", async () => {
  const client = fakeClient(completeTrace());
  const result = await executeTraceabilityChain({
    ...client,
    taskTrace: async () => [{ executionId: "exec-other" }]
  });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the task-round-trip link does not hold/);
});

test("P0-024 fails when the commit does not resolve back to the execution", async () => {
  const client = fakeClient(completeTrace());
  const result = await executeTraceabilityChain({
    ...client,
    commitTrace: async () => ({ executionId: "exec-other" })
  });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the commit-round-trip link does not hold/);
});

test("P0-024 fails when the run itself failed", async () => {
  const result = await executeTraceabilityChain(fakeClient(completeTrace(), { status: "failed" }));

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the execution ended as 'failed'/);
});

test("P0-024 fails when an execution that was never recorded is answered with an empty chain", async () => {
  // The id was mistyped, or the run never happened. Answering 200 with six empty links would make
  // it look like a real run that produced no tools, commits or checks.
  const result = await executeTraceabilityChain(
    fakeClient(completeTrace(), { missingTraceStatus: 200 })
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the not-found link does not hold/);
});

test("judgeTrace names every link, so a failure says which one is missing", () => {
  const judgement = judgeTrace({
    executionId: EXECUTION_ID,
    taskId: TASK_ID,
    status: "succeeded",
    trace: completeTrace({ taskIds: [], evaluations: [] }),
    commitTrace: { executionId: EXECUTION_ID },
    taskTrace: [{ executionId: EXECUTION_ID }],
    missingTraceStatus: 404
  });

  assert.equal(judgement.ok, false);
  assert.deepEqual(
    judgement.links.filter(link => !link.ok).map(link => link.name),
    ["task", "test"]
  );
  assert.deepEqual(
    judgement.links.map(link => link.name),
    ["task", "run", "tool", "change", "commit", "test", "task-round-trip", "commit-round-trip", "not-found"]
  );
});
