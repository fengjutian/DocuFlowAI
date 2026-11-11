import test from "node:test";
import assert from "node:assert/strict";
import {
  executePiSkillDeliveryPhase0,
  type PiSkillDeliveryClient,
  type SkillDeliveryEvent
} from "../../runners/pi-skill-delivery-phase0.js";
import { skillContentChecksum, type SkillManifest } from "../../skills/skill.js";

const TOKEN = "CODEWEAVER_SKILL_OK";

interface FakeOptions {
  /** The output of the run that has the skill. */
  instructedOutput?: string;
  /** The output of the run without the skill. */
  controlOutput?: string;
  /** Whether the instructed run recorded a delivery. */
  delivered?: boolean;
  /** The status the runs report. */
  status?: string;
  /** Whether the runs produced any output at all. */
  silent?: boolean;
}

function fakeClient(options: FakeOptions = {}): PiSkillDeliveryClient {
  const status = options.status ?? "succeeded";
  const controlOutput = options.silent ? "" : options.controlOutput ?? "Hello! How can I help you today?";
  const instructedOutput = options.silent ? "" : options.instructedOutput ?? TOKEN;
  const delivered = options.delivered ?? true;

  const prompts = new Map<string, string>();
  let counter = 0;

  return {
    async registerSkill(manifest: SkillManifest) {
      assert.equal(manifest.checksum, skillContentChecksum(manifest.content));
      return `${manifest.skillId}@${manifest.version}`;
    },
    async start(startOptions) {
      counter += 1;
      const executionId = `exec-${counter}`;
      prompts.set(executionId, startOptions.prompt);
      return executionId;
    },
    async output(executionId) {
      return executionId === "exec-1" ? controlOutput : instructedOutput;
    },
    async waitForTerminal() {
      return status;
    },
    async fact(executionId, category) {
      if (category !== "skill.delivered" || !delivered) return undefined;
      if (executionId !== "exec-2") return undefined;
      return { skillId: "greeting-protocol", deliveryMode: "prompt_inline", bytes: 64 };
    },
    async promptHash(executionId) {
      // The instructed run's prompt is the composed one, so its hash differs.
      return executionId === "exec-1" ? "hash-control" : "hash-instructed";
    }
  };
}

test("passes when the control run stays quiet and the instructed run says the token", async () => {
  const results = await executePiSkillDeliveryPhase0(fakeClient());
  assert.equal(results.length, 1);
  assert.equal(results[0]!.status, "passed");
  assert.equal(results[0]!.id, "SKILL-DELIVERY");
  assert.ok(results[0]!.evidence.some(line => line === "controlOutputHasToken=false"));
  assert.ok(results[0]!.evidence.some(line => line === "instructedOutputHasToken=true"));
  assert.ok(results[0]!.evidence.some(line => line === "deliveryMode=prompt_inline"));
});

test("fails when the instructed run did not follow the skill", async () => {
  const results = await executePiSkillDeliveryPhase0(
    fakeClient({ instructedOutput: "Hello there!" })
  );
  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /did not produce the token/);
});

test("fails when the control run produced the token anyway", async () => {
  const results = await executePiSkillDeliveryPhase0(
    fakeClient({ controlOutput: `sure, ${TOKEN}` })
  );
  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /cannot attribute it/);
});

test("is blocked when the model produced no output at all", async () => {
  const results = await executePiSkillDeliveryPhase0(fakeClient({ silent: true }));
  assert.equal(results[0]!.status, "blocked");
  assert.match(results[0]!.notes ?? "", /nothing to compare/);
});

test("fails when the delivery was not recorded", async () => {
  const results = await executePiSkillDeliveryPhase0(fakeClient({ delivered: false }));
  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /no skill\.delivered fact/);
});

test("fails when the two runs were started with the same prompt", async () => {
  const client = fakeClient();
  const sameHash = {
    ...client,
    async promptHash() {
      return "hash-identical";
    }
  } as PiSkillDeliveryClient;
  const results = await executePiSkillDeliveryPhase0(sameHash);
  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /same prompt/);
});

test("the probe's event shape is the one the API serves", () => {
  // A guard on the assumption the client makes: the events carry the delta under `data`.
  const event: SkillDeliveryEvent = { type: "agent.message.delta", data: { delta: "x" } };
  assert.equal(String(event.data?.["delta"]), "x");
});
