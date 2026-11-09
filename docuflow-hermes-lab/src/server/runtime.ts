import { resolve } from "node:path";
import { HermesAdapter } from "../adapters/hermes-adapter.js";
import { AgentRuntimeRegistry } from "../adapters/runtime-registry.js";
import { createApiServer, type ApiDependencies } from "../api/app.js";
import { EventBus } from "../api/event-bus.js";
import { UsageLedger } from "../accounting/usage.js";
import { HermesHttpClient } from "../clients/http/hermes-http-client.js";
import { CodeChangePipeline } from "../code-engine/code-change-pipeline.js";
import { SpawnGitRunner } from "../code-engine/git-runner.js";
import { WorktreeManager } from "../code-engine/worktree-manager.js";
import { JsonlJournalStore } from "../journal/journal.js";
import { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
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
import type { ChangeRecord } from "../code-engine/change-record.js";
import type { EvaluationResult } from "../evaluation/evaluation.js";
import type { AgentRuntimeEvent } from "../types/adapter.js";

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
}

export function loadConfigFromEnv(env: NodeJS.ProcessEnv = process.env): RuntimeConfig {
  const dataDir = resolve(env["DOCUFLOW_DATA_DIR"] ?? ".docuflow-data");
  const hermesBaseUrl = env["HERMES_HTTP_BASE_URL"]?.trim();
  if (!hermesBaseUrl) {
    throw new Error("HERMES_HTTP_BASE_URL is required to start the lab server");
  }
  const port = Number(env["DOCUFLOW_API_PORT"] ?? 8787);
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    throw new Error("DOCUFLOW_API_PORT must be an integer between 0 and 65535");
  }
  const timeout = Number(env["HERMES_TIMEOUT_MS"] ?? 30_000);
  if (!Number.isFinite(timeout) || timeout <= 0) {
    throw new Error("HERMES_TIMEOUT_MS must be a positive number");
  }
  const token = env["DOCUFLOW_API_TOKEN"]?.trim();

  return {
    host: env["DOCUFLOW_API_HOST"] ?? "127.0.0.1",
    port,
    ...(token ? { token } : {}),
    dataDir,
    worktreeRoot: resolve(env["DOCUFLOW_WORKTREE_ROOT"] ?? `${dataDir}/worktrees`),
    repository: resolve(env["DOCUFLOW_REPOSITORY"] ?? process.cwd()),
    hermesBaseUrl,
    ...(env["HERMES_API_KEY"] ? { hermesApiKey: env["HERMES_API_KEY"] } : {}),
    hermesTimeoutMs: timeout
  };
}

export interface Runtime {
  deps: ApiDependencies;
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
  const usage = new UsageLedger();
  const startedAt = new Map<string, string>();

  const TERMINAL = new Set(["execution.completed", "execution.failed", "execution.cancelled"]);
  const recordUsage = (event: AgentRuntimeEvent): void => {
    if (event.eventType === "execution.started") {
      if (!startedAt.has(event.executionId)) startedAt.set(event.executionId, event.timestamp);
      return;
    }
    if (!TERMINAL.has(event.eventType)) return;
    const began = startedAt.get(event.executionId);
    const durationMs = began ? Math.max(0, Date.parse(event.timestamp) - Date.parse(began)) : 0;
    usage.recordFromEvent(event, {
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
      recordUsage(event);
    }
  });

  const git = new SpawnGitRunner();
  const worktrees = new WorktreeManager(git, config.worktreeRoot, { repository: config.repository });
  const changes = new CodeChangePipeline(git, worktrees);
  const traceability = new TraceabilityQuery(stores);
  const taskService = new TaskExecutionService({ git, coordinator, changes, stores, traceability });

  return {
    deps: {
      registry,
      coordinator,
      taskService,
      journal,
      bus,
      usage,
      ...(config.token ? { token: config.token } : {})
    },
    close: () => coordinator.stopAll()
  };
}

export { createApiServer };
