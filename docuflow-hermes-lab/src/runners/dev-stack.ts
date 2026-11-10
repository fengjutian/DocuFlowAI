import { startMockHermes } from "../tests/support/mock-hermes-server.js";
import { createRuntime, loadConfigFromEnv } from "../server/runtime.js";
import { createApiServer } from "../api/app.js";

/**
 * Runs the API server against the lab's mock Hermes runtime.
 *
 * The desktop client needs a backend to talk to, and the real Hermes runtime is not always present
 * on a developer machine. The mock is the same one the contract tests use, so the event shapes the
 * UI sees here are the shapes the adapter is specified to produce — but a mock is not proof that
 * the pinned Hermes version behaves the same way.
 *
 * Usage:
 *   npm run dev:stack
 */
async function main(): Promise<void> {
  const mock = await startMockHermes({ payloadNamedEvents: true });
  process.env["HERMES_HTTP_BASE_URL"] = mock.url;

  const config = loadConfigFromEnv();
  const runtime = createRuntime(config);
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(config.port, config.host);

  console.log(`DocuFlow lab stack: ${server.url}`);
  console.log(`  mock hermes: ${mock.url}`);
  console.log(`  data:        ${config.dataDir}`);
  console.log(`  auth:        ${config.token ? "bearer token required" : "open (loopback only)"}`);

  let closing = false;
  const shutdown = async (signal: string) => {
    if (closing) return;
    closing = true;
    console.log(`Shutting down on ${signal}`);
    await runtime.close().catch(() => undefined);
    await server.close().catch(() => undefined);
    await mock.close().catch(() => undefined);
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown("SIGINT"));
  process.on("SIGTERM", () => void shutdown("SIGTERM"));
}

main().catch((error: unknown) => {
  console.error(`Failed to start the lab stack: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
