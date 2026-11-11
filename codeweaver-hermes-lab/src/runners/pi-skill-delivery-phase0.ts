import { spawnSync } from "node:child_process";
import { cp, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import { skillContentChecksum, type SkillManifest } from "../skills/skill.js";
import type { JournalStore } from "../journal/journal.js";
import type { TestResult } from "../types/hermes.js";

/**
 * Skill delivery: does the skill body actually reach the model?
 *
 * The registry, the authorization and the binding all worked long before this probe existed. What
 * was missing is the part that matters: the body never reached the runtime, so the model never saw
 * the instructions the user wrote. A skill that is registered, authorized and bound but never
 * delivered is a permission declaration with nothing to say.
 *
 * So the probe runs the same task twice on the real model:
 *
 * - the control run, without the skill: the answer must NOT contain the token;
 * - the instructed run, with the skill: the answer MUST contain the token.
 *
 * The token exists only inside the skill body, so the control run cannot produce it by accident.
 * One run alone would not be evidence: the model might have produced the token by luck, or might
 * have produced nothing at all. The pair is what "the skill reached the model" means.
 */

/** The token only the skill body knows. */
const TOKEN = "CODEWEAVER_SKILL_OK";

/** The task both runs are given. It never names the token. */
const TASK_PROMPT = "Please greet the user.";

/** The skill body: an instruction the model can only follow if it has read it. */
const SKILL_CONTENT = [
  "# Greeting protocol",
  "",
  "When the user asks you to greet them, your entire reply must be exactly this text and",
  "nothing else:",
  "",
  TOKEN,
  ""
].join("\n");

const SKILL_ID = "greeting-protocol";

export interface SkillDeliveryEvent {
  type: string;
  data?: Record<string, unknown>;
}

/** The slice of the system this probe drives; the real one talks to a server over the wire. */
export interface PiSkillDeliveryClient {
  /** Registers a skill manifest; returns the id the server stored it under. */
  registerSkill(manifest: SkillManifest): Promise<string>;
  /** Starts an execution; returns its execution id. */
  start(options: { prompt: string; skills?: string[] }): Promise<string>;
  /** The assistant's own output of an execution. */
  output(executionId: string): Promise<string>;
  /** Waits for a terminal state and returns it. */
  waitForTerminal(executionId: string, timeoutMs: number): Promise<string>;
  /** A journaled audit fact of an execution. */
  fact(executionId: string, category: string): Promise<Record<string, unknown> | undefined>;
  /** The hash of the prompt snapshot of an execution. */
  promptHash(executionId: string): Promise<string | undefined>;
}

export interface PiSkillDeliveryOptions {
  /** How long to wait for a run to finish. */
  terminalTimeoutMs?: number;
  now?: () => Date;
}

const TERMINAL_STATUSES = new Set(["succeeded", "failed", "cancelled", "interrupted"]);

export async function executePiSkillDeliveryPhase0(
  client: PiSkillDeliveryClient,
  options: PiSkillDeliveryOptions = {}
): Promise<TestResult[]> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const terminalTimeoutMs = options.terminalTimeoutMs ?? 180_000;
  const evidence: string[] = [];
  const failures: string[] = [];

  const registered = await client.registerSkill({
    skillId: SKILL_ID,
    version: "1.0.0",
    name: "Greeting protocol",
    description: "Answers a greeting with the fixed protocol token.",
    kind: "engineering",
    permissions: [],
    // The runtime the task actually runs on. A skill that names `pi-local` and is delivered on
    // `pi-local` also proves that the authorization used the task's runtime rather than a fixed
    // one: the old wiring would have refused it.
    allowedRuntimes: ["pi-local"],
    riskLevel: "low",
    approvalPolicy: "auto",
    content: SKILL_CONTENT,
    checksum: skillContentChecksum(SKILL_CONTENT),
    createdAt: now().toISOString()
  });
  evidence.push(`registeredSkill=${registered}`);

  // The control run: same task, no skill. The token is not in the prompt, so it cannot appear.
  const controlId = await client.start({ prompt: TASK_PROMPT });
  evidence.push(`controlExecutionId=${controlId}`);
  const controlStatus = await client.waitForTerminal(controlId, terminalTimeoutMs);
  evidence.push(`controlStatus=${controlStatus}`);
  const controlOutput = await client.output(controlId);
  evidence.push(`controlOutputHasToken=${controlOutput.includes(TOKEN)}`);

  // The instructed run: the same task, with the skill bound.
  const instructedId = await client.start({ prompt: TASK_PROMPT, skills: [SKILL_ID] });
  evidence.push(`instructedExecutionId=${instructedId}`);
  const instructedStatus = await client.waitForTerminal(instructedId, terminalTimeoutMs);
  evidence.push(`instructedStatus=${instructedStatus}`);
  const instructedOutput = await client.output(instructedId);
  evidence.push(`instructedOutputHasToken=${instructedOutput.includes(TOKEN)}`);

  // A run that never produced anything is an infrastructure problem, not a result.
  if (controlOutput.trim() === "" || instructedOutput.trim() === "") {
    return [
      {
        id: "SKILL-DELIVERY",
        title: "Skill body reaches the model",
        status: "blocked",
        startedAt,
        finishedAt: now().toISOString(),
        evidence,
        notes: "The model produced no output, so there was nothing to compare"
      }
    ];
  }

  if (!TERMINAL_STATUSES.has(controlStatus) || !TERMINAL_STATUSES.has(instructedStatus)) {
    failures.push(`a run did not reach a terminal state (control=${controlStatus}, instructed=${instructedStatus})`);
  }
  if (controlOutput.includes(TOKEN)) {
    failures.push("the control run produced the token without the skill; the probe cannot attribute it");
  }
  if (!instructedOutput.includes(TOKEN)) {
    failures.push("the instructed run did not produce the token, so the skill body did not reach the model");
  }

  // The delivery has to be recorded, not just observed in the output.
  const delivered = await client.fact(instructedId, "skill.delivered");
  evidence.push(`deliveryMode=${String(delivered?.["deliveryMode"] ?? "none")}`);
  evidence.push(`deliveredSkill=${String(delivered?.["skillId"] ?? "none")}`);
  if (!delivered) {
    failures.push("the instructed run recorded no skill.delivered fact");
  } else if (delivered["deliveryMode"] !== "prompt_inline") {
    failures.push(`the delivery mode was '${String(delivered["deliveryMode"])}', expected 'prompt_inline'`);
  }

  // The prompt the runtime received must be the composed one, not the bare task.
  const controlHash = await client.promptHash(controlId);
  const instructedHash = await client.promptHash(instructedId);
  evidence.push(`promptHashDiffers=${controlHash !== instructedHash}`);
  if (!controlHash || !instructedHash) {
    failures.push("a run has no prompt snapshot hash, so the delivered prompt cannot be compared");
  } else if (controlHash === instructedHash) {
    failures.push("the instructed run was started with the same prompt as the control run");
  }

  return [
    {
      id: "SKILL-DELIVERY",
      title: "Skill body reaches the model",
      status: failures.length === 0 ? "passed" : "failed",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
    }
  ];
}

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.PI_SKILL_DELIVERY_REPORT_PATH ?? "reports/phase0-pi-skill-delivery.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-skill-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-skill-data-"));
  git(["init", "-b", "main"], repo);
  git(["config", "user.name", "CodeWeaver Probe"], repo);
  git(["config", "user.email", "probe@codeweaver.local"], repo);
  await writeFile(join(repo, "README.md"), "# Skill delivery probe\n", "utf8");
  git(["add", "-A"], repo);
  git(["commit", "-m", "initial"], repo);

  const runtime = await createRuntime({
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
  const url = server.url;

  try {
    const client = createClient(url, runtime.deps.journal as JournalStore, repo, model);
    const results = await executePiSkillDeliveryPhase0(client);
    await writeTestReport(reportPath, results);
    for (const result of results) {
      console.log(`${result.id}: ${result.status}`);
      for (const line of result.evidence) console.log(`  ${line}`);
      if (result.notes) console.log(`  notes: ${result.notes}`);
    }
    console.log(`Report: ${reportPath}`);
    if (results.some(result => result.status === "failed")) process.exitCode = 1;
  } finally {
    await server.close();
    await runtime.close();
    await rm(repo, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
  }
}

function createClient(url: string, journal: JournalStore, repo: string, model: string): PiSkillDeliveryClient {
  const readEvents = async (executionId: string): Promise<SkillDeliveryEvent[]> => {
    const response = await fetch(`${url}/api/v1/executions/${executionId}/events`);
    const body = (await response.json()) as { events?: SkillDeliveryEvent[] };
    return body.events ?? [];
  };

  const startBody = (prompt: string, skills?: string[]) => ({
    projectId: "probe-pi-skill",
    prompt,
    worktreePath: repo,
    documents: ["README.md"],
    selectedFiles: ["README.md"],
    architectureVersion: "1.0",
    policy: {
      snapshotId: "snap-skill",
      projectId: "probe-pi-skill",
      taskId: "task-pi-skill",
      executionId: "",
      worktreeId: "wt-skill",
      policyVersion: "1.0.0",
      grantedCapabilities: ["*"],
      allowedPaths: [repo],
      expiresAt: "2099-01-01T00:00:00.000Z"
    },
    template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
    modelConfig: { model },
    correlationId: "probe-pi-skill",
    timeoutSeconds: 600,
    allowedTools: [],
    runtimeId: "pi-local",
    ...(skills ? { skills: skills.map(skillId => ({ skillId })) } : {})
  });

  return {
    registerSkill: async manifest => {
      const response = await fetch(`${url}/api/v1/skills`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(manifest)
      });
      if (response.status !== 201) {
        throw new Error(`the skill was refused: ${response.status} ${await response.text()}`);
      }
      return `${manifest.skillId}@${manifest.version}`;
    },
    start: async options => {
      const response = await fetch(`${url}/api/v1/tasks/task-pi-skill/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(startBody(options.prompt, options.skills))
      });
      if (response.status !== 201) {
        throw new Error(`the task was refused: ${response.status} ${await response.text()}`);
      }
      const created = (await response.json()) as { execution: { executionId: string } };
      return created.execution.executionId;
    },
    output: async executionId => {
      const events = await readEvents(executionId);
      return events
        .filter(event => event.type === "agent.message.delta")
        .map(event => String(event.data?.["delta"] ?? ""))
        .join("");
    },
    waitForTerminal: async (executionId, timeoutMs) => {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        const response = await fetch(`${url}/api/v1/executions/${executionId}`);
        const body = (await response.json()) as { status?: string };
        const status = String(body.status ?? "unknown");
        if (TERMINAL_STATUSES.has(status)) return status;
        await sleep(250);
      }
      return "timeout";
    },
    fact: async (executionId, category) => {
      const entries = await journal.read(executionId);
      return entries.find(entry => entry.category === category)?.payload;
    },
    promptHash: async executionId => {
      // The prompt snapshot is an immutable record, so the hash of the prompt that was actually
      // delivered comes from the trace the API serves.
      const response = await fetch(`${url}/api/v1/executions/${executionId}/trace`);
      if (response.status !== 200) return undefined;
      const body = (await response.json()) as {
        promptSnapshots?: Array<{ renderedPromptHash?: string }>;
      };
      return body.promptSnapshots?.[0]?.renderedPromptHash;
    }
  };
}

function git(args: string[], cwd: string): void {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr || result.stdout}`);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise(done => setTimeout(done, ms));
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) await main();
