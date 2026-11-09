import test from "node:test";
import assert from "node:assert/strict";
import { HermesHttpClient } from "../../clients/http/hermes-http-client.js";

const baseUrl = process.env.HERMES_HTTP_BASE_URL;

test("Hermes HTTP API creates a run and returns an identifier", { skip: !baseUrl }, async () => {
  const client = new HermesHttpClient({
    baseUrl: baseUrl!,
    ...(process.env.HERMES_API_KEY ? { apiKey: process.env.HERMES_API_KEY } : {}),
    timeoutMs: Number(process.env.HERMES_TIMEOUT_MS ?? 30_000)
  });
  const run = await client.createRun({
    input: "Reply with exactly: DOCUFLOW_PHASE0_OK",
    ...(process.env.HERMES_MODEL ? { model: process.env.HERMES_MODEL } : {})
  });
  assert.equal(typeof run.id, "string", "response must include a run id");
  assert.ok(run.id.length > 0);
  const status = await client.getRun(run.id);
  assert.equal(status.id, run.id);
});
