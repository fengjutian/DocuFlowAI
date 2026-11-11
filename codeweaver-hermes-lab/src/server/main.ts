import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { createRuntime, loadConfigFromEnv } from "./runtime.js";

/**
 * Lab server entry point. Usage:
 *
 *   HERMES_HTTP_BASE_URL=http://127.0.0.1:8642 npm run serve
 *
 * Data (journal, snapshots, change records) lives under CODEWEAVER_DATA_DIR, default `.codeweaver-data`.
 * This is a validation lab, not a hardened deployment: bind to loopback and set CODEWEAVER_API_TOKEN.
 */
async function main(): Promise<void> {
  const config = loadConfigFromEnv();
  const runtime = createRuntime(config);
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(config.port, config.host);

  console.log(`CodeWeaver lab server: ${server.url}`);
  console.log(`  data:      ${resolve(config.dataDir)}`);
  console.log(`  worktrees: ${resolve(config.worktreeRoot)}`);
  console.log(`  hermes:    ${config.hermesBaseUrl}`);
  console.log(`  auth:      ${config.token ? "bearer token required" : "open (loopback only)"}`);

  let closing = false;
  const shutdown = async (signal: string): Promise<void> => {
    if (closing) return;
    closing = true;
    console.log(`Shutting down on ${signal}`);
    await runtime.close().catch(() => undefined);
    await server.close().catch(() => undefined);
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown("SIGINT"));
  process.on("SIGTERM", () => void shutdown("SIGTERM"));
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  try {
    await main();
  } catch (error) {
    console.error(`Failed to start: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}
