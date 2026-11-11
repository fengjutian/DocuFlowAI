// Probe: run the CodeWeaver Pi adapter against the real MiniMax CN model.
//
// Unlike probe-pi-agent.mjs (faux provider, no network), this one talks to
// https://api.minimaxi.com/anthropic with a real key. It proves the whole path:
// CodeWeaver adapter -> pi-agent-core -> MiniMax CN -> back through the CodeWeaver event stream.
//
// The key comes from the environment (MINIMAX_CN_API_KEY); nothing is embedded here.
import { createModels } from "@earendil-works/pi-ai";
import { minimaxCnProvider } from "@earendil-works/pi-ai/providers/minimax-cn";
import { PiAdapter } from "../dist/adapters/pi-adapter.js";
import { createPiAgentFactory } from "../dist/adapters/pi-agent-factory.js";

const modelId = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
if (!process.env.MINIMAX_CN_API_KEY) {
  console.error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  process.exit(1);
}

const models = createModels();
models.setProvider(minimaxCnProvider());
const model = models.getModel("minimax-cn", modelId);
if (!model) {
  console.error(`Unknown model ${modelId} for provider minimax-cn`);
  process.exit(1);
}

const adapter = new PiAdapter(createPiAgentFactory({ models, model }), {
  systemPrompt: "You are a helpful assistant. Answer with exactly the requested token."
});

const execution = await adapter.start({
  projectId: "probe",
  taskId: "probe-pi-minimax",
  runId: "probe",
  prompt: "Reply with exactly: CODEWEAVER_PI_OK",
  contextSnapshotId: "ctx-probe",
  policySnapshotId: "pol-probe",
  worktreeId: "wt-probe",
  workingDirectory: process.cwd(),
  allowedTools: [],
  timeoutSeconds: 120,
  correlationId: "probe"
});

const events = [];
await adapter.subscribe(execution.executionId, event => events.push(event));

const deadline = Date.now() + 120_000;
let status = await adapter.getStatus(execution.executionId);
while (!["succeeded", "failed", "cancelled", "interrupted"].includes(status) && Date.now() < deadline) {
  await new Promise(resolve => setTimeout(resolve, 500));
  status = await adapter.getStatus(execution.executionId);
}

const deltas = events
  .filter(event => event.eventType === "message.delta")
  .map(event => event.payload.delta)
  .join("");

console.log("status:", status);
console.log("event types:", [...new Set(events.map(event => event.eventType))].join(", "));
console.log("assistant reply:", deltas.trim());
console.log(
  "ok:",
  status === "succeeded" && deltas.includes("CODEWEAVER_PI_OK") ? "PASS" : "FAIL"
);
if (status !== "succeeded" || !deltas.includes("CODEWEAVER_PI_OK")) {
  console.log(
    "failure detail:",
    JSON.stringify(events.filter(e => e.eventType === "execution.failed").map(e => e.payload))
  );
  process.exitCode = 1;
}
