import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { AgentAdapterError } from "../types/adapter.js";
import type { AgentRuntimeRegistry } from "../adapters/runtime-registry.js";
import type { RuntimeScheduler } from "../adapters/runtime-scheduler.js";
import type { SubagentManager } from "../orchestrator/subagent-manager.js";
import type { JournalStore } from "../journal/journal.js";
import type { UsageLedger } from "../accounting/usage.js";
import type { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
import type { TaskExecutionService, StartTaskRequest } from "../tasks/task-execution-service.js";
import { SkillDeniedError } from "../tasks/task-execution-service.js";
import type { EvaluationCheckSpec } from "../evaluation/evaluation.js";
import type { AgentTaskInput, RuntimePermissionRequest } from "../types/adapter.js";
import type { SkillBindingStore } from "../skills/skill-bindings.js";
import type { SkillRegistry } from "../skills/skill-registry.js";
import { SkillConflictError, SkillNotFoundError } from "../skills/skill-registry.js";
import { SkillValidationError } from "../skills/skill.js";
import { EventBus, formatSseFrame, journalEntryToEvent } from "./event-bus.js";

/**
 * HTTP surface (design 12) kept intentionally thin: it authenticates, validates, delegates and maps
 * errors. Every decision lives in the service it calls.
 *
 * Transport note: live events are served as SSE over `GET /api/v1/events`. The envelope and replay
 * contract are the ones design 13 specifies for WebSocket; a WS transport can be added later without
 * changing clients that already understand the envelope.
 */
export interface ApiDependencies {
  registry: AgentRuntimeRegistry;
  coordinator: ExecutionCoordinator;
  taskService: TaskExecutionService;
  journal: JournalStore;
  bus: EventBus;
  /** Optional: without it the usage endpoint reports that accounting is not configured. */
  usage?: UsageLedger;
  /** Optional skill surface; absent means the server cannot register or bind skills. */
  skills?: { registry: SkillRegistry; bindings: SkillBindingStore };
  /** Optional multi-runtime selection; absent means only the `auto` runtime id is unavailable. */
  scheduler?: RuntimeScheduler;
  /** Optional subagent spawning; absent means the server cannot delegate to a child execution. */
  subagents?: SubagentManager;
  /** When set, every request must present `Authorization: Bearer <token>`. */
  token?: string;
  now?: () => Date;
}

export interface ApiServer {
  url: string;
  close(): Promise<void>;
}

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = "ApiError";
  }
}

const MAX_BODY_BYTES = 1_000_000;

/**
 * Origins allowed to call this server from a browser context.
 *
 * The desktop client (Tauri webview) and the Vite dev server run on a different origin than the API,
 * and a browser refuses such requests unless the server says so. The list is an allowlist, never a
 * wildcard: CORS is not authentication — the bearer token still is — but a loopback lab server should
 * not answer arbitrary pages that ask.
 */
const DEFAULT_ALLOWED_ORIGINS = [
  "http://localhost:1420",
  "http://127.0.0.1:1420",
  "http://localhost:1421",
  "http://tauri.localhost",
  "https://tauri.localhost"
];

function allowedOrigins(env: NodeJS.ProcessEnv = process.env): Set<string> {
  const configured = (env["CODEWEAVER_ALLOWED_ORIGINS"] ?? "")
    .split(",")
    .map(origin => origin.trim())
    .filter(origin => origin !== "");
  return new Set([...DEFAULT_ALLOWED_ORIGINS, ...configured]);
}

/** Applies CORS headers for allowlisted origins; returns true when the origin is allowed. */
function applyCors(request: IncomingMessage, response: ServerResponse): boolean {
  const origin = request.headers.origin;
  if (typeof origin !== "string" || !allowedOrigins().has(origin)) return false;
  response.setHeader("Access-Control-Allow-Origin", origin);
  response.setHeader("Vary", "Origin");
  response.setHeader("Access-Control-Allow-Methods", "GET, POST, PATCH, DELETE, OPTIONS");
  response.setHeader("Access-Control-Allow-Headers", "Authorization, Content-Type, Last-Event-ID");
  response.setHeader("Access-Control-Expose-Headers", "Content-Type");
  return true;
}

export function createApiServer(deps: ApiDependencies) {
  return {
    listen(port = 0, host = "127.0.0.1"): Promise<ApiServer> {
      const server = createServer((request, response) => {
        void handleRequest(request, response, deps);
      });
      return new Promise((resolveListen, rejectListen) => {
        server.on("error", rejectListen);
        server.listen(port, host, () => {
          const address = server.address();
          const actualPort = typeof address === "object" && address ? address.port : port;
          resolveListen({
            url: `http://${host}:${actualPort}`,
            close: () => new Promise<void>((done, fail) =>
              server.close(error => (error ? fail(error) : done()))
            )
          });
        });
      });
    }
  };
}

async function handleRequest(
  request: IncomingMessage,
  response: ServerResponse,
  deps: ApiDependencies
): Promise<void> {
  try {
    const url = new URL(request.url ?? "/", "http://localhost");
    const method = request.method ?? "GET";

    // CORS first: a preflight carries no Authorization header, so it cannot pass the auth check.
    const originAllowed = applyCors(request, response);
    if (method === "OPTIONS") {
      if (!originAllowed) {
        sendJson(response, 403, { error: "origin_not_allowed", message: "This origin may not call the API" });
        return;
      }
      response.writeHead(204);
      response.end();
      return;
    }

    if (url.pathname === "/api/v1/events") {
      if (!authorize(request, deps, response)) return;
      await serveEvents(request, response, deps, url);
      return;
    }

    if (!authorize(request, deps, response)) return;

    const body = method === "POST" ? await readJsonBody(request) : undefined;
    const result = await route(method, url, body, deps);
    sendJson(response, result.status, result.body);
  } catch (error) {
    const mapped = mapError(error);
    if (!response.headersSent) sendJson(response, mapped.status, { error: mapped.code, message: mapped.message });
    else response.end();
  }
}

interface RouteResult {
  status: number;
  body: unknown;
}

async function route(
  method: string,
  url: URL,
  body: unknown,
  deps: ApiDependencies
): Promise<RouteResult> {
  const segments = url.pathname.split("/").filter(Boolean);

  if (method === "GET" && matches(segments, ["api", "v1", "runtimes"])) {
    return {
      status: 200,
      body: {
        runtimes: deps.registry.list().map(entry => ({
          id: entry.id,
          runtimeType: entry.runtimeType,
          version: entry.version ?? null,
          metadata: entry.metadata ?? {}
        }))
      }
    };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "runtimes", ":id"])) {
    const id = segments[3]!;
    const entry = deps.registry.list().find(item => item.id === id);
    if (!entry) throw new ApiError(404, "runtime_not_found", `Runtime ${id} is not registered`);
    const health = await deps.registry.healthCheckAll();
    return {
      status: 200,
      body: {
        id: entry.id,
        runtimeType: entry.runtimeType,
        version: entry.version ?? null,
        metadata: entry.metadata ?? {},
        health: health.runtimes.find(item => item.id === id) ?? null
      }
    };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "runtimes", ":id", "health-check"])) {
    const id = segments[3]!;
    const report = await deps.registry.healthCheckAll();
    const health = report.runtimes.find(item => item.id === id);
    if (!health) throw new ApiError(404, "runtime_not_found", `Runtime ${id} is not registered`);
    return { status: health.healthy ? 200 : 503, body: health };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "tasks", ":taskId", "executions"])) {
    const taskId = segments[3]!;
    const request = toStartTaskRequest(taskId, body);
    const result = await deps.taskService.startTask(request);
    return {
      status: 201,
      body: {
        execution: result.execution,
        contextSnapshotId: result.contextSnapshot.id,
        policySnapshotId: result.policySnapshot.id,
        promptSnapshotId: result.promptSnapshot.id
      }
    };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id"])) {
    const id = segments[3]!;
    return { status: 200, body: await describeExecution(id, deps) };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id", "events"])) {
    const id = segments[3]!;
    const since = Number(url.searchParams.get("since") ?? 0);
    if (!Number.isFinite(since) || since < 0) throw new ApiError(400, "invalid_since", "since must be a non-negative number");
    const entries = (await deps.journal.read(id)).filter(entry => entry.sequence > since);
    return { status: 200, body: { events: entries.map(journalEntryToEvent) } };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id", "logs"])) {
    // The raw journal, not the event projection: an auditor needs the exact recorded lines.
    const id = segments[3]!;
    const entries = await deps.journal.read(id);
    return {
      status: 200,
      body: {
        executionId: id,
        count: entries.length,
        entries: entries.map(entry => ({
          sequence: entry.sequence,
          kind: entry.kind,
          recordedAt: entry.recordedAt,
          ...(entry.category ? { category: entry.category } : {}),
          ...(entry.payload !== undefined ? { payload: entry.payload } : {})
        }))
      }
    };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "changes"])) {
    const id = segments[3]!;
    const outcome = await deps.taskService.applyProposedChange(id, readPatch(body), readApplyOptions(body));
    return { status: outcome.status === "applied" ? 201 : 200, body: outcome };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "evaluation"])) {
    const id = segments[3]!;
    const result = await deps.taskService.evaluateChange(id, readEvaluationChecks(body));
    return { status: 200, body: result };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id", "trace"])) {
    const id = segments[3]!;
    const trace = await deps.taskService.trace(id);
    if (!trace) throw new ApiError(404, "trace_not_found", `No trace is recorded for execution ${id}`);
    return { status: 200, body: trace };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "subagents"])) {
    if (!deps.subagents) {
      throw new ApiError(501, "subagents_not_configured", "Subagent delegation is not wired into this server");
    }
    const parentExecutionId = segments[3]!;
    const child = await deps.subagents.spawn(readSpawnRequest(parentExecutionId, body));
    return {
      status: 201,
      body: {
        execution: child.execution,
        worktreeMode: child.worktreeMode,
        workingDirectory: child.workingDirectory,
        rootExecutionId: deps.subagents.rootOf(child.execution.executionId),
        lineage: deps.subagents.lineageOf(child.execution.executionId)
      }
    };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id", "subagents"])) {
    if (!deps.subagents) {
      throw new ApiError(501, "subagents_not_configured", "Subagent delegation is not wired into this server");
    }
    const parentExecutionId = segments[3]!;
    return {
      status: 200,
      body: {
        children: deps.subagents.childrenOf(parentExecutionId),
        lineage: deps.subagents.lineageOf(parentExecutionId),
        rootExecutionId: deps.subagents.rootOf(parentExecutionId)
      }
    };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "runtimes", "selection"])) {
    if (!deps.scheduler) {
      throw new ApiError(501, "scheduler_not_configured", "Runtime scheduling is not wired into this server");
    }
    const selection = await deps.scheduler.select({
      ...(url.searchParams.get("runtimeType")
        ? { runtimeType: url.searchParams.get("runtimeType") as never }
        : {}),
      skipHealthCheck: url.searchParams.get("skipHealthCheck") === "true"
    });
    return {
      status: 200,
      body: {
        runtimeId: selection.runtimeId,
        runtimeType: selection.registration.runtimeType,
        version: selection.registration.version ?? null
      }
    };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id", "approvals"])) {
    const id = segments[3]!;
    // The adapter that owns this execution, not "the" adapter: with several runtimes registered,
    // only the owning one can have pending permission requests.
    const pending = deps.coordinator.adapterFor(id).getPendingPermissions?.(id) ?? [];
    return { status: 200, body: { approvals: pending } };
  }

  // Design 12.3: an approval is addressed by its own request id, and the decision is the path.
  if (
    method === "POST" &&
    (matches(segments, ["api", "v1", "approvals", ":requestId", "approve"]) ||
      matches(segments, ["api", "v1", "approvals", ":requestId", "reject"]))
  ) {
    const requestId = segments[3]!;
    const decision = segments[4] === "approve" ? "approve" : "deny";
    const reason = readReason(body);
    const executionId = await deps.coordinator.findByPermissionRequest(requestId);
    if (!executionId) {
      throw new ApiError(404, "approval_not_found", `No pending approval request ${requestId}`);
    }
    await answerApproval(deps, executionId, requestId, decision, reason);
    return { status: 200, body: { executionId, requestId, decision } };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "approvals", ":requestId"])) {
    const executionId = segments[3]!;
    const requestId = segments[5]!;
    const decision = readDecision(body);
    await answerApproval(deps, executionId, requestId, decision.decision, decision.reason);
    return { status: 200, body: { executionId, requestId, decision: decision.decision } };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "cancel"])) {
    const id = segments[3]!;
    // Cancelling an execution this process no longer holds would fail deep inside the adapter with
    // "Unknown execution". Saying so up front keeps the failure legible.
    if (!deps.coordinator.isLive(id)) {
      throw new ApiError(409, "execution_not_live", `Execution ${id} is not running in this server process`);
    }
    await deps.coordinator.cancel(id);
    return { status: 202, body: { executionId: id, status: "cancelling" } };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "retry"])) {
    const id = segments[3]!;
    const result = await deps.coordinator.recover(id);
    if (result.plan.action !== "rebuild") {
      throw new ApiError(409, "retry_not_applicable", `Recovery decided '${result.plan.action}': ${result.plan.reason}`);
    }
    return { status: 202, body: { previousExecutionId: id, execution: result.execution, plan: result.plan } };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "resume"])) {
    const id = segments[3]!;
    const result = await deps.coordinator.recover(id);
    if (result.plan.action !== "resume") {
      throw new ApiError(
        501,
        "resume_unsupported",
        `Runtime cannot resume this execution (${result.plan.action}): ${result.plan.reason}`
      );
    }
    return { status: 200, body: { execution: result.execution, plan: result.plan } };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id", "usage"])) {
    const id = segments[3]!;
    if (!deps.usage) {
      throw new ApiError(501, "usage_not_configured", "Usage accounting is not wired into this server");
    }
    const record = deps.usage.forExecution(id);
    if (!record) throw new ApiError(404, "usage_not_found", `No usage recorded for execution ${id}`);
    return { status: 200, body: { usage: record } };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "usage"])) {
    if (!deps.usage) {
      throw new ApiError(501, "usage_not_configured", "Usage accounting is not wired into this server");
    }
    return { status: 200, body: { summary: deps.usage.summary(), records: deps.usage.list() } };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "skills"])) {
    if (!deps.skills) throw new ApiError(501, "skills_not_configured", "Skills are not wired into this server");
    return {
      status: 200,
      body: {
        skills: deps.skills.registry.list().map(manifest => ({
          ...manifest,
          deprecated: deps.skills!.registry.isDeprecated(manifest.skillId, manifest.version)
        }))
      }
    };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "skills"])) {
    if (!deps.skills) throw new ApiError(501, "skills_not_configured", "Skills are not wired into this server");
    if (!body || typeof body !== "object" || Array.isArray(body)) {
      throw new ApiError(400, "invalid_body", "A JSON object body is required");
    }
    // The manifest is validated and written to disk before the API answers: a skill the server
    // accepted but cannot read back is a skill it would silently drop on the next restart.
    await deps.skills.registry.register(body as never);
    return { status: 201, body: { registered: true } };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id", "skills"])) {
    const id = segments[3]!;
    if (!deps.skills) throw new ApiError(501, "skills_not_configured", "Skills are not wired into this server");
    return { status: 200, body: { bindings: deps.skills.bindings.forExecution(id) } };
  }

  if (method === "GET" && matches(segments, ["api", "v1", "trace", "commits", ":sha"])) {
    const sha = segments[4]!;
    const trace = await deps.taskService.traceCommit(sha);
    if (!trace) throw new ApiError(404, "commit_not_found", `No change record for commit ${sha}`);
    return { status: 200, body: trace };
  }

  throw new ApiError(404, "not_found", `No route for ${method} ${url.pathname}`);
}

async function describeExecution(executionId: string, deps: ApiDependencies): Promise<unknown> {
  const checkpoint = await deps.coordinator.getCheckpoint(executionId);
  // A real execution always journals at least `execution.started`; an empty journal means unknown.
  if (checkpoint.eventCount === 0) {
    throw new ApiError(404, "execution_not_found", `No execution ${executionId} is recorded`);
  }
  // A restart empties the coordinator's live sessions but not the journal, so an execution can be
  // recorded and fully readable while nothing about it runs in this process any more. The adapter
  // then knows nothing about it, and asking it anyway would fail the whole request — or, worse,
  // answer about the server's default runtime instead of the one that ran the task.
  const live = deps.coordinator.isLive(executionId);
  const adapter = deps.coordinator.adapterFor(executionId);
  let status: string | undefined;
  let pendingApprovals: RuntimePermissionRequest[] = [];
  if (live) {
    try {
      status = await adapter.getStatus(executionId);
    } catch {
      status = undefined;
    }
    try {
      pendingApprovals = adapter.getPendingPermissions?.(executionId) ?? [];
    } catch {
      pendingApprovals = [];
    }
  }
  // Which runtime ran this execution. The adapter is the fact while the session is live; after a
  // restart the journaled runtime id is the fact, because the registry would only name the
  // server's default runtime and that would be a different runtime than the one that ran it.
  const journaledRuntimeId = await journaledRuntimeIdFor(executionId, deps);
  const runtimeId =
    journaledRuntimeId ??
    (live ? deps.registry.list().find(entry => entry.adapter === adapter)?.id : undefined);
  return {
    executionId,
    runtimeId,
    // Whether this server process still holds the session. The console uses it to stop offering
    // "cancel" for an execution that ended in an earlier process.
    live,
    status: status ?? statusFromTerminalEvent(checkpoint.terminalEventType) ?? "unknown",
    checkpoint,
    pendingApprovals,
    // AT-204 surfaced to the caller: "completed" is a claim that needs a passing evaluation.
    completion: await deps.taskService.completionGate(executionId)
  };
}

/** The runtime id journaled when the execution started, if the execution was started at all. */
async function journaledRuntimeIdFor(executionId: string, deps: ApiDependencies): Promise<string | undefined> {
  const entries = await deps.journal.read(executionId);
  const fact = entries.find(entry => entry.category === "execution.runtime");
  const runtimeId = fact?.payload["runtimeId"];
  return typeof runtimeId === "string" && runtimeId !== "" ? runtimeId : undefined;
}

/**
 * The status a terminal event implies.
 *
 * The journal names the terminal outcome (`execution.cancelled`), while the console and the API
 * speak in statuses (`cancelled`). Without this translation a recorded execution would be reported
 * with an event name where a status belongs.
 */
function statusFromTerminalEvent(terminalEventType: string | undefined): string | undefined {
  switch (terminalEventType) {
    case "execution.completed":
      return "succeeded";
    case "execution.failed":
      return "failed";
    case "execution.cancelled":
      return "cancelled";
    default:
      return undefined;
  }
}

async function serveEvents(
  request: IncomingMessage,
  response: ServerResponse,
  deps: ApiDependencies,
  url: URL
): Promise<void> {
  response.writeHead(200, {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache",
    Connection: "keep-alive"
  });
  response.write("retry: 3000\n\n");

  const since = resolveSince(url, request);
  const executionId = url.searchParams.get("executionId");
  if (executionId) {
    const entries = (await deps.journal.read(executionId)).filter(entry => entry.sequence > since);
    for (const entry of entries) response.write(formatSseFrame(journalEntryToEvent(entry)));
  }

  const unsubscribe = deps.bus.subscribe(event => {
    if (executionId && event.executionId !== executionId) return;
    response.write(formatSseFrame(event));
  });
  const heartbeat = setInterval(() => response.write(": heartbeat\n\n"), 15_000);

  const cleanup = () => {
    clearInterval(heartbeat);
    unsubscribe();
  };
  request.on("close", cleanup);
  response.on("close", cleanup);
}

function resolveSince(url: URL, request: IncomingMessage): number {
  const querySince = Number(url.searchParams.get("since") ?? 0);
  if (Number.isFinite(querySince) && querySince > 0) return querySince;
  const lastEventId = request.headers["last-event-id"];
  if (typeof lastEventId === "string") {
    const sequence = Number(lastEventId.split(":").pop());
    if (Number.isFinite(sequence)) return sequence;
  }
  return 0;
}

function matches(segments: string[], pattern: string[]): boolean {
  if (segments.length !== pattern.length) return false;
  return pattern.every((expected, index) => expected.startsWith(":") || expected === segments[index]);
}

function authorize(request: IncomingMessage, deps: ApiDependencies, response: ServerResponse): boolean {
  if (!deps.token) return true;
  const header = request.headers.authorization;
  if (header === `Bearer ${deps.token}`) return true;
  sendJson(response, 401, { error: "unauthorized", message: "A valid bearer token is required" });
  return false;
}

function sendJson(response: ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  response.writeHead(status, {
    // The charset is explicit on purpose: browsers assume UTF-8 for JSON, but stricter clients
    // (PowerShell 5.1 among them) fall back to Latin-1 and turn every Chinese character into `?`.
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": Buffer.byteLength(payload)
  });
  response.end(payload);
}

function readJsonBody(request: IncomingMessage): Promise<unknown> {
  return new Promise((resolveBody, rejectBody) => {
    let size = 0;
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => {
      size += chunk.length;
      if (size > MAX_BODY_BYTES) {
        rejectBody(new ApiError(413, "payload_too_large", "Request body exceeds 1MB"));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on("end", () => {
      if (chunks.length === 0) return resolveBody(undefined);
      try {
        resolveBody(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      } catch {
        rejectBody(new ApiError(400, "invalid_json", "Request body is not valid JSON"));
      }
    });
    request.on("error", rejectBody);
  });
}

function toStartTaskRequest(taskId: string, body: unknown): StartTaskRequest {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    throw new ApiError(400, "invalid_body", "A JSON object body is required");
  }
  const record = body as Record<string, unknown>;
  const requireString = (key: string): string => {
    const value = record[key];
    if (typeof value !== "string" || value.trim() === "") {
      throw new ApiError(400, "invalid_body", `Field '${key}' must be a non-empty string`);
    }
    return value;
  };
  const requireArray = (key: string): string[] => {
    const value = record[key];
    if (!Array.isArray(value) || !value.every(item => typeof item === "string")) {
      throw new ApiError(400, "invalid_body", `Field '${key}' must be an array of strings`);
    }
    return value;
  };
  const policy = record["policy"];
  if (!policy || typeof policy !== "object") {
    throw new ApiError(400, "invalid_body", "Field 'policy' must be a policy snapshot object");
  }
  const template = record["template"];
  if (!template || typeof template !== "object") {
    throw new ApiError(400, "invalid_body", "Field 'template' must be an object");
  }

  return {
    projectId: requireString("projectId"),
    taskId,
    prompt: requireString("prompt"),
    worktreePath: requireString("worktreePath"),
    documents: requireArray("documents"),
    selectedFiles: requireArray("selectedFiles"),
    architectureVersion: requireString("architectureVersion"),
    policy: policy as StartTaskRequest["policy"],
    template: template as StartTaskRequest["template"],
    modelConfig: (record["modelConfig"] as Record<string, unknown> | undefined) ?? {},
    correlationId: record["correlationId"] === undefined ? taskId : requireString("correlationId"),
    timeoutSeconds: typeof record["timeoutSeconds"] === "number" ? record["timeoutSeconds"] : 60,
    allowedTools: Array.isArray(record["allowedTools"])
      ? (record["allowedTools"] as unknown[]).every(item => typeof item === "string")
        ? (record["allowedTools"] as string[])
        : []
      : [],
    ...(parseRequestedSkills(record) ?? {}),
    ...(record["evaluationChecks"] === undefined
      ? {}
      : { evaluationChecks: parseEvaluationChecks(record["evaluationChecks"], "evaluationChecks") }),
    ...(typeof record["runtimeId"] === "string" && record["runtimeId"].trim() !== ""
      ? { runtimeId: record["runtimeId"] }
      : {})
  };
}

function parseRequestedSkills(record: Record<string, unknown>): Pick<StartTaskRequest, "skills" | "approvedSkills"> | undefined {
  const raw = record["skills"];
  if (raw === undefined) return undefined;
  if (!Array.isArray(raw)) throw new ApiError(400, "invalid_body", "Field 'skills' must be an array");

  const skills = raw.map(item => {
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw new ApiError(400, "invalid_body", "Each skill must be an object with a skillId");
    }
    const skill = item as Record<string, unknown>;
    if (typeof skill["skillId"] !== "string" || skill["skillId"].trim() === "") {
      throw new ApiError(400, "invalid_body", "Each skill needs a non-empty skillId");
    }
    return {
      skillId: skill["skillId"],
      ...(typeof skill["version"] === "string" ? { version: skill["version"] } : {})
    };
  });

  const approvedRaw = record["approvedSkills"];
  if (approvedRaw !== undefined && (!Array.isArray(approvedRaw) || !approvedRaw.every(item => typeof item === "string"))) {
    throw new ApiError(400, "invalid_body", "Field 'approvedSkills' must be an array of strings");
  }

  return {
    skills,
    ...(Array.isArray(approvedRaw) ? { approvedSkills: approvedRaw as string[] } : {})
  };
}

/** A child execution request. The parent decides the lineage; the caller supplies the work. */
function readSpawnRequest(parentExecutionId: string, body: unknown): {
  parentExecutionId: string;
  taskInput: AgentTaskInput;
  worktree?: "isolated" | "shared";
  baseSha?: string;
  label?: string;
} {
  const record = (body ?? {}) as Record<string, unknown>;
  const requireString = (key: string): string => {
    const value = record[key];
    if (typeof value !== "string" || value.trim() === "") {
      throw new ApiError(400, "invalid_body", `Field '${key}' must be a non-empty string`);
    }
    return value;
  };
  const worktree = record["worktree"];
  if (worktree !== undefined && worktree !== "isolated" && worktree !== "shared") {
    throw new ApiError(400, "invalid_body", "Field 'worktree' must be 'isolated' or 'shared'");
  }
  const baseSha = record["baseSha"];
  const label = record["label"];
  const prompt = requireString("prompt");

  return {
    parentExecutionId,
    taskInput: {
      projectId: requireString("projectId"),
      taskId: requireString("taskId"),
      runId: requireString("runId"),
      prompt,
      contextSnapshotId: requireString("contextSnapshotId"),
      policySnapshotId: requireString("policySnapshotId"),
      worktreeId: requireString("worktreeId"),
      workingDirectory: typeof record["workingDirectory"] === "string" ? record["workingDirectory"] : "",
      allowedTools: Array.isArray(record["allowedTools"])
        ? (record["allowedTools"] as unknown[]).filter((item): item is string => typeof item === "string")
        : [],
      timeoutSeconds: typeof record["timeoutSeconds"] === "number" ? record["timeoutSeconds"] : 60,
      correlationId: requireString("correlationId")
    },
    ...(worktree ? { worktree } : {}),
    ...(typeof baseSha === "string" && baseSha ? { baseSha } : {}),
    ...(typeof label === "string" && label ? { label } : {})
  };
}

/** The patch an agent proposes. It is the only thing the Code Engine accepts as input. */
function readPatch(body: unknown): string {
  const record = (body ?? {}) as Record<string, unknown>;
  const patch = record["patch"];
  if (typeof patch !== "string" || patch.trim() === "") {
    throw new ApiError(400, "invalid_body", "Field 'patch' must be a non-empty diff");
  }
  return patch;
}

function readApplyOptions(body: unknown): {
  commitMessage?: string;
  approval?: { approvedBy: string; reason: string; approvedAt: string };
  allowedPaths?: string[];
} {
  const record = (body ?? {}) as Record<string, unknown>;
  const options: {
    commitMessage?: string;
    approval?: { approvedBy: string; reason: string; approvedAt: string };
    allowedPaths?: string[];
  } = {};
  if (typeof record["commitMessage"] === "string" && record["commitMessage"].trim() !== "") {
    options.commitMessage = record["commitMessage"];
  }
  if (record["approval"] !== undefined) {
    const approval = record["approval"];
    if (!approval || typeof approval !== "object" || Array.isArray(approval)) {
      throw new ApiError(400, "invalid_body", "Field 'approval' must be an object");
    }
    const value = approval as Record<string, unknown>;
    // An approval is evidence of a human decision, so all three fields are required. A missing
    // approver would leave the change record naming nobody.
    const approvedBy = value["approvedBy"];
    if (typeof approvedBy !== "string" || approvedBy.trim() === "") {
      throw new ApiError(400, "invalid_body", "approval.approvedBy must be a non-empty string");
    }
    const reason = value["reason"];
    if (typeof reason !== "string" || reason.trim() === "") {
      throw new ApiError(400, "invalid_body", "approval.reason must be a non-empty string");
    }
    const approvedAt = value["approvedAt"];
    if (typeof approvedAt !== "string" || approvedAt.trim() === "") {
      throw new ApiError(400, "invalid_body", "approval.approvedAt must be an ISO timestamp");
    }
    options.approval = { approvedBy, reason, approvedAt };
  }
  const paths = record["allowedPaths"];
  if (paths !== undefined) {
    if (!Array.isArray(paths) || !paths.every(item => typeof item === "string")) {
      throw new ApiError(400, "invalid_body", "Field 'allowedPaths' must be an array of strings");
    }
    options.allowedPaths = paths as string[];
  }
  return options;
}

/** Evaluation checks are independent commands, not the agent's own claim (AT-204). */
function readEvaluationChecks(body: unknown): EvaluationCheckSpec[] {
  const record = (body ?? {}) as Record<string, unknown>;
  return parseEvaluationChecks(record["checks"], "checks");
}

function parseEvaluationChecks(raw: unknown, field: string): EvaluationCheckSpec[] {
  if (!Array.isArray(raw)) {
    throw new ApiError(400, "invalid_body", `Field '${field}' must be an array of evaluation checks`);
  }
  return raw.map((item, index) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw new ApiError(400, "invalid_body", `${field}[${index}] must be an object`);
    }
    const check = item as Record<string, unknown>;
    const command = check["command"];
    if (typeof command !== "string" || command.trim() === "") {
      throw new ApiError(400, "invalid_body", `${field}[${index}].command must be a non-empty string`);
    }
    const args = check["args"];
    if (args !== undefined && (!Array.isArray(args) || !args.every(arg => typeof arg === "string"))) {
      throw new ApiError(400, "invalid_body", `${field}[${index}].args must be an array of strings`);
    }
    return {
      id: typeof check["id"] === "string" ? check["id"] : `check-${index + 1}`,
      name: typeof check["name"] === "string" ? check["name"] : `check-${index + 1}`,
      command,
      args: Array.isArray(args) ? (args as string[]) : [],
      ...(typeof check["cwd"] === "string" ? { cwd: check["cwd"] } : {}),
      ...(typeof check["timeoutMs"] === "number" ? { timeoutMs: check["timeoutMs"] } : {}),
      ...(typeof check["expectExitCode"] === "number" ? { expectExitCode: check["expectExitCode"] } : {})
    };
  });
}

function readDecision(body: unknown): { decision: "approve" | "deny"; reason?: string } {
  const record = (body ?? {}) as Record<string, unknown>;
  const decision = record["decision"];
  if (decision !== "approve" && decision !== "deny") {
    throw new ApiError(400, "invalid_decision", "decision must be 'approve' or 'deny'");
  }
  const reason = record["reason"];
  return { decision, ...(typeof reason === "string" && reason ? { reason } : {}) };
}

/** The optional reason a human gives with an approval decision. */
function readReason(body: unknown): string | undefined {
  const record = (body ?? {}) as Record<string, unknown>;
  const reason = record["reason"];
  return typeof reason === "string" && reason.trim() !== "" ? reason : undefined;
}

/**
 * Sends an approval decision to the runtime that raised it.
 *
 * The answer always goes to the owning adapter, and a runtime without an approval surface is
 * reported as such rather than silently swallowing the decision.
 */
async function answerApproval(
  deps: ApiDependencies,
  executionId: string,
  requestId: string,
  decision: "approve" | "deny",
  reason?: string
): Promise<void> {
  const adapter = deps.coordinator.adapterFor(executionId);
  if (!adapter.respondToPermission) {
    throw new ApiError(
      501,
      "approvals_not_supported",
      `Runtime '${adapter.constructor.name}' cannot answer approval requests`
    );
  }
  await adapter.respondToPermission(executionId, requestId, {
    decision,
    ...(reason ? { reason } : {})
  });
}

function mapError(error: unknown): { status: number; code: string; message: string } {
  if (error instanceof ApiError) {
    return { status: error.status, code: error.code, message: error.message };
  }
  if (error instanceof SkillDeniedError) {
    return { status: 403, code: "skill_denied", message: `${error.message}: ${error.reasons.join("; ")}` };
  }
  if (error instanceof SkillValidationError) {
    return { status: 400, code: "invalid_skill", message: error.message };
  }
  if (error instanceof SkillConflictError) {
    return { status: 409, code: "skill_conflict", message: error.message };
  }
  if (error instanceof SkillNotFoundError) {
    return { status: 404, code: "skill_not_found", message: error.message };
  }
  if (error instanceof AgentAdapterError) {
    const status = {
      execution_not_found: 404,
      unsupported_capability: 501,
      invalid_state: 409,
      runtime_unreachable: 502,
      protocol_error: 502
    }[error.code];
    return { status, code: error.code, message: error.message };
  }
  return { status: 500, code: "internal_error", message: error instanceof Error ? error.message : "Unexpected error" };
}
