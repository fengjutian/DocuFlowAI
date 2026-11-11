// An end-to-end bridge for the editor's agent path.
//
// It starts the mock Hermes runtime, spawns the *real* ACP agent endpoint
// against it, and relays stdio in both directions. The editor's ACP client
// therefore talks to the production endpoint — the real backend, the real
// policy and snapshot handling — with only the runtime behind it mocked.
//
// Usage: node acp-e2e-bridge.mjs

import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const repoRoot = process.env.CODEWEAVER_REPO_ROOT
  ? resolve(process.env.CODEWEAVER_REPO_ROOT)
  : resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../../..");

const { startMockHermes } = await import(
  pathToFileURL(
    resolve(repoRoot, "codeweaver-hermes-lab/dist/tests/support/mock-hermes-server.js")
  ).href
);

const hermes = await startMockHermes({
  events: [
    { event: "run.started", data: {}, id: "1" },
    { event: "message.delta", data: { text: "hello " }, id: "2" },
    { event: "message.delta", data: { text: "from the agent" }, id: "3" },
    { event: "run.completed", data: { output: "hello from the agent" }, id: "4" }
  ]
});

const endpoint = resolve(repoRoot, "codeweaver-hermes-lab/dist/runners/acp-server.js");
const dataDir = mkdtempSync(join(tmpdir(), "codeweaver-acp-e2e-"));

const child = spawn("node", [endpoint], {
  env: {
    ...process.env,
    HERMES_HTTP_BASE_URL: hermes.url,
    CODEWEAVER_DATA_DIR: dataDir
  },
  stdio: ["pipe", "pipe", "inherit"]
});

process.stdin.pipe(child.stdin);
child.stdout.pipe(process.stdout);

const shutdown = async () => {
  child.kill();
  await hermes.close().catch(() => undefined);
  process.exit(0);
};

process.on("SIGINT", () => void shutdown());
process.on("SIGTERM", () => void shutdown());
child.on("exit", (code) => {
  void hermes.close().catch(() => undefined);
  process.exit(code ?? 0);
});
