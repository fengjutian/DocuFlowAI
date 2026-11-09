import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createDocuFlowAcpBackend } from "../acp/docuflow-backend.js";
import { AcpAgentServer } from "../acp/server.js";
import { createRuntime, loadConfigFromEnv } from "../server/runtime.js";
import type { PolicySnapshot } from "../policy/permission-policy.js";

/**
 * ACP agent endpoint for Zed (`agent_servers` spawns this command and talks JSON-RPC over stdio).
 *
 * Zed → ACP → DocuFlow → Hermes, per design 4.4. Sessions are workspace-scoped; each prompt starts a
 * DocuFlow execution through `TaskExecutionService`, so the editor run gets context/policy/prompt
 * snapshots and audit entries like any platform run. A cwd that is not a git repository degrades to a
 * journal-only run with a warning, because a snapshot that cannot be captured must not be invented.
 */
async function main(): Promise<void> {
  let config;
  try {
    config = loadConfigFromEnv();
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
    return;
  }

  const runtime = createRuntime(config);
  await runtime.ready();

  const projectId = process.env["DOCUFLOW_ACP_PROJECT_ID"] ?? "zed";
  const timeoutSeconds = Number(process.env["DOCUFLOW_ACP_TIMEOUT_SECONDS"] ?? 300);

  const policyFor = ({ cwd, taskId }: { cwd: string; taskId: string }): PolicySnapshot => ({
    snapshotId: `pol_acp_${taskId}`,
    projectId,
    taskId,
    executionId: "",
    worktreeId: taskId,
    policyVersion: "acp-default-1",
    grantedCapabilities: ["read_file", "search_code", "generate_diff"],
    allowedPaths: [cwd],
    expiresAt: new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString()
  });

  let server: AcpAgentServer | undefined;
  const backend = createDocuFlowAcpBackend({
    coordinator: runtime.deps.coordinator,
    defaults: {
      projectId,
      contextSnapshotId: "acp-context",
      policySnapshotId: "acp-policy",
      allowedTools: ["read_file", "search_code", "generate_diff"],
      timeoutSeconds
    },
    snapshots: { taskService: runtime.deps.taskService, policyFor },
    onWarning: message => process.stderr.write(`[acp] ${message}\n`),
    requestPermission: async request => {
      if (!server) return "cancelled";
      const result = await server.request("session/request_permission", {
        sessionId: request.sessionId,
        toolCall: {
          toolCallId: request.toolCallId,
          title: request.title,
          kind: "other",
          status: "pending"
        },
        options: request.options
      }) as { outcome?: { outcome?: string; optionId?: string } } | undefined;
      const outcome = result?.outcome;
      if (outcome?.outcome === "selected" && outcome.optionId) return { optionId: outcome.optionId };
      return "cancelled";
    }
  });

  server = new AcpAgentServer(backend, { input: process.stdin, output: process.stdout });
  server.start();
  process.stderr.write("DocuFlow ACP endpoint ready on stdio\n");

  const shutdown = async (): Promise<void> => {
    server?.stop();
    await runtime.close().catch(() => undefined);
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
