// Probe: the CodeWeaver tool surface end to end against the real MiniMax CN model.
//
// HTTP API -> TaskExecutionService -> ExecutionCoordinator -> Pi runtime (pi-agent-core)
//   -> CodeWeaver tool surface -> Code Engine -> Evaluation Engine
//
// This is the proof that the tool surface is really wired: the model has to call
// read_file / request_apply_diff / evaluate_changes, and the change has to land in the
// worktree through the Code Engine with an evaluation verdict behind it.
//
// The key comes from the environment (MINIMAX_CN_API_KEY); nothing is embedded here.
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createApiServer } from "../dist/api/app.js";
import { createRuntime } from "../dist/server/runtime.js";

const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
if (!process.env.MINIMAX_CN_API_KEY) {
  console.error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  process.exit(1);
}

function git(args, cwd) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  }
  return result.stdout;
}

const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-tools-repo-"));
const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-tools-data-"));

await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
await writeFile(join(repo, "spec.md"), "# Spec\nThe value must become 2.\n", "utf8");
await writeFile(
  join(repo, "check.mjs"),
  [
    "import { readFileSync } from 'node:fs';",
    "const content = readFileSync(new URL('./app.ts', import.meta.url), 'utf8');",
    "if (!content.includes('value = 2')) {",
    "  console.error('app.ts still holds the old value');",
    "  process.exit(1);",
    "}",
    "console.log('app.ts holds value = 2');",
    ""
  ].join("\n"),
  "utf8"
);
git(["init", "-b", "main"], repo);
git(["config", "user.name", "CodeWeaver Probe"], repo);
git(["config", "user.email", "probe@codeweaver.local"], repo);
git(["add", "-A"], repo);
git(["commit", "-m", "initial"], repo);

const runtime = createRuntime({
  host: "127.0.0.1",
  port: 0,
  dataDir,
  worktreeRoot: join(dataDir, "worktrees"),
  repository: repo,
  // Hermes is registered so the server has a default runtime; this probe never talks to it.
  hermesBaseUrl: "http://127.0.0.1:9",
  hermesTimeoutMs: 5_000,
  pi: { provider: "minimax-cn", model }
});
await runtime.ready();
const server = await createApiServer(runtime.deps).listen(0);

const prompt = [
  "Work in the repository you are given. Do exactly these three steps, using the tools:",
  "1. Call read_file with path 'app.ts' and look at what it contains.",
  "2. Call request_apply_diff with a unified diff that replaces the single line",
  "   'export const value = 1;' with 'export const value = 2;'. The diff must use the",
  "   a/app.ts and b/app.ts prefixes and one hunk, so that git apply accepts it.",
  "3. Call evaluate_changes to run the acceptance checks of this task.",
  "When the change is applied, reply with exactly: CODEWEAVER_PI_TOOLS_OK"
].join("\n");

const start = await fetch(`${server.url}/api/v1/tasks/task-pi-tools/executions`, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({
    projectId: "probe-pi-tools",
    prompt,
    worktreePath: repo,
    documents: ["spec.md"],
    selectedFiles: ["app.ts"],
    architectureVersion: "1.0",
    policy: {
      snapshotId: "snap-probe",
      projectId: "probe-pi-tools",
      taskId: "task-pi-tools",
      executionId: "",
      worktreeId: "wt-probe",
      policyVersion: "1.0.0",
      grantedCapabilities: ["*"],
      allowedPaths: [repo],
      expiresAt: "2099-01-01T00:00:00.000Z"
    },
    template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
    modelConfig: { model },
    correlationId: "probe-pi-tools",
    timeoutSeconds: 300,
    allowedTools: ["read_file", "request_diff", "request_apply_diff", "evaluate_changes"],
    runtimeId: "pi-local",
    evaluationChecks: [
      { id: "check", name: "app.ts holds the new value", command: "node", args: ["check.mjs"], cwd: repo }
    ]
  })
});

if (start.status !== 201) {
  console.error("the task was refused:", start.status, await start.text());
  process.exit(1);
}
const created = await start.json();
const executionId = created.execution.executionId;

const deadline = Date.now() + 300_000;
let status = "unknown";
while (Date.now() < deadline) {
  const response = await fetch(`${server.url}/api/v1/executions/${executionId}`);
  const body = await response.json();
  status = body.status;
  if (["succeeded", "failed", "cancelled", "interrupted"].includes(status)) break;
  await new Promise(resolve => setTimeout(resolve, 1_000));
}

const events = (await (await fetch(`${server.url}/api/v1/executions/${executionId}/events`)).json()).events;
// The events endpoint serves the design-13 envelope: `type` is "agent.<event>", `data` the payload.
const toolEvents = events
  .filter(event => String(event.type).startsWith("agent.tool."))
  .map(event => `${event.type} ${event.data?.toolName ?? ""}`);

const applied = await readFile(join(repo, "app.ts"), "utf8");
const trace = await (await fetch(`${server.url}/api/v1/executions/${executionId}/trace`)).json();
const evaluation = await (await fetch(`${server.url}/api/v1/executions/${executionId}`)).json();
const assistant = events
  .filter(event => event.type === "agent.message.delta")
  .map(event => event.data?.delta ?? "")
  .join("");

const usedTools = new Set(toolEvents.map(item => item.split(" ")[1]));
const checks = {
  executionFinished: status === "succeeded",
  usedReadFile: usedTools.has("read_file"),
  usedApplyDiff: usedTools.has("request_apply_diff"),
  usedEvaluation: usedTools.has("evaluate_changes"),
  worktreeChanged: applied.includes("value = 2"),
  changeRecorded: (trace.changeRecords ?? []).length > 0,
  completionGateOpen: evaluation.completion?.allowed === true
};

console.log("status:", status);
console.log("tool events:", toolEvents.join(", ") || "(none)");
console.log("worktree app.ts:", JSON.stringify(applied));
console.log("change records:", (trace.changeRecords ?? []).length);
console.log("completion gate:", JSON.stringify(evaluation.completion));
console.log("assistant reply:", assistant.trim().slice(0, 200));
console.log("checks:", JSON.stringify(checks, null, 2));

const pass = Object.values(checks).every(Boolean);
console.log("ok:", pass ? "PASS" : "FAIL");
if (!pass) {
  const failures = events.filter(event => event.type === "agent.execution.failed");
  console.log("failure detail:", JSON.stringify(failures.map(event => event.data)));
  process.exitCode = 1;
}

await server.close();
await runtime.close();
await rm(repo, { recursive: true, force: true });
await rm(dataDir, { recursive: true, force: true });
