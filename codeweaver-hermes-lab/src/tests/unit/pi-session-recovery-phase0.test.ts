import test from "node:test";
import assert from "node:assert/strict";
import {
  executePiSessionRecoveryPhase0,
  type PiSessionRecoveryClient,
  type RecoveryBody,
  type RecoveryEvent
} from "../../runners/pi-session-recovery-phase0.js";

/**
 * The recovery probe is judged on what the restarted worker decided and what it then did, so
 * these tests drive that judgement with a fake client: a worker that rebuilt correctly, one that
 * tried to resume a session that no longer exists, and one that rebuilt on the wrong runtime.
 */

interface FakeOptions {
  planAction?: string;
  runtimeId?: string;
  prompt?: string;
  rebuiltStatus?: string;
  rebuiltEvents?: RecoveryEvent[];
  streaming?: boolean;
  survivingEvents?: RecoveryEvent[];
}

function client(options: FakeOptions = {}): PiSessionRecoveryClient {
  const rebuiltId = "exec-rebuilt";
  const prompt = options.prompt ?? "original request\n\n[CODEWEAVER CHECKPOINT]\nexec-1";
  const facts: Record<string, Record<string, Record<string, unknown>>> = {
    [rebuiltId]: {
      "execution.runtime": { runtimeId: options.runtimeId ?? "pi-local" },
      "execution.input": { prompt }
    }
  };
  return {
    start: async () => "exec-1",
    waitForOutput: async () => options.streaming !== false,
    events: async () => options.survivingEvents ?? [{ type: "agent.message.delta" }],
    crash: async () => undefined,
    status: async () => options.rebuiltStatus ?? "succeeded",
    recover: async () => {
      const body: RecoveryBody = {
        previousExecutionId: "exec-1",
        execution: { executionId: rebuiltId },
        plan: { action: options.planAction ?? "rebuild", reason: "the session is gone" }
      };
      return { status: 202, body };
    },
    fact: async (executionId, category) => facts[executionId]?.[category],
    waitForTerminal: async () => options.rebuiltStatus ?? "succeeded"
  };
}

test("passes when the worker rebuilt on the same runtime and the rebuild ran", async () => {
  const results = await executePiSessionRecoveryPhase0(client());

  assert.equal(results.length, 1);
  assert.equal(results[0]!.id, "P0-021");
  assert.equal(results[0]!.status, "passed");
  assert.ok(results[0]!.evidence.some(item => item.includes("planAction=rebuild")));
  assert.ok(results[0]!.evidence.some(item => item.includes("rebuiltRuntimeId=pi-local")));
  assert.ok(results[0]!.evidence.some(item => item.includes("rebuiltPromptHasCheckpoint=true")));
});

test("fails when the worker tried to resume a session that died with it", async () => {
  const results = await executePiSessionRecoveryPhase0(client({ planAction: "resume" }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /instead of 'rebuild'/);
});

test("fails when the rebuild ran on a runtime the task never named", async () => {
  const results = await executePiSessionRecoveryPhase0(client({ runtimeId: "hermes-local" }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /instead of the runtime that ran the original/);
});

test("fails when the rebuilt request carries no checkpoint", async () => {
  const results = await executePiSessionRecoveryPhase0(client({ prompt: "just the original request" }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /does not carry the checkpoint/);
});

test("fails when the rebuild never reached a terminal state", async () => {
  const results = await executePiSessionRecoveryPhase0(client({ rebuiltStatus: "running" }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /never reached a terminal state/);
});

test("fails when the journal did not survive the crash", async () => {
  const results = await executePiSessionRecoveryPhase0(client({ survivingEvents: [] }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /journal did not survive/);
});

test("fails when the runtime never produced anything to interrupt", async () => {
  const results = await executePiSessionRecoveryPhase0(client({ streaming: false }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /no running turn to interrupt/);
});
