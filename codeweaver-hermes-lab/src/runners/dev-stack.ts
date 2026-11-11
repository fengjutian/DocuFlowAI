import { startMockHermes } from "../tests/support/mock-hermes-server.js";
import { createRuntime, loadConfigFromEnv } from "../server/runtime.js";
import { createApiServer } from "../api/app.js";

/**
 * Runs the API server against a Hermes runtime.
 *
 * Three modes, chosen by `CODEWEAVER_RUNTIME`:
 *
 *   mock   — the lab's own mock Hermes server. The desktop client needs a backend to talk to, and
 *            the real runtime is not always present on a developer machine. The mock is the same
 *            one the contract tests use, so the event shapes the UI sees here are the shapes the
 *            adapter is specified to produce — but a mock is not proof that the pinned Hermes
 *            version behaves the same way.
 *
 *   pi     — the primary runtime (D-042, D-060). Pi runs in-process, so it needs no separate
 *            process; the model and the API key come from the environment. The mock stays
 *            registered next to it, so the console shows both and the named-runtime rule (D-047)
 *            is exercisable without a second install.
 *
 *   hermes — the pinned remote runtime, reached over HTTP. The base URL and API key come from the
 *            environment, because they describe an installation this script did not perform: the
 *            profile's `.env` owns the key, and the port is the one that installation listens on.
 *
 * The mode is explicit rather than inferred. A stack that silently connected to whatever happened
 * to be on 8642 would report the real runtime's answers under the mock's name, and the difference
 * between "the UI works" and "the pinned runtime works" is exactly what this lab exists to prove.
 *
 * Usage:
 *   CODEWEAVER_RUNTIME=mock npm run dev:stack
 *   CODEWEAVER_RUNTIME=pi PI_MODEL=MiniMax-M3 MINIMAX_CN_API_KEY=... npm run dev:stack
 *   CODEWEAVER_RUNTIME=hermes HERMES_HTTP_BASE_URL=http://127.0.0.1:8642 \
 *     HERMES_API_KEY=... npm run dev:stack
 */
async function main(): Promise<void> {
  const mode = (process.env["CODEWEAVER_RUNTIME"] ?? "mock").trim().toLowerCase();
  let mockUrl: string | undefined;

  if (mode === "mock" || mode === "pi") {
    // Pi runs in-process, so the mock is still the reference runtime it sits next to.
    const mock = await startMockHermes({ payloadNamedEvents: true });
    mockUrl = mock.url;
    process.env["HERMES_HTTP_BASE_URL"] = mock.url;
  } else if (mode === "hermes") {
    // The base URL must already be set: this process does not install the runtime and must not
    // guess which one is meant. A missing URL is a configuration error, not a reason to fall back
    // to the mock, because the fallback would hide exactly the thing under test.
    const baseUrl = process.env["HERMES_HTTP_BASE_URL"]?.trim();
    if (!baseUrl) {
      throw new Error(
        "CODEWEAVER_RUNTIME=hermes requires HERMES_HTTP_BASE_URL (for example http://127.0.0.1:8642)"
      );
    }
  } else {
    throw new Error(`Unknown CODEWEAVER_RUNTIME '${mode}'; expected 'mock', 'pi' or 'hermes'`);
  }

  if (mode === "pi" && !process.env["PI_MODEL"]?.trim()) {
    throw new Error("CODEWEAVER_RUNTIME=pi requires PI_MODEL (for example MiniMax-M3)");
  }

  const config = loadConfigFromEnv();
  const runtime = createRuntime(config);
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(config.port, config.host);

  console.log(`CodeWeaver lab stack: ${server.url}`);
  console.log(`  runtime:    ${mode}${mockUrl ? ` (${mockUrl})` : ` (${config.hermesBaseUrl})`}`);
  console.log(`  data:       ${config.dataDir}`);
  console.log(`  auth:       ${config.token ? "bearer token required" : "open (loopback only)"}`);

  let closing = false;
  const shutdown = async (signal: string) => {
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

main().catch((error: unknown) => {
  console.error(`Failed to start the lab stack: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
