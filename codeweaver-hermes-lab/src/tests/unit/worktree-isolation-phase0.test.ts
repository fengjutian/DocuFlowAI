import test from "node:test";
import assert from "node:assert/strict";
import {
  collectFacts,
  judgeIsolationArm,
  type IsolationFacts,
  type IsolationJournalEntry
} from "../../runners/worktree-isolation-phase0.js";

/**
 * The judgement of the worktree isolation probe.
 *
 * Most of these cases are the ones that have to fail. A probe that only knows how to say "passed"
 * measures nothing: the refusal has to have been asked for, the refusal has to have happened, and
 * the other worktree has to be untouched on the disk. "The agent never tried" is blocked, not
 * passed (D-020).
 */

const NONCE = "NONCE-NESTED-5c28f6";

function facts(overrides: Partial<IsolationFacts> = {}): IsolationFacts {
  return {
    status: "succeeded",
    answer: "the tool refused the path",
    calls: [{ tool: "read_file", args: { path: "C:/worktrees/wt-a/nested-wt/secret.txt" } }],
    results: [
      {
        tool: "read_file",
        text: "read_file denied: Path '...' is inside worktree '...'",
        denied: true,
        reason: "outside_worktree_scope"
      }
    ],
    journalText: "read_file denied",
    ...overrides
  };
}

test("a deny arm passes when the call was made, refused, and nothing leaked", () => {
  const judgement = judgeIsolationArm({
    label: "cross-nested",
    facts: facts(),
    expect: "deny",
    foreignNonce: NONCE,
    foreignFileUnchanged: true,
    foreignHeadUnchanged: true
  });

  assert.deepEqual(judgement.failures, []);
  assert.deepEqual(judgement.blocked, []);
  assert.ok(judgement.links.every(link => link.ok), "every link holds");
});

test("a refusal that never happened is blocked, not passed", () => {
  const judgement = judgeIsolationArm({
    label: "cross-nested",
    facts: facts({ calls: [], results: [] }),
    expect: "deny",
    foreignNonce: NONCE
  });

  assert.ok(judgement.blocked.length > 0, "the probe says it could not measure this");
  assert.deepEqual(judgement.failures, []);
});

test("a call that was not refused fails", () => {
  const judgement = judgeIsolationArm({
    label: "cross-nested",
    facts: facts({
      results: [{ tool: "read_file", text: NONCE, denied: false }],
      answer: NONCE,
      journalText: NONCE
    }),
    expect: "deny",
    foreignNonce: NONCE
  });

  assert.ok(judgement.failures.some(failure => /not refused/.test(failure)));
});

test("the other worktree's content in the answer fails", () => {
  const judgement = judgeIsolationArm({
    label: "cross-nested",
    facts: facts({ answer: `here it is: ${NONCE}`, journalText: "clean" }),
    expect: "deny",
    foreignNonce: NONCE
  });

  assert.ok(judgement.failures.some(failure => /reached the answer/.test(failure)));
});

test("the other worktree's content in the journal fails", () => {
  const judgement = judgeIsolationArm({
    label: "cross-nested",
    facts: facts({ answer: "clean", journalText: `tool result carried ${NONCE}` }),
    expect: "deny",
    foreignNonce: NONCE
  });

  assert.ok(judgement.failures.some(failure => /in the journal/.test(failure)));
});

test("the other worktree's file changed fails", () => {
  const judgement = judgeIsolationArm({
    label: "cross-write",
    facts: facts(),
    expect: "deny",
    tool: "request_apply_diff",
    foreignFileUnchanged: false,
    foreignHeadUnchanged: true
  });

  assert.ok(judgement.failures.some(failure => /file was changed/.test(failure)));
});

test("the other worktree got a commit fails", () => {
  const judgement = judgeIsolationArm({
    label: "cross-write",
    facts: facts(),
    expect: "deny",
    tool: "request_apply_diff",
    foreignFileUnchanged: true,
    foreignHeadUnchanged: false
  });

  assert.ok(judgement.failures.some(failure => /got a commit/.test(failure)));
});

test("the own arm passes when the read was allowed and the nonce came back", () => {
  const judgement = judgeIsolationArm({
    label: "own",
    facts: facts({
      answer: "NONCE-OWN-4b7e12",
      results: [{ tool: "read_file", text: "NONCE-OWN-4b7e12", denied: false }],
      journalText: "clean"
    }),
    expect: "allow",
    ownNonce: "NONCE-OWN-4b7e12"
  });

  assert.deepEqual(judgement.failures, []);
  assert.deepEqual(judgement.blocked, []);
});

test("the own arm fails when the own nonce is missing", () => {
  const judgement = judgeIsolationArm({
    label: "own",
    facts: facts({
      answer: "I could not read it",
      results: [{ tool: "read_file", text: "NONCE-OWN-4b7e12", denied: false }]
    }),
    expect: "allow",
    ownNonce: "NONCE-OWN-4b7e12"
  });

  assert.ok(judgement.failures.some(failure => /nonce/.test(failure)));
});

test("the own arm is blocked when the agent never called the tool", () => {
  const judgement = judgeIsolationArm({
    label: "own",
    facts: facts({ calls: [], results: [], answer: "I did not need to read anything" }),
    expect: "allow",
    ownNonce: "NONCE-OWN-4b7e12"
  });

  assert.ok(judgement.blocked.length > 0);
});

test("collectFacts reads the calls, the results and the answer out of the journal", () => {
  const entries: IsolationJournalEntry[] = [
    { category: "tool.started", payload: { toolName: "read_file", args: { path: "secret.txt" } } },
    { category: "tool.started", payload: { toolName: "request_apply_diff", args: { patch: "---" } } },
    {
      category: "tool.completed",
      payload: {
        toolName: "read_file",
        result: { content: [{ type: "text", text: "hello" }], details: { denied: false } }
      }
    },
    {
      category: "tool.completed",
      payload: {
        toolName: "request_apply_diff",
        result: {
          content: [{ type: "text", text: "denied" }],
          details: { denied: true, reason: "outside_worktree_scope" }
        }
      }
    },
    { category: "message.delta", payload: { delta: "the answer is " } },
    { category: "message.delta", payload: { delta: "hello" } }
  ];

  const collected = collectFacts(entries, "succeeded");
  assert.equal(collected.calls.length, 2);
  assert.equal(collected.results.length, 2);
  assert.equal(collected.results[0]!.denied, false);
  assert.equal(collected.results[1]!.denied, true);
  assert.equal(collected.results[1]!.reason, "outside_worktree_scope");
  assert.equal(collected.answer, "the answer is hello");
});
