import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { HermesAdapter } from "../adapters/hermes-adapter.js";
import { createDocuFlowAcpBackend } from "../acp/docuflow-backend.js";
import { AcpAgentServer } from "../acp/server.js";
import { HermesHttpClient } from "../clients/http/hermes-http-client.js";
import { JsonlJournalStore } from "../journal/journal.js";
import { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";

/**
 * ACP agent endpoint for Zed (`agent_servers` spawns this command and talks JSON-RPC over stdio).
 *
 * Zed → ACP → DocuFlow → Hermes, per design 4.4. Sessions are workspace-scoped; each prompt starts a
 * DocuFlow execution that is journaled like any platform run.
 *
 * Scope note: this entry does not yet create context/policy snapshot records or enforce the policy
 * snapshot — the agent's confinement depends on Hermes running inside the verified sandbox. Treat it as
 * an editor-integration skeleton until those pieces are wired.
 */
async function main(): Promise<void> {
  const baseUrl = process.env["HERMES_HTTP_BASE_URL"];
  if (!baseUrl) {
    process.stderr.write("HERMES_HTTP_BASE_URL is required to run the ACP endpoint\n");
    process.exitCode = 1;
    return;
  }
  const dataDir = resolve(process.env["DOCUFLOW_DATA_DIR"] ?? ".docuflow-data");
  const client = new HermesHttpClient({
    baseUrl,
    ...(process.env["HERMES_API_KEY"] ? { apiKey: process.env["HERMES_API_KEY"] } : {}),
    timeoutMs: Number(process.env["HERMES_TIMEOUT_MS"] ?? 30_000)
  });
  const coordinator = new ExecutionCoordinator({
    adapter: new HermesAdapter(client),
    journal: new JsonlJournalStore(resolve(dataDir, "journal"))
  });

  const backend = createDocuFlowAcpBackend({
    coordinator,
    defaults: {
      projectId: process.env["DOCUFLOW_ACP_PROJECT_ID"] ?? "zed",
      contextSnapshotId: process.env["DOCUFLOW_ACP_CONTEXT_SNAPSHOT"] ?? "ctx-zed",
      policySnapshotId: process.env["DOCUFLOW_ACP_POLICY_SNAPSHOT"] ?? "pol-zed",
      allowedTools: ["read_file", "search_code", "generate_diff"],
      timeoutSeconds: Number(process.env["DOCUFLOW_ACP_TIMEOUT_SECONDS"] ?? 300)
    }
  });

  const server = new AcpAgentServer(backend, { input: process.stdin, output: process.stdout });
  server.start();
  process.stderr.write("DocuFlow ACP endpoint ready on stdio\n");

  const shutdown = async (): Promise<void> => {
    server.stop();
    await coordinator.stopAll().catch(() => undefined);
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown());
  process.on("SIGTERM", () => void shutdown());
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main().catch(error => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
