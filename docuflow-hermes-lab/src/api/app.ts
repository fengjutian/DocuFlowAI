import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { AgentAdapterError } from "../types/adapter.js";
import type { AgentRuntimeRegistry } from "../adapters/runtime-registry.js";
import type { JournalStore } from "../journal/journal.js";
import type { UsageLedger } from "../accounting/usage.js";
import type { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
import type { TaskExecutionService, StartTaskRequest } from "../tasks/task-execution-service.js";
import { SkillDeniedError } from "../tasks/task-execution-service.js";
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

  if (method === "GET" && matches(segments, ["api", "v1", "executions", ":id", "approvals"])) {
    const id = segments[3]!;
    const pending = deps.coordinator.getAdapter().getPendingPermissions?.(id) ?? [];
    return { status: 200, body: { approvals: pending } };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "approvals", ":requestId"])) {
    const executionId = segments[3]!;
    const requestId = segments[5]!;
    const decision = readDecision(body);
    await deps.coordinator.getAdapter().respondToPermission?.(executionId, requestId, decision);
    return { status: 200, body: { executionId, requestId, decision: decision.decision } };
  }

  if (method === "POST" && matches(segments, ["api", "v1", "executions", ":id", "cancel"])) {
    const id = segments[3]!;
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
    deps.skills.registry.register(body as never);
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
  let status: string | undefined;
  try {
    status = await deps.coordinator.getAdapter().getStatus(executionId);
  } catch {
    status = undefined;
  }
  return {
    executionId,
    status: status ?? (checkpoint.terminal ? checkpoint.terminalEventType : "unknown"),
    checkpoint,
    pendingApprovals: deps.coordinator.getAdapter().getPendingPermissions?.(executionId) ?? [],
    // AT-204 surfaced to the caller: "completed" is a claim that needs a passing evaluation.
    completion: await deps.taskService.completionGate(executionId)
  };
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
    "Content-Type": "application/json",
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
    ...(parseRequestedSkills(record) ?? {})
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

function readDecision(body: unknown): { decision: "approve" | "deny"; reason?: string } {
  const record = (body ?? {}) as Record<string, unknown>;
  const decision = record["decision"];
  if (decision !== "approve" && decision !== "deny") {
    throw new ApiError(400, "invalid_decision", "decision must be 'approve' or 'deny'");
  }
  const reason = record["reason"];
  return { decision, ...(typeof reason === "string" && reason ? { reason } : {}) };
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
