import test from "node:test";
import assert from "node:assert/strict";
import {
  executeRuntimeCrash,
  judgeRuntimeCrash,
  type CrashClient,
  type CrashJournalEntry,
  type CrashKill
} from "../../runners/runtime-crash-phase0.js";

/**
 * The verdict logic of the P0-020 probe, without a runtime.
 *
 * A crash is the case where a system is most likely to lie: a run that hangs looks like a run that
 * is working, a run that "succeeded" before the crash looks like a crash that was handled, and a
 * silent retry looks like recovery. So most of these cases are the ones that have to fail.
 */

const EXECUTION_ID = "exec-p0-020";
const TASK_ID = "task-p0-020";

function journalWithFailure(): CrashJournalEntry[] {
  return [
    { category: "execution.runtime", kind: "audit", payload: { runtimeId: "hermes-local" } },
    { category: "execution.input", kind: "audit", payload: { taskId: TASK_ID } },
    { category: "execution.started", kind: "runtime.event", payload: {} },
    { category: "message.delta", kind: "runtime.event", payload: { text: "a write-ahead log" } },
    {
      category: "execution.failed",
      kind: "runtime.event",
      payload: { reason: "stream_error", message: "socket hang up" }
    }
  ];
}

interface FakeOptions {
  status?: string;
  journal?: CrashJournalEntry[];
  traceStatus?: number;
  executionIds?: string[];
  retry?: { status: number; message?: string };
  runtimeHealthy?: boolean;
  kill?: { aliveBefore: boolean; aliveAfter: boolean };
  /** The runtime never produced output, so the crash was never exercised. */
  noOutput?: boolean;
}

/** A server that behaves like the crashed one, with the crash itself under the test's control. */
function fakeClient(options: FakeOptions = {}): { client: CrashClient; kill: () => CrashKill } {
  let killed = false;
  const client: CrashClient = {
    start: async () => EXECUTION_ID,
    status: async () => options.status ?? "failed",
    journal: async () => {
      if (options.noOutput) {
        return [
          { category: "execution.input", kind: "audit", payload: { taskId: TASK_ID } },
          { category: "execution.started", kind: "runtime.event", payload: {} }
        ];
      }
      const entries = options.journal ?? journalWithFailure();
      // The failure only reaches the journal once the runtime has actually died.
      return killed ? entries : entries.filter(entry => entry.category !== "execution.failed");
    },
    traceStatus: async () => options.traceStatus ?? 200,
    taskExecutions: async () => options.executionIds ?? [EXECUTION_ID],
    retry: async () =>
      options.retry ?? { status: 409, message: "Recovery decided 'stop': Execution is already terminal" },
    runtimeHealthy: async () => options.runtimeHealthy ?? false
  };
  return {
    client,
    kill: () => {
      killed = true;
      return options.kill ?? { aliveBefore: true, aliveAfter: false };
    }
  };
}

function judge(overrides: Partial<Parameters<typeof judgeRuntimeCrash>[0]> = {}) {
  return judgeRuntimeCrash({
    status: "failed",
    journal: journalWithFailure(),
    traceStatus: 200,
    executionIds: [EXECUTION_ID],
    expectedExecutionId: EXECUTION_ID,
    retry: { status: 409, message: "Recovery decided 'stop': Execution is already terminal" },
    runtimeHealthy: false,
    kill: { aliveBefore: true, aliveAfter: false },
    ...overrides
  });
}

test("P0-020 passes when a crashed runtime is handled honestly", () => {
  const judgement = judge();

  assert.equal(judgement.ok, true);
  assert.deepEqual(
    judgement.links.map(link => link.name),
    ["crash-was-real", "crash-detected", "crash-recorded", "audit-kept", "no-silent-rerun", "runtime-unhealthy"]
  );
});

test("P0-020 fails when the runtime was still alive after the kill", () => {
  // The probe did not crash anything, so nothing was tested.
  const judgement = judge({ kill: { aliveBefore: true, aliveAfter: true } });

  assert.equal(judgement.ok, false);
  assert.deepEqual(
    judgement.links.filter(link => !link.ok).map(link => link.name),
    ["crash-was-real"]
  );
});

test("P0-020 fails when the execution is reported as succeeded", () => {
  const judgement = judge({ status: "succeeded" });

  assert.equal(judgement.ok, false);
  assert.equal(judgement.links.find(link => link.name === "crash-detected")?.ok, false);
});

test("P0-020 fails when the journal holds no failure entry", () => {
  const judgement = judge({
    journal: journalWithFailure().filter(entry => entry.category !== "execution.failed")
  });

  assert.equal(judgement.ok, false);
  assert.match(judgement.links.find(link => link.name === "crash-recorded")?.detail ?? "", /no failure entry/);
});

test("P0-020 fails when the failure is recorded without a reason", () => {
  const judgement = judge({
    journal: [
      ...journalWithFailure().filter(entry => entry.category !== "execution.failed"),
      { category: "execution.failed", kind: "runtime.event", payload: {} }
    ]
  });

  assert.equal(judgement.ok, false);
  assert.match(judgement.links.find(link => link.name === "crash-recorded")?.detail ?? "", /without a reason/);
});

test("P0-020 fails when the failed execution's record cannot be read back", () => {
  const judgement = judge({ traceStatus: 404 });

  assert.equal(judgement.ok, false);
  assert.equal(judgement.links.find(link => link.name === "audit-kept")?.ok, false);
});

test("P0-020 fails when the request that was made was thrown away", () => {
  const judgement = judge({
    journal: journalWithFailure().filter(entry => entry.category !== "execution.input")
  });

  assert.equal(judgement.ok, false);
  assert.equal(judgement.links.find(link => link.name === "audit-kept")?.ok, false);
});

test("P0-020 fails when the recovery attempt calls the model again", () => {
  // A 202 means the server decided to rebuild and started a new run — the thing design 14.2
  // forbids answering a crash with.
  const judgement = judge({ retry: { status: 202, message: "rebuilt" } });

  assert.equal(judgement.ok, false);
  assert.match(judgement.links.find(link => link.name === "no-silent-rerun")?.detail ?? "", /instead of refusing/);
});

test("P0-020 fails when the task grew a second execution", () => {
  const judgement = judge({ executionIds: [EXECUTION_ID, "exec-other"] });

  assert.equal(judgement.ok, false);
  assert.match(judgement.links.find(link => link.name === "no-silent-rerun")?.detail ?? "", /2 execution/);
});

test("P0-020 fails when the registry still calls the dead runtime healthy", () => {
  const judgement = judge({ runtimeHealthy: true });

  assert.equal(judgement.ok, false);
  assert.equal(judgement.links.find(link => link.name === "runtime-unhealthy")?.ok, false);
});

test("P0-020 is blocked when the runtime produced nothing before the timeout", () => {
  // The crash was never exercised against a running turn. That is neither a pass nor a failure:
  // the probe never got to test the thing (D-020).
  const { client, kill } = fakeClient({ noOutput: true });

  return executeRuntimeCrash(client, { activityTimeoutMs: 20, killRuntime: kill }).then(result => {
    assert.equal(result.status, "blocked");
    assert.match(result.notes ?? "", /never exercised/);
  });
});

test("P0-020 fails when a link does not hold", async () => {
  const { client, kill } = fakeClient({ status: "succeeded" });

  const result = await executeRuntimeCrash(client, { activityTimeoutMs: 200, killRuntime: kill });

  assert.equal(result.status, "failed");
  assert.match(result.notes ?? "", /the crash-detected link does not hold/);
});

test("P0-020 passes end to end when the crash is handled honestly", async () => {
  const { client, kill } = fakeClient();

  const result = await executeRuntimeCrash(client, { activityTimeoutMs: 200, killRuntime: kill });

  assert.equal(result.id, "P0-020");
  assert.equal(result.status, "passed");
  assert.equal(result.notes, undefined);
  const links = result.evidence.filter(line => /:ok /.test(line));
  assert.equal(links.length, 6, "every one of the six links is checked");
});
