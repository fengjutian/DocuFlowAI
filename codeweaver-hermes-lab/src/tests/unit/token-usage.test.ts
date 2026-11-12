import test from "node:test";
import assert from "node:assert/strict";
import { extractTokenUsage } from "../../accounting/token-usage.js";

/**
 * The extractor's rules.
 *
 * One implementation reads every runtime's payload, because two implementations are how a payload
 * ends up accounted for in one place and not the other — and the runtime that was missed was the
 * primary one. These tests pin the shapes the runtimes actually report.
 */

test("reads the gateway's shape", () => {
  assert.deepEqual(
    extractTokenUsage({ usage: { input_tokens: 26_199, output_tokens: 4_612, total_tokens: 30_811 } }),
    { promptTokens: 26_199, completionTokens: 4_612, totalTokens: 30_811 }
  );
});

test("reads the primary runtime's shape, and sums every model request", () => {
  const usage = extractTokenUsage({
    messages: [
      { role: "user", content: "hi" },
      { role: "assistant", model: "MiniMax-M3", usage: { input: 100, output: 20, cacheRead: 30, totalTokens: 150 } },
      { role: "assistant", model: "MiniMax-M3", usage: { input: 200, output: 40, cacheRead: 60, totalTokens: 300 } }
    ]
  });

  // Two assistant messages are two model requests: the run spent the sum, not the first one.
  assert.equal(usage.promptTokens, 300);
  assert.equal(usage.completionTokens, 60);
  assert.equal(usage.cachedTokens, 90);
  assert.equal(usage.totalTokens, 450);
  assert.equal(usage.model, "MiniMax-M3");
});

test("a runtime that never mentions a cache did not report zero cached tokens", () => {
  const usage = extractTokenUsage({ usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 } });

  // Saying zero would claim a fact the runtime never made.
  assert.equal(usage.cachedTokens, undefined);
  assert.equal(usage.cacheWriteTokens, undefined);
});

test("a runtime that reports a cache of zero is believed", () => {
  const usage = extractTokenUsage({
    messages: [{ role: "assistant", model: "m", usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0, totalTokens: 15 } }]
  });

  assert.equal(usage.cachedTokens, 0);
  assert.equal(usage.cacheWriteTokens, 0);
});

test("the parts explain the total, cache writes included", () => {
  const usage = extractTokenUsage({
    messages: [{ role: "assistant", model: "m", usage: { input: 10, output: 5, cacheRead: 30, cacheWrite: 7, totalTokens: 52 } }]
  });

  assert.equal(
    (usage.promptTokens ?? 0) + (usage.completionTokens ?? 0) + (usage.cachedTokens ?? 0) + (usage.cacheWriteTokens ?? 0),
    usage.totalTokens
  );
});

test("a payload with no usage yields nothing rather than a free run", () => {
  assert.deepEqual(extractTokenUsage({}), {});
  assert.deepEqual(extractTokenUsage({ output: "done" }), {});
  assert.deepEqual(extractTokenUsage({ messages: [{ role: "user", content: "hi" }] }), {});
});

test("negative counts are refused instead of recorded as nonsense", () => {
  const usage = extractTokenUsage({ usage: { input_tokens: -5, output_tokens: 10, total_tokens: 5 } });

  assert.equal(usage.promptTokens, undefined);
  assert.equal(usage.completionTokens, 10);
});

test("a runtime that reports the parts but not the sum still accounted for the run", () => {
  const usage = extractTokenUsage({ usage: { input_tokens: 100, output_tokens: 20 } });

  assert.equal(usage.totalTokens, 120);
});

test("the model comes from the payload, the usage or the assistant message", () => {
  assert.equal(extractTokenUsage({ model: "m-a", usage: { total_tokens: 1 } }).model, "m-a");
  assert.equal(extractTokenUsage({ usage: { model: "m-b", total_tokens: 1 } }).model, "m-b");
  assert.equal(
    extractTokenUsage({ messages: [{ role: "assistant", model: "m-c", usage: { totalTokens: 1 } }] }).model,
    "m-c"
  );
  assert.equal(extractTokenUsage({ usage: { total_tokens: 1 } }).model, undefined);
});
