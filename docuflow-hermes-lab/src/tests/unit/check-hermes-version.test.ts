import test from "node:test";
import assert from "node:assert/strict";
import {
  extractVersion,
  verifyHermesVersion,
  type HermesVersionLock
} from "../../runners/check-hermes-version.js";

const lock: HermesVersionLock = {
  repository: "https://github.com/NousResearch/hermes-agent",
  tag: "v0.23.0",
  version: "0.23.0",
  commit: "2bd378ed1b8d6bf2192cb26e0a55ff3091e6bd19",
  releasedAt: "2026-11-05"
};

test("accepts the pinned Hermes version with or without a commit", () => {
  assert.doesNotThrow(() => verifyHermesVersion("Hermes Agent v0.23.0", lock));
  assert.doesNotThrow(() => verifyHermesVersion("hermes 0.23.0 (2bd378e)", lock));
  assert.doesNotThrow(() => verifyHermesVersion("0.23.0", lock));
});

test("rejects a different Hermes version or commit", () => {
  assert.throws(() => verifyHermesVersion("Hermes Agent v0.22.6", lock), /0\.22\.6/);
  assert.throws(
    () => verifyHermesVersion("Hermes Agent v0.23.0 (deadbee)", lock),
    /commit/
  );
});

test("explains the provider-initialization trap instead of pretending it is a version", () => {
  const output = [
    "AI Agent with Tool Calling",
    "Failed to initialize agent: No LLM provider configured. Run `hermes model` to select a provider"
  ].join("\n");

  assert.throws(() => verifyHermesVersion(output, lock), /HERMES_PYTHON/);
});

test("extracts the first dotted version from any output shape", () => {
  assert.equal(extractVersion("hermes-agent 0.15.1"), "0.15.1");
  assert.equal(extractVersion("v1.2.3-rc.1 extra"), "1.2.3-rc.1");
  assert.equal(extractVersion("no version here"), undefined);
});
