import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createApiServer } from "../api/app.js";
import type { EvaluationCheckSpec } from "../evaluation/evaluation.js";
import { createRuntime } from "../server/runtime.js";
import { collectToolFacts, type ToolJournalEntry } from "./tool-journal.js";

/**
 * Driving one real agent execution and reading back what it did.
 *
 * The agent arms of this lab all ask the same questions of an execution: did the agent really
 * call the tool, what did the tool answer, what did the agent say afterwards, and did the
 * completion gate stay open. The client is here so that those arms do not each grow their own
 * copy of the same HTTP calls, and so that every arm drives the product the way the console
 * drives it: over HTTP, against the real runtime, with a real model.
 */
export interface AgentArmClient {
  start(input: {
    taskId: string;
    projectId: string;
    workspace: string;
    prompt: string;
    checks: EvaluationCheckSpec[];
    timeoutSeconds: number;
  }): Promise<string>;
  logs(executionId: string): Promise<{ entries: ToolJournalEntry[] }>;
  execution(executionId: string): Promise<{
    status: string;
    completion: { allowed: boolean; reason: string };
  }>;
}

export interface AgentExecution {
  executionId: string;
  status: string;
  /** Everything the agent said. */
  answer: string;
  /** How often the agent called the tool under test. */
  toolCalls: number;
  /** What the tool answered, as the agent read it. */
  toolText: string;
  /** What the checks themselves printed. */
  checkOutputs: string[];
  /** The completion gate of the execution. */
  gate: { allowed: boolean; reason: string };
}

/** Runs one execution and reads back what it did. */
export async function runAgentExecution(
  client: AgentArmClient,
  options: {
    taskId: string;
    projectId: string;
    workspace: string;
    prompt: string;
    checks: EvaluationCheckSpec[];
    timeoutSeconds?: number;
    tool?: string;
  }
): Promise<AgentExecution> {
  const executionId = await client.start({
    taskId: options.taskId,
    projectId: options.projectId,
    workspace: options.workspace,
    prompt: options.prompt,
    checks: options.checks,
    timeoutSeconds: options.timeoutSeconds ?? 180
  });
  const status = await waitForTerminal(client, executionId, (options.timeoutSeconds ?? 180) * 1000);
  const { entries } = await client.logs(executionId);
  const facts = collectToolFacts(entries);
  const tool = options.tool ?? "evaluate_changes";
  const toolText = facts.results
    .filter(result => result.tool === tool)
    .map(result => result.text)
    .join("\n");
  const checkOutputs = facts.results
    .filter(result => result.tool === tool)
    .flatMap(result => result.checkOutputs ?? []);
  const execution = await client.execution(executionId);
  return {
    executionId,
    status,
    answer: facts.answer,
    toolCalls: facts.calls.filter(call => call.tool === tool).length,
    toolText,
    checkOutputs,
    gate: execution.completion
  };
}

export async function waitForTerminal(
  client: AgentArmClient,
  executionId: string,
  timeoutMs: number
): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const { entries } = await client.logs(executionId);
    const terminal = entries.find(
      entry =>
        entry.category === "execution.completed" ||
        entry.category === "execution.failed" ||
        entry.category === "execution.cancelled"
    );
    if (terminal) return String((terminal.payload ?? {})["status"] ?? terminal.category);
    await sleep(500);
  }
  return "timeout";
}

export function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Starts the product for one agent arm and closes it again.
 *
 * The repository is the workspace: the task service wants a worktree, and the acceptance check
 * has to live inside it, because that is where the task's declaration points.
 */
export async function withAgentRuntime<T>(options: {
  workspace: string;
  projectId: string;
  model?: string;
  run: (client: AgentArmClient) => Promise<T>;
}): Promise<T> {
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-agent-arm-"));
  const runtime = createRuntime({
    host: "127.0.0.1",
    port: 0,
    dataDir,
    worktreeRoot: join(dataDir, "worktrees"),
    repository: options.workspace,
    hermesBaseUrl: "http://127.0.0.1:9",
    hermesTimeoutMs: 5_000,
    pi: { provider: "minimax-cn", model: options.model ?? "MiniMax-M3" }
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);
  try {
    return await options.run(httpAgentClient(server.url));
  } finally {
    await server.close();
    await runtime.close();
    await rm(dataDir, { recursive: true, force: true });
  }
}

/** The real client: everything goes over HTTP, the way the console drives it. */
export function httpAgentClient(baseUrl: string): AgentArmClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: input.projectId,
          prompt: input.prompt,
          worktreePath: input.workspace,
          documents: [],
          selectedFiles: [],
          architectureVersion: "1.0",
          policy: {
            snapshotId: `snap-${input.projectId}`,
            projectId: input.projectId,
            taskId: input.taskId,
            executionId: "",
            worktreeId: `wt-${input.projectId}`,
            policyVersion: "1.0.0",
            grantedCapabilities: ["*"],
            allowedPaths: [input.workspace],
            expiresAt: "2099-01-01T00:00:00.000Z"
          },
          template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
          modelConfig: { model: process.env.PI_PROBE_MODEL ?? "MiniMax-M3" },
          correlationId: input.taskId,
          timeoutSeconds: input.timeoutSeconds ?? 180,
          allowedTools: ["evaluate_changes"],
          evaluationChecks: input.checks,
          runtimeId: "pi-local"
        })
      });
      if (response.status !== 201) {
        throw new Error(`the task was refused: ${response.status} ${await response.text()}`);
      }
      const created = (await response.json()) as { execution: { executionId: string } };
      return created.execution.executionId;
    },
    logs: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/logs`);
      if (response.status !== 200) {
        throw new Error(`the journal was refused: ${response.status} ${await response.text()}`);
      }
      return (await response.json()) as { count: number; entries: ToolJournalEntry[] };
    },
    execution: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}`);
      if (response.status !== 200) {
        throw new Error(`the execution was refused: ${response.status} ${await response.text()}`);
      }
      const body = (await response.json()) as {
        status?: string;
        completion?: { allowed?: boolean; reason?: string };
      };
      return {
        status: String(body.status ?? "unknown"),
        completion: {
          allowed: body.completion?.allowed === true,
          reason: String(body.completion?.reason ?? "")
        }
      };
    }
  };
}
