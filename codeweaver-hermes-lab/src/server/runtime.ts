import { resolve } from "node:path";
import { randomUUID } from "node:crypto";
import { HermesAdapter } from "../adapters/hermes-adapter.js";
import { AgentRuntimeRegistry } from "../adapters/runtime-registry.js";
import { RuntimeScheduler } from "../adapters/runtime-scheduler.js";
import { createApiServer, type ApiDependencies } from "../api/app.js";
import { EventBus } from "../api/event-bus.js";
import { UsageLedger, type UsageRecord } from "../accounting/usage.js";
import { HermesHttpClient } from "../clients/http/hermes-http-client.js";
import { CodeChangePipeline } from "../code-engine/code-change-pipeline.js";
import { SpawnGitRunner } from "../code-engine/git-runner.js";
import { WorktreeManager } from "../code-engine/worktree-manager.js";
import { JsonlJournalStore, type JournalStore } from "../journal/journal.js";
import { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
import { SubagentManager } from "../orchestrator/subagent-manager.js";
import { ImmutableRecordStore } from "../persistence/record-store.js";
import {
  assertContextSnapshot,
  assertPolicySnapshot,
  assertPromptSnapshot,
  type ContextSnapshotRecord,
  type PolicySnapshotRecord,
  type PromptSnapshotRecord
} from "../persistence/records.js";
import { TraceabilityQuery } from "../persistence/traceability.js";
import { TaskExecutionService } from "../tasks/task-execution-service.js";
import { createPiRuntime } from "./pi-runtime.js";
import { CODEWEAVER_TOOL_NAMES } from "../tools/codeweaver-tools.js";
import { CodeWeaverToolBinder } from "../tools/execution-tool-binding.js";
import { createPolicyPermissionHandler } from "../tools/runtime-permission-handler.js";
import { SkillRegistry, type SkillManifestRecord } from "../skills/skill-registry.js";
import { SkillBindingStore, type SkillBindingEvent } from "../skills/skill-bindings.js";
import type { ChangeRecord } from "../code-engine/change-record.js";
import type { EvaluationResult } from "../evaluation/evaluation.js";
import type { AgentRuntimeEvent, AgentTaskInput } from "../types/adapter.js";

/**
 * Composition root.
 *
 * Everything here is wiring and configuration validation; behavior stays in the modules. The server
 * boots even when Hermes is unreachable, because "runtime is down" is a state the API must be able
 * to report (health check 503) rather than a reason to refuse to start.
 */
export interface RuntimeConfig {
  host: string;
  port: number;
  token?: string;
  dataDir: string;
  worktreeRoot: string;
  repository: string;
  hermesBaseUrl: string;
  hermesApiKey?: string;
  hermesTimeoutMs: number;
  /**
   * Pi runtime configuration. When present, a second runtime is registered next to Hermes and a
   * task may name it with `runtimeId`. Absent means this server runs Hermes only.
   */
  pi?: { provider: string; model: string };
}

export function loadConfigFromEnv(env: NodeJS.ProcessEnv = process.env): RuntimeConfig {
  const dataDir = resolve(env["CODEWEAVER_DATA_DIR"] ?? ".codeweaver-data");
  const hermesBaseUrl = env["HERMES_HTTP_BASE_URL"]?.trim();
  if (!hermesBaseUrl) {
    throw new Error("HERMES_HTTP_BASE_URL is required to start the lab server");
  }
  const port = Number(env["CODEWEAVER_API_PORT"] ?? 8787);
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    throw new Error("CODEWEAVER_API_PORT must be an integer between 0 and 65535");
  }
  const timeout = Number(env["HERMES_TIMEOUT_MS"] ?? 30_000);
  if (!Number.isFinite(timeout) || timeout <= 0) {
    throw new Error("HERMES_TIMEOUT_MS must be a positive number");
  }
  const token = env["CODEWEAVER_API_TOKEN"]?.trim();
  const piModel = env["PI_MODEL"]?.trim();

  return {
    host: env["CODEWEAVER_API_HOST"] ?? "127.0.0.1",
    port,
    ...(token ? { token } : {}),
    dataDir,
    worktreeRoot: resolve(env["CODEWEAVER_WORKTREE_ROOT"] ?? `${dataDir}/worktrees`),
    repository: resolve(env["CODEWEAVER_REPOSITORY"] ?? process.cwd()),
    hermesBaseUrl,
    ...(env["HERMES_API_KEY"] ? { hermesApiKey: env["HERMES_API_KEY"] } : {}),
    hermesTimeoutMs: timeout,
    // A Pi runtime needs a model. The provider defaults to the one this lab has evidence for.
    ...(piModel ? { pi: { provider: env["PI_PROVIDER"]?.trim() ?? "minimax-cn", model: piModel } } : {})
  };
}

export interface Runtime {
  deps: ApiDependencies;
  /** Loads durable indexes (usage) before serving; safe to call once at startup. */
  ready(): Promise<void>;
  close(): Promise<void>;
}

export function createRuntime(config: RuntimeConfig): Runtime {
  const stores = {
    contextSnapshots: new ImmutableRecordStore<ContextSnapshotRecord>(
      config.dataDir, "contexts", assertContextSnapshot
    ),
    policySnapshots: new ImmutableRecordStore<PolicySnapshotRecord>(
      config.dataDir, "policies", assertPolicySnapshot
    ),
    promptSnapshots: new ImmutableRecordStore<PromptSnapshotRecord>(
      config.dataDir, "prompts", assertPromptSnapshot
    ),
    changeRecords: new ImmutableRecordStore<ChangeRecord>(config.dataDir, "changes"),
    evaluations: new ImmutableRecordStore<EvaluationResult>(config.dataDir, "evaluations")
  };
  const journal = new JsonlJournalStore(resolve(config.dataDir, "journal"));
  const bus = new EventBus();
  const usage = new UsageLedger(
    new ImmutableRecordStore<UsageRecord>(config.dataDir, "usage")
  );
  const startedAt = new Map<string, string>();

  const TERMINAL = new Set(["execution.completed", "execution.failed", "execution.cancelled"]);
  const recordUsage = async (event: AgentRuntimeEvent): Promise<void> => {
    if (event.eventType === "execution.started") {
      if (!startedAt.has(event.executionId)) startedAt.set(event.executionId, event.timestamp);
      return;
    }
    if (!TERMINAL.has(event.eventType)) return;
    const began = startedAt.get(event.executionId);
    const durationMs = began ? Math.max(0, Date.parse(event.timestamp) - Date.parse(began)) : 0;
    await usage.recordFromEvent(event, {
      durationMs,
      ...(began ? { startedAt: began } : {}),
      finishedAt: event.timestamp
    });
  };

  const client = new HermesHttpClient({
    baseUrl: config.hermesBaseUrl,
    ...(config.hermesApiKey ? { apiKey: config.hermesApiKey } : {}),
    timeoutMs: config.hermesTimeoutMs
  });
  const adapter = new HermesAdapter(client);
  const registry = new AgentRuntimeRegistry();
  registry.register({ id: "hermes-local", runtimeType: "hermes", adapter, version: "pinned" });

  const coordinator = new ExecutionCoordinator({
    adapter,
    journal,
    onEvent: event => {
      bus.publishRuntimeEvent(event);
      void recordUsage(event).catch(() => undefined);
    },
    // After a restart the session is gone, but the journal still says which runtime ran it and
    // what was asked. Both are needed to recover: the runtime to recover on, and the input to
    // rebuild from. Without the runtime id the rebuild would silently land on the default one.
    recoveringAdapter: async executionId => {
      const runtimeId = await journaledRuntimeId(journal, executionId);
      return runtimeId ? registry.get(runtimeId) : undefined;
    },
    // The reverse lookup, so an execution can carry the name of the runtime that ran it.
    runtimeIdOf: adapter => registry.list().find(entry => entry.adapter === adapter)?.id,
    recoveryInput: async checkpoint => {
      const input = await journaledTaskInput(journal, checkpoint.executionId);
      return input;
    }
  });

  const git = new SpawnGitRunner();
  const worktrees = new WorktreeManager(git, config.worktreeRoot, { repository: config.repository });
  const changes = new CodeChangePipeline(git, worktrees);
  const traceability = new TraceabilityQuery(stores);
  // The registry keeps the manifest *and* its body, and it keeps them on disk: a skill library that
  // empties on every restart is a library the user cannot rely on.
  const skillRegistry = new SkillRegistry(
    new ImmutableRecordStore<SkillManifestRecord>(config.dataDir, "skills")
  );
  const skillBindings = new SkillBindingStore(
    new ImmutableRecordStore<SkillBindingEvent>(config.dataDir, "bindings")
  );
    // The scheduler reads the registry at call time, so a runtime registered later (Pi is created
    // during `ready()`) is still visible to it.
    const scheduler = new RuntimeScheduler(registry);
    const taskService = new TaskExecutionService({
      git,
      coordinator,
      changes,
      stores,
      traceability,
      skills: { registry: skillRegistry, bindings: skillBindings },
      // `auto` asks the scheduler to choose a healthy, capable runtime. Every other id names one
      // registered runtime exactly, so a task never lands on a runtime the caller did not pick.
      resolveRuntime: async runtimeId => {
        if (runtimeId === "auto") {
          const selection = await scheduler.select({ requiredCapabilities: { streaming: true } });
          return selection.adapter;
        }
        return registry.get(runtimeId);
      }
    });
  const subagents = new SubagentManager({ coordinator, worktrees });

  // The tool surface a runtime sees is bound to one execution: its worktree, its policy snapshot
  // and the acceptance checks its task declared. The binder is the only bridge from a runtime's
  // tool call to the Code Engine and the Evaluation Engine.
  const toolBinder = new CodeWeaverToolBinder({
    service: taskService,
    propose: patch => changes.propose(patch)
  });
  const permissionHandler = createPolicyPermissionHandler({
    policyFor: executionId => taskService.policyFor(executionId)
  });

  return {
    deps: {
      registry,
      coordinator,
      taskService,
      journal,
      bus,
      usage,
      scheduler,
      subagents,
      skills: { registry: skillRegistry, bindings: skillBindings },
      ...(config.token ? { token: config.token } : {})
    },
    ready: async () => {
      await usage.load();
      await skillBindings.load();
      await skillRegistry.load();
      // Pi is created here rather than in `createRuntime` because the Pi packages are ESM-only
      // and are loaded dynamically. A server without `PI_MODEL` never loads them at all.
      if (config.pi) {
        const { adapter, modelId } = await createPiRuntime({
          ...config.pi,
          // The tools are built per execution, so the worktree and the policy snapshot the tool
          // enforces are the ones this run actually has.
          tools: (input, executionId) => toolBinder.bind(input, executionId),
          // CodeWeaver's own tools are their own enforcement point; everything else the runtime
          // brings is decided by the policy engine.
          selfEnforcedTools: [...CODEWEAVER_TOOL_NAMES],
          permissionHandler
        });
        registry.register({
          id: "pi-local",
          runtimeType: "native",
          adapter,
          version: modelId,
          metadata: { provider: config.pi.provider }
        });
      }
    },
    close: () => coordinator.stopAll()
  };
}

export { createApiServer, journaledRuntimeId, journaledTaskInput };

/**
 * The runtime id journaled with an execution, if the execution recorded one.
 *
 * The journal is the only thing that survives a worker crash, so it is also the only place that
 * can still say which runtime ran a task.
 */
async function journaledRuntimeId(journal: JournalStore, executionId: string): Promise<string | undefined> {
  const entries = await journal.read(executionId);
  const fact = entries.find(entry => entry.category === "execution.runtime");
  const runtimeId = fact?.payload["runtimeId"];
  return typeof runtimeId === "string" && runtimeId !== "" ? runtimeId : undefined;
}

/**
 * The task as it was asked, recovered from the journal.
 *
 * A rebuild needs the original request; the checkpoint alone only says how far the last attempt
 * got. An execution that recorded no input cannot be rebuilt, and the caller is told so rather
 * than handed a task that is not the one it asked for.
 */
async function journaledTaskInput(
  journal: JournalStore,
  executionId: string
): Promise<AgentTaskInput | undefined> {
  const entries = await journal.read(executionId);
  const fact = entries.find(entry => entry.category === "execution.input");
  if (!fact) return undefined;
  const payload = fact.payload;
  const prompt = payload["prompt"];
  if (typeof prompt !== "string" || prompt === "") return undefined;
  const allowedTools = payload["allowedTools"];
  const featureId = payload["featureId"];
  return {
    projectId: readString(payload, "projectId"),
    ...(typeof featureId === "string" && featureId !== "" ? { featureId } : {}),
    taskId: readString(payload, "taskId"),
    // The rebuild is a new run of the same request, so it gets its own run id.
    runId: `run_${randomUUID()}`,
    prompt,
    contextSnapshotId: readString(payload, "contextSnapshotId"),
    policySnapshotId: readString(payload, "policySnapshotId"),
    worktreeId: readString(payload, "worktreeId"),
    workingDirectory: readString(payload, "workingDirectory"),
    allowedTools: Array.isArray(allowedTools) ? allowedTools.map(item => String(item)) : [],
    timeoutSeconds: Number(payload["timeoutSeconds"] ?? 60),
    correlationId: readString(payload, "correlationId")
  };
}

function readString(payload: Record<string, unknown>, key: string): string {
  const value = payload[key];
  return typeof value === "string" ? value : "";
}
