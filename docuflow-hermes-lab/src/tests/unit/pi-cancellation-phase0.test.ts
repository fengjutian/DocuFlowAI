import test from "node:test";
import assert from "node:assert/strict";
import {
  executePiCancellationPhase0,
  type PiCancellationClient,
  type PiPhase0Event
} from "../../runners/pi-cancellation-phase0.js";

/**
 * The cancellation probe is judged on the transcript, so these tests drive the judgement with
 * transcripts of their own: a runtime that confirmed the abort, a runtime that did not, and a
 * model that finished its answer anyway.
 */

function client(events: PiPhase0Event[], status: string): PiCancellationClient {
  return {
    start: async () => "exec-1",
    status: async () => status,
    cancel: async () => undefined,
    events: async () => events
  };
}

function delta(text: string): PiPhase0Event {
  return { type: "agent.message.delta", data: { delta: text } };
}

function cancelled(confirmed: boolean, stopReason: string): PiPhase0Event {
  return {
    type: "agent.execution.cancelled",
    data: {
      reason: "cancelled by DocuFlow",
      runtimeConfirmed: confirmed,
      runtimeStopReason: stopReason
    }
  };
}

test("passes when the runtime confirmed the abort and the answer never finished", async () => {
  const results = await executePiCancellationPhase0(
    // `agent.execution.started` shares the prefix of the terminal events but is not one of them.
    client(
      [{ type: "agent.execution.started" }, delta("1\n2\n3\n"), cancelled(true, "aborted")],
      "cancelled"
    ),
    { completionToken: "TOKEN" }
  );

  assert.equal(results.length, 1);
  assert.equal(results[0]!.id, "P0-018");
  assert.equal(results[0]!.status, "passed");
  assert.ok(results[0]!.evidence.some(item => item.includes("runtimeStopReason=aborted")));
  assert.ok(results[0]!.evidence.some(item => item.includes("terminalStatus=cancelled")));
});

test("fails when the runtime never confirmed that the abort reached it", async () => {
  const results = await executePiCancellationPhase0(
    client([delta("1\n"), cancelled(false, "unconfirmed")], "cancelled"),
    { completionToken: "TOKEN" }
  );

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /did not confirm/);
});

test("fails when the model finished its answer, so the turn was never cut off", async () => {
  const results = await executePiCancellationPhase0(
    client([delta("all done: TOKEN"), cancelled(true, "aborted")], "cancelled"),
    { completionToken: "TOKEN" }
  );

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /finished its answer/);
});

test("fails when the execution ended in some other terminal state", async () => {
  const results = await executePiCancellationPhase0(
    client([delta("1\n"), cancelled(true, "aborted")], "succeeded"),
    { completionToken: "TOKEN" }
  );

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /ended as 'succeeded'/);
});

test("fails when events keep arriving after the terminal event", async () => {
  const results = await executePiCancellationPhase0(
    client([delta("1\n"), cancelled(true, "aborted"), delta("more")], "cancelled"),
    { completionToken: "TOKEN" }
  );

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /after the terminal event/);
});

test("fails when the runtime never produced output to cancel", async () => {
  const results = await executePiCancellationPhase0(client([], "succeeded"), {
    completionToken: "TOKEN",
    startTimeoutMs: 300
  });

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /never produced output/);
});
