import test from "node:test";
import assert from "node:assert/strict";
import {
  executeClientDisconnect,
  judgeDisconnect,
  type DisconnectClient,
  type StreamEvent
} from "../../runners/pi-client-disconnect-phase0.js";

/**
 * The verdict logic of the P0-019 probe, without a model.
 *
 * The probe promises two things at once: the client that comes back gets every event it missed,
 * and it does not get any event twice. A probe that cannot tell those two apart from "the run
 * produced no events at all" is not evidence, so most of these cases are the ones that have to
 * fail — a hole in the timeline, a repeated frame, a second execution, a run that failed, a run
 * that ended before the client could even disconnect.
 */

const EXECUTION_ID = "exec-p0-019";
const TASK_ID = "task-p0-019";
const COMMIT_SHA = "a".repeat(40);

/** One frame of the unified envelope. */
function frame(sequence: number, type = "agent.tool.started"): StreamEvent {
  return {
    id: `${EXECUTION_ID}:${sequence}`,
    type,
    sequence,
    timestamp: new Date(Date.UTC(2026, 0, 1, 0, 0, sequence)).toISOString(),
    data: {}
  };
}

interface FakeOptions {
  /** Every journal position the server recorded. */
  journal?: number[];
  /** The positions the first connection delivers before the client drops. */
  firstStream?: number[];
  /** The positions the reconnect delivers. */
  resumedStream?: number[];
  status?: string;
  executionIds?: string[];
  gitCommits?: string[];
  verdicts?: string[];
}

/**
 * A server that behaves like the real one: the first connection hands over the first three
 * events, the reconnect hands over what the client missed.
 *
 * The reconnect does not filter by `since` on purpose. A real server does, and a server that
 * replays one event too many is exactly the failure the probe has to catch — a fake that always
 * filtered would make the duplicate case impossible to express.
 */
function fakeClient(options: FakeOptions = {}): DisconnectClient {
  const journal = options.journal ?? [1, 2, 3, 4, 5];
  const firstStream = options.firstStream ?? [1, 2, 3];
  const resumedStream = options.resumedStream ?? journal.filter(position => position > 3);
  const terminalSequence = resumedStream.at(-1) ?? firstStream.at(-1);
  let calls = 0;

  return {
    start: async () => EXECUTION_ID,
    status: async () => options.status ?? "succeeded",
    stream: async () => {
      calls += 1;
      const positions = calls === 1 ? firstStream : resumedStream;
      const frames = positions.map(position =>
        frame(position, position === terminalSequence ? "agent.execution.completed" : "agent.tool.started")
      );
      return { frames, close: () => undefined };
    },
    journal: async () => journal,
    taskExecutions: async () => options.executionIds ?? [EXECUTION_ID],
    trace: async () => ({
      gitCommits: options.gitCommits ?? [COMMIT_SHA],
      verdicts: options.verdicts ?? ["passed"]
    })
  };
}

test("P0-019 passes when the reconnect completes the timeline without repeating it", async () => {
  const result = await executeClientDisconnect(fakeClient());

  assert.equal(result.id, "P0-019");
  assert.equal(result.status, "passed");
  assert.equal(result.notes, undefined);
  const links = result.evidence.filter(line => /:ok /.test(line));
  assert.equal(links.length, 5, "every one of the five links is checked");
});

test("P0-019 fails when the client never receives an event the server recorded", async () => {
  // The server recorded five events; the reconnect only delivered the last two, so the run has a
  // hole in it that no reader of the timeline would notice.
  const result = await executeClientDisconnect(
    fakeClient({ journal: [1, 2, 3, 4, 5, 6], resumedStream: [4, 5] })
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the complete link does not hold/);
});

test("P0-019 fails when the reconnect delivers an event the client already had", async () => {
  // The replay overlaps the live stream: the same frame arrives twice. A client that keys on the
  // frame id hides it, so only a probe that counts the raw deliveries can see it.
  const result = await executeClientDisconnect(fakeClient({ resumedStream: [3, 4, 5] }));

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the no-duplicate link does not hold/);
});

test("P0-019 fails when the task produced a second execution", async () => {
  const result = await executeClientDisconnect(
    fakeClient({ executionIds: [EXECUTION_ID, "exec-other"] })
  );

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the single-execution link does not hold/);
});

test("P0-019 fails when the execution did not succeed", async () => {
  const result = await executeClientDisconnect(fakeClient({ status: "failed" }));

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the continues link does not hold/);
});

test("P0-019 fails when the change never landed in a commit", async () => {
  const result = await executeClientDisconnect(fakeClient({ gitCommits: [] }));

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the evidence link does not hold/);
});

test("P0-019 fails when the acceptance check did not pass", async () => {
  const result = await executeClientDisconnect(fakeClient({ verdicts: ["failed"] }));

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the evidence link does not hold/);
});

test("P0-019 is blocked when the run finished before the client could disconnect", async () => {
  // Nothing was missed, so the reconnect was never exercised. That is neither a pass nor a
  // failure: the probe never got to test the thing it is here to test (D-020).
  const result = await executeClientDisconnect(
    fakeClient({ journal: [1, 2, 3, 4, 5], firstStream: [1, 2, 3, 4, 5] })
  );

  assert.equal(result.status, "blocked");
  assert.match(result.notes ?? "", /finished before the client could disconnect/);
});

test("judgeDisconnect does not pass a run that recorded nothing", async () => {
  // An empty journal with an empty timeline would satisfy "the client holds every recorded
  // event" — vacuously, and while proving nothing.
  const judgement = judgeDisconnect({
    status: "succeeded",
    received: [],
    journal: [],
    executionIds: [EXECUTION_ID],
    expectedExecutionId: EXECUTION_ID,
    gitCommits: [COMMIT_SHA],
    verdicts: ["passed"]
  });

  assert.equal(judgement.ok, false);
  assert.deepEqual(
    judgement.links.filter(link => !link.ok).map(link => link.name),
    ["complete"]
  );
});

test("judgeDisconnect counts a repeated frame even when the client could hide it", async () => {
  const judgement = judgeDisconnect({
    status: "succeeded",
    received: [frame(1), frame(2), frame(2), frame(3)],
    journal: [1, 2, 3],
    executionIds: [EXECUTION_ID],
    expectedExecutionId: EXECUTION_ID,
    gitCommits: [COMMIT_SHA],
    verdicts: ["passed"]
  });

  assert.equal(judgement.ok, false);
  const duplicate = judgement.links.find(link => link.name === "no-duplicate");
  assert.equal(duplicate?.ok, false);
  assert.match(duplicate?.detail ?? "", /event\(s\) twice/);
});

test("judgeDisconnect names every link, so a failure says which one is missing", () => {
  const judgement = judgeDisconnect({
    status: "failed",
    received: [frame(1)],
    journal: [1, 2],
    executionIds: [],
    expectedExecutionId: EXECUTION_ID,
    gitCommits: [],
    verdicts: []
  });

  assert.equal(judgement.ok, false);
  assert.deepEqual(
    judgement.links.map(link => link.name),
    ["continues", "complete", "no-duplicate", "single-execution", "evidence"]
  );
  assert.deepEqual(
    judgement.links.filter(link => !link.ok).map(link => link.name),
    ["continues", "complete", "single-execution", "evidence"]
  );
});
