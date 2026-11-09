import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createApiServer } from "../../api/app.js";
import { createRuntime, loadConfigFromEnv } from "../../server/runtime.js";

const unreachable = "http://127.0.0.1:9";

test("requires a Hermes base URL and validates the port", () => {
  assert.throws(() => loadConfigFromEnv({}), /HERMES_HTTP_BASE_URL is required/);
  assert.throws(
    () => loadConfigFromEnv({ HERMES_HTTP_BASE_URL: unreachable, DOCUFLOW_API_PORT: "nope" }),
    /DOCUFLOW_API_PORT/
  );
  assert.throws(
    () => loadConfigFromEnv({ HERMES_HTTP_BASE_URL: unreachable, DOCUFLOW_API_PORT: "70000" }),
    /DOCUFLOW_API_PORT/
  );
});

test("applies defaults and overrides", () => {
  const defaults = loadConfigFromEnv({ HERMES_HTTP_BASE_URL: unreachable });
  assert.equal(defaults.port, 8787);
  assert.equal(defaults.host, "127.0.0.1");
  assert.match(defaults.dataDir, /\.docuflow-data$/);
  assert.equal(defaults.token, undefined);

  const custom = loadConfigFromEnv({
    HERMES_HTTP_BASE_URL: "http://127.0.0.1:8642",
    DOCUFLOW_API_PORT: "9000",
    DOCUFLOW_API_HOST: "0.0.0.0",
    DOCUFLOW_API_TOKEN: "s3cret",
    DOCUFLOW_DATA_DIR: "./tmp-data",
    HERMES_TIMEOUT_MS: "1000"
  });
  assert.equal(custom.port, 9000);
  assert.equal(custom.host, "0.0.0.0");
  assert.equal(custom.token, "s3cret");
  assert.match(custom.dataDir, /tmp-data$/);
  assert.equal(custom.hermesTimeoutMs, 1000);
});

test("boots the API even when Hermes is unreachable", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "docuflow-serve-"));
  const runtime = createRuntime(loadConfigFromEnv({
    HERMES_HTTP_BASE_URL: unreachable,
    DOCUFLOW_DATA_DIR: dataDir
  }));
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);
  try {
    const runtimes = await fetch(`${server.url}/api/v1/runtimes`);
    const body = await runtimes.json() as { runtimes: Array<{ id: string; runtimeType: string }> };
    assert.equal(runtimes.status, 200);
    assert.deepEqual(body.runtimes.map(item => item.id), ["hermes-local"]);
    assert.equal(body.runtimes[0]!.runtimeType, "hermes");

    const health = await fetch(`${server.url}/api/v1/runtimes/hermes-local/health-check`, {
      method: "POST"
    });
    assert.equal(health.status, 503);

    const unknown = await fetch(`${server.url}/api/v1/executions/exec_missing`);
    assert.equal(unknown.status, 404);
  } finally {
    await server.close();
    await runtime.close();
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("applies the configured bearer token to every route", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "docuflow-serve-"));
  const runtime = createRuntime(loadConfigFromEnv({
    HERMES_HTTP_BASE_URL: unreachable,
    DOCUFLOW_DATA_DIR: dataDir,
    DOCUFLOW_API_TOKEN: "t0ken"
  }));

  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);
  try {
    assert.equal((await fetch(`${server.url}/api/v1/runtimes`)).status, 401);
    const authorized = await fetch(`${server.url}/api/v1/runtimes`, {
      headers: { authorization: "Bearer t0ken" }
    });
    assert.equal(authorized.status, 200);
  } finally {
    await server.close();
    await runtime.close();
    await rm(dataDir, { recursive: true, force: true });
  }
});
