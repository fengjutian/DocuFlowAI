import test from "node:test";
import assert from "node:assert/strict";
import { verifyHermesVersion, type HermesVersionLock } from "../../runners/check-hermes-version.js";

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
});

test("rejects a different Hermes version or commit", () => {
  assert.throws(() => verifyHermesVersion("Hermes Agent v0.22.6", lock), /Expected/);
  assert.throws(() => verifyHermesVersion("Hermes Agent v0.23.0 (deadbee)", lock), /commit/);
});
