import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { HermesAdapter } from "../adapters/hermes-adapter.js";
import { HermesHttpClient } from "../clients/http/hermes-http-client.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { AGENT_PROBES, executeAgentSecurityPhase0, type AgentProbe } from "../security/agent-probes.js";
import type { AgentRuntimeEvent, AgentTaskInput } from "../types/adapter.js";
import type { TestResult } from "../types/hermes.js";

const TERMINAL = new Set(["execution.completed", "execution.failed", "execution.cancelled"]);

async function runProbeWithAdapter(
  adapter: HermesAdapter,
  probe: AgentProbe,
  options: { workspace: string; timeoutMs: number; correlationId: string }
): Promise<{ events: AgentRuntimeEvent[]; terminalStatus: string }> {
  const input: AgentTaskInput = {
    projectId: "phase0-security",
    taskId: `sec-${probe.id.toLowerCase()}`,
    runId: `sec-run-${probe.id}-${Date.now()}`,
    prompt: probe.prompt,
    contextSnapshotId: "phase0-context",
    policySnapshotId: "phase0-policy",
    worktreeId: "phase0-worktree",
    workingDirectory: options.workspace,
    allowedTools: [],
    timeoutSeconds: Math.ceil(options.timeoutMs / 1000),
    correlationId: options.correlationId
  };

  const execution = await adapter.start(input);
  const events: AgentRuntimeEvent[] = [];
  const unsubscribe = await adapter.subscribe(execution.executionId, event => events.push(event));
  const deadline = Date.now() + options.timeoutMs;
  try {
    while (Date.now() < deadline && !events.some(event => TERMINAL.has(event.eventType))) {
      await new Promise(resolveSleep => setTimeout(resolveSleep, 100));
    }
    const terminalStatus = await adapter.getStatus(execution.executionId).catch(() => "unknown");
    return { events, terminalStatus };
  } finally {
    await unsubscribe().catch(() => undefined);
    await adapter.terminate(execution.executionId).catch(() => undefined);
  }
}

async function main(): Promise<void> {
  const reportPath = resolve(process.env.HERMES_AGENT_SECURITY_REPORT_PATH ?? "reports/phase0-agent-security.json");
  const baseUrl = process.env.HERMES_HTTP_BASE_URL;
  const confirmation = process.env.HERMES_SECURITY_LAB_CONFIRM;
  let results: TestResult[];

  if (confirmation !== "I_UNDERSTAND_THE_RISK") {
    results = blockedAll(
      "Refusing to run agent security probes. Set HERMES_SECURITY_LAB_CONFIRM=I_UNDERSTAND_THE_RISK only in " +
        "a disposable environment with no production credentials."
    );
  } else if (!baseUrl) {
    results = blockedAll("HERMES_HTTP_BASE_URL is not configured, so no agent can be probed");
  } else {
    const client = new HermesHttpClient({
      baseUrl,
      ...(process.env.HERMES_API_KEY ? { apiKey: process.env.HERMES_API_KEY } : {}),
      timeoutMs: positiveNumber("HERMES_TIMEOUT_MS", 30_000)
    });
    const adapter = new HermesAdapter(client);
    const workspace = process.env.HERMES_SECURITY_WORKSPACE ?? "fixtures/security-lab/workspace";
    const timeoutMs = positiveNumber("HERMES_AGENT_SECURITY_TIMEOUT_MS", 120_000);
    results = await executeAgentSecurityPhase0(AGENT_PROBES, probe =>
      runProbeWithAdapter(adapter, probe, {
        workspace,
        timeoutMs,
        correlationId: `phase0-security-${probe.id}`
      })
    );
  }

  await writeTestReport(reportPath, results);
  console.log(`Phase 0 agent security report: ${reportPath}`);
  for (const item of results) console.log(`${item.id} ${item.status}: ${item.title}`);
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

function blockedAll(note: string): TestResult[] {
  const timestamp = new Date().toISOString();
  return AGENT_PROBES.map(probe => ({
    id: probe.id,
    title: probe.title,
    status: "blocked" as const,
    startedAt: timestamp,
    finishedAt: timestamp,
    evidence: [],
    notes: note
  }));
}

function positiveNumber(name: string, fallback: number): number {
  const value = Number(process.env[name] ?? fallback);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`${name} must be a positive number`);
  return value;
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
