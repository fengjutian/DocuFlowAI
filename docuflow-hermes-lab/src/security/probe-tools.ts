import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { evaluatePermission, type PolicySnapshot } from "../policy/permission-policy.js";
import { isPathAuthorized, type DocuFlowToolContext } from "../tools/docuflow-tools.js";
import type { RuntimePermissionRequest, RuntimePermissionResponse } from "../types/adapter.js";

/**
 * The tool surface the agent-level security probes run against.
 *
 * These tools are deliberately capable and unguarded: the read really reads, the write really
 * writes, the fetch really fetches. The boundary under test is DocuFlow's policy gate, which the
 * Pi adapter runs in its `beforeToolCall` hook — before the tool executes, not after the fact. A
 * tool that policed itself would only prove that the tool is careful.
 *
 * The consequence is deliberate: if the gate fails, the probe really does write outside the
 * workspace or really does reach the network. That is why the probes only ever run against a
 * disposable lab.
 */
export interface ProbeTool {
  name: string;
  label: string;
  description: string;
  parameters: Record<string, unknown>;
  execute: (
    toolCallId: string,
    args: Record<string, unknown>,
    signal?: AbortSignal,
    onUpdate?: (update: unknown) => void
  ) => Promise<ProbeToolResult>;
}

export interface ProbeToolResult {
  content: Array<{ type: "text"; text: string }>;
  details: Record<string, unknown>;
}

/** The tools a probe agent is given. */
export const PROBE_TOOL_NAMES = ["read_file", "write_file", "fetch_url"] as const;

export function createProbeTools(): ProbeTool[] {
  return [
    {
      name: "read_file",
      label: "Read file",
      description: "Read a file from the filesystem and return its content as text.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string", description: "Absolute path of the file to read." }
        },
        required: ["path"]
      },
      execute: async (_toolCallId, args) => {
        const path = text(args, "path");
        try {
          const content = await readFile(path, "utf8");
          return textResult(content, { path, bytes: content.length });
        } catch (error) {
          return textResult(`read_file failed: ${message(error)}`, { path, error: true });
        }
      }
    },
    {
      name: "write_file",
      label: "Write file",
      description: "Create or overwrite a file with the given text content.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string", description: "Absolute path of the file to write." },
          content: { type: "string", description: "The text to write into the file." }
        },
        required: ["path", "content"]
      },
      execute: async (_toolCallId, args) => {
        const path = text(args, "path");
        const content = text(args, "content");
        try {
          await mkdir(dirname(path), { recursive: true });
          await writeFile(path, content, "utf8");
          return textResult(`Wrote ${content.length} byte(s) to ${path}`, { path, bytes: content.length });
        } catch (error) {
          return textResult(`write_file failed: ${message(error)}`, { path, error: true });
        }
      }
    },
    {
      name: "fetch_url",
      label: "Fetch URL",
      description: "Fetch an HTTP URL and return the status line and the first part of the body.",
      parameters: {
        type: "object",
        properties: {
          url: { type: "string", description: "The absolute http(s) URL to fetch." }
        },
        required: ["url"]
      },
      execute: async (_toolCallId, args, signal) => {
        const url = text(args, "url");
        try {
          const timeout = AbortSignal.timeout(10_000);
          const combined = signal ? AbortSignal.any([signal, timeout]) : timeout;
          const response = await fetch(url, { signal: combined });
          const body = (await response.text()).slice(0, 500);
          return textResult(`HTTP ${response.status} ${response.statusText}\n${body}`, {
            url,
            status: response.status
          });
        } catch (error) {
          return textResult(`fetch_url failed: ${message(error)}`, { url, error: true });
        }
      }
    }
  ];
}

export interface ProbePolicyOptions {
  /** The only directory the agent may touch. */
  workspacePath: string;
  /** The policy snapshot the probe execution runs under. */
  snapshot: PolicySnapshot;
  now?: () => Date;
}

/**
 * The policy gate the probes run against.
 *
 * The decisions come from the product's policy engine; this handler only maps a tool call onto an
 * operation and composes the reason. Two rules do the work:
 *
 * - a read is bounded by the worktree and the snapshot's allowed paths, the same rule the product's
 *   own `read_file` tool applies (`isPathAuthorized`);
 * - a write is a `modify_worktree` operation, which the policy engine denies when the path leaves
 *   the authorized scope;
 * - network access has no entry in the policy table at all, and an unknown operation is denied by
 *   default. That default is the property under test: egress is not something an agent can talk
 *   itself into.
 */
export function createProbePermissionHandler(
  options: ProbePolicyOptions
): (request: RuntimePermissionRequest) => Promise<RuntimePermissionResponse> {
  const now = options.now ?? (() => new Date());
  const context: DocuFlowToolContext = {
    worktreePath: options.workspacePath,
    policy: () => options.snapshot,
    // The probe never reaches the write or evaluation path: those calls are gated before they run.
    propose: () => {
      throw new Error("the probe does not propose diffs");
    },
    apply: async () => {
      throw new Error("the probe does not apply diffs");
    },
    evaluationChecks: () => [],
    evaluate: async () => {
      throw new Error("the probe does not evaluate changes");
    }
  };

  return async request => {
    const tool = request.tool ?? "";
    const args = asArgs(request.payload["args"]);

    if (tool === "read_file") {
      const target = resolve(options.workspacePath, text(args, "path"));
      if (!(await isPathAuthorized(context, target))) {
        return deny(`permission denied: path '${target}' is outside the authorized scope`);
      }
      const evaluation = evaluatePermission(
        { operation: "read_file", paths: [target], worktreePath: options.workspacePath },
        options.snapshot,
        now()
      );
      return evaluation.decision === "allow"
        ? approve()
        : deny(`permission denied: ${evaluation.reason}`);
    }

    if (tool === "write_file") {
      const target = resolve(options.workspacePath, text(args, "path"));
      const evaluation = evaluatePermission(
        { operation: "modify_worktree", paths: [target], worktreePath: options.workspacePath },
        options.snapshot,
        now()
      );
      return evaluation.decision === "allow"
        ? approve()
        : deny(`permission denied: ${evaluation.reason}`);
    }

    if (tool === "fetch_url") {
      const evaluation = evaluatePermission(
        { operation: "access_network" },
        options.snapshot,
        now()
      );
      return evaluation.decision === "allow"
        ? approve()
        : deny(`permission denied: ${evaluation.reason}`);
    }

    return deny(`permission denied: tool '${tool}' is not authorized by the policy snapshot`);
  };
}

function approve(): RuntimePermissionResponse {
  return { decision: "approve" };
}

function deny(reason: string): RuntimePermissionResponse {
  return { decision: "deny", reason };
}

function textResult(text: string, details: Record<string, unknown>): ProbeToolResult {
  return { content: [{ type: "text", text }], details };
}

function asArgs(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
}

function text(args: Record<string, unknown>, key: string): string {
  const value = args[key];
  return typeof value === "string" ? value : "";
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
