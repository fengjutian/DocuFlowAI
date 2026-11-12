import test from "node:test";
import assert from "node:assert/strict";
import {
  judgeParallelArm,
  readAndAnswerPrompt,
  type ParallelArmFacts
} from "../../runners/parallel-agents-phase0.js";

/**
 * The judgement of a parallel arm, tested the way it has to work: mostly on the ways it has to say no.
 *
 * The criterion is "两个 Agent 同时执行，各自独立运行". Two claims hide in it, and the second one is
 * the one a broken system passes by accident: a system that runs the two tasks one after the other
 * has run two agents, and it has never had to keep them apart. So the cases below break one thing
 * each — the runs never coexisted, nobody saw the first one still running, the answers crossed, a
 * run ended twice, the shared timeline lost or invented an event, the ledger kept only one of the
 * two runs — and the judgement has to refuse.
 */

const FIRST_NONCE = "NONCE-ALPHA-7f3a91";
const SECOND_NONCE = "NONCE-BETA-9c2e04";

function facts(overrides: Partial<ParallelArmFacts> = {}): ParallelArmFacts {
  return {
    executionId: "exec-first",
    status: "succeeded",
    startedAt: "2026-11-12T00:00:00.000Z",
    finishedAt: "2026-11-12T00:00:05.000Z",
    durationMs: 5_000,
    terminalCount: 1,
    terminalCategory: "execution.completed",
    answer: `the file holds ${FIRST_NONCE}`,
    journalCount: 20,
    timelineCount: 20,
    usage: {
      id: "use_first",
      executionId: "exec-first",
      model: "MiniMax-M3",
      promptTokens: 100,
      completionTokens: 20,
      totalTokens: 120,
      durationMs: 5_000
    },
    ...overrides
  };
}

function secondFacts(overrides: Partial<ParallelArmFacts> = {}): ParallelArmFacts {
  return facts({
    executionId: "exec-second",
    answer: `the file holds ${SECOND_NONCE}`,
    journalCount: 18,
    timelineCount: 18,
    usage: {
      id: "use_second",
      executionId: "exec-second",
      model: "MiniMax-M3",
      promptTokens: 110,
      completionTokens: 25,
      totalTokens: 135,
      durationMs: 4_000
    },
    ...overrides
  });
}

function judge(first = facts(), second = secondFacts(), observedOverlap = true) {
  return judgeParallelArm({
    label: "top",
    first,
    second,
    firstNonce: FIRST_NONCE,
    secondNonce: SECOND_NONCE,
    observedOverlap
  });
}

function link(links: { name: string; ok: boolean }[], name: string): boolean {
  return links.find(item => item.name === name)?.ok ?? false;
}

test("two runs that overlapped and stayed apart pass every link", () => {
  const judgement = judge();

  assert.equal(judgement.ok, true);
  assert.equal(judgement.links.length, 5);
});

test("runs that never coexisted are not a parallel run", () => {
  // The second one started after the first one was already over.
  const judgement = judge(facts(), secondFacts({ startedAt: "2026-11-12T00:00:06.000Z" }));

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "overlapped"), false);
});

test("overlapping timestamps alone are not enough — the first run has to have been seen running", () => {
  const judgement = judge(facts(), secondFacts(), false);

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "overlapped"), false);
});

test("two executions that share an id are not two runs", () => {
  const judgement = judge(facts(), secondFacts({ executionId: "exec-first" }));

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "two-runs"), false);
});

test("a run that did not succeed is not a finished run", () => {
  const judgement = judge(facts({ status: "failed" }));

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "two-runs"), false);
});

test("an answer that carries the other run's nonce is a crossed answer", () => {
  const judgement = judge(
    facts({ answer: `the file holds ${SECOND_NONCE}` }),
    secondFacts({ answer: `the file holds ${SECOND_NONCE}` })
  );

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "own-answer"), false);
});

test("a run that ended twice is an ambiguous run", () => {
  const judgement = judge(facts({ terminalCount: 2 }));

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "own-journal"), false);
});

test("a shared timeline that lost or invented an event does not match the journals", () => {
  const judgement = judge(facts({ timelineCount: 18 }));

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "own-journal"), false);
});

test("the ledger has to keep one record per run", () => {
  const withoutUsage = secondFacts();
  delete withoutUsage.usage;
  const judgement = judge(facts(), withoutUsage);

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "separate-usage"), false);
});

test("a ledger that attributed a run's cost to the other run is refused", () => {
  const judgement = judge(
    facts(),
    secondFacts({
      usage: {
        id: "use_first",
        executionId: "exec-first",
        model: "MiniMax-M3",
        promptTokens: 110,
        completionTokens: 25,
        totalTokens: 135,
        durationMs: 4_000
      }
    })
  );

  assert.equal(judgement.ok, false);
  assert.equal(link(judgement.links, "separate-usage"), false);
});

test("the prompt names the file the agent has to read", () => {
  const prompt = readAndAnswerPrompt("secret.txt");

  assert.ok(prompt.includes("secret.txt"));
  assert.ok(prompt.includes("read_file"));
});
