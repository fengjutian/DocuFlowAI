import test from "node:test";
import assert from "node:assert/strict";
import {
  executeMultiTurnSession,
  type PiMultiTurnClient,
  type PiPhase0Event
} from "../../runners/pi-multi-turn-phase0.js";

/**
 * The verdict logic of the P0-007 probe, without a model.
 *
 * The item is "session continuity evidence", so the interesting cases are the ones where the
 * continuity is fake: an answer that repeats the previous turn, an answer that dumps the whole
 * context, a follow-up the server refused, and a second turn that never happened.
 */

const NONCE_A = "CODEWEAVER_P0_007_A_TEST";
const NONCE_B = "CODEWEAVER_P0_007_B_TEST";

function delta(text: string, sequence: number): PiPhase0Event {
  return { type: "agent.message.delta", sequence, data: { delta: text } };
}

function terminal(sequence: number, type = "agent.execution.completed"): PiPhase0Event {
  return { type, sequence, data: {} };
}

function turnMarker(sequence: number): PiPhase0Event {
  return { type: "codeweaver.turn.started", sequence, data: { prompt: "now tell me B" } };
}

/** The first turn answered READY; the second turn answered with B. */
function twoTurns(secondTurnText = NONCE_B, secondTurnTerminal = "agent.execution.completed"): PiPhase0Event[] {
  return [
    delta("READY\n", 1),
    terminal(2),
    turnMarker(3),
    delta(`${secondTurnText}\n`, 4),
    terminal(5, secondTurnTerminal)
  ];
}

function fakeClient(options: {
  events?: PiPhase0Event[];
  accepted?: boolean;
  status?: number;
} = {}): PiMultiTurnClient {
  return {
    start: async () => "exec-1",
    followUp: async () => ({
      accepted: options.accepted ?? true,
      status: options.status ?? 202,
      body: options.accepted === false ? "unsupported_capability" : ""
    }),
    events: async () => options.events ?? twoTurns()
  };
}

test("P0-007 passes when the second turn answers with the string the first turn was given", async () => {
  const result = await executeMultiTurnSession(fakeClient(), {
    nonceA: NONCE_A,
    nonceB: NONCE_B
  });

  assert.equal(result.id, "P0-007");
  assert.equal(result.status, "passed");
  assert.equal(result.notes, undefined);
});

test("P0-007 fails when the follow-up was refused", async () => {
  const result = await executeMultiTurnSession(fakeClient({ accepted: false, status: 501 }), {
    nonceA: NONCE_A,
    nonceB: NONCE_B
  });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /follow-up was refused/);
});

test("P0-007 fails when the second turn does not carry the string", async () => {
  const result = await executeMultiTurnSession(fakeClient({ events: twoTurns("I don't know") }), {
    nonceA: NONCE_A,
    nonceB: NONCE_B
  });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /did not answer with the string/);
});

test("P0-007 fails when the second turn dumps the whole context", async () => {
  const result = await executeMultiTurnSession(fakeClient({ events: twoTurns(`A=${NONCE_A} B=${NONCE_B}`) }), {
    nonceA: NONCE_A,
    nonceB: NONCE_B
  });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /repeated the string it was not asked for/);
});

test("P0-007 fails when the journal has no turn boundary", async () => {
  const result = await executeMultiTurnSession(
    fakeClient({ events: [delta("READY\n", 1), terminal(2), delta(`${NONCE_B}\n`, 3), terminal(4)] }),
    { nonceA: NONCE_A, nonceB: NONCE_B }
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /no turn boundary/);
});

test("P0-007 fails when the second turn did not complete", async () => {
  const result = await executeMultiTurnSession(
    fakeClient({ events: twoTurns(NONCE_B, "agent.execution.failed") }),
    { nonceA: NONCE_A, nonceB: NONCE_B }
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /second turn ended as/);
});

test("the two nonces are different, so the follow-up cannot ask for the wrong one", async () => {
  const events = twoTurns();
  const result = await executeMultiTurnSession(fakeClient({ events }), {
    nonceA: NONCE_A,
    nonceB: NONCE_B
  });

  // The answer carries B and not A: the model answered the question it was asked, and it could
  // only have known B from the retained transcript.
  const evidence = result.evidence.join("\n");
  assert.match(evidence, /secondTurnHasB=true/);
  assert.match(evidence, /secondTurnHasA=false/);
});
