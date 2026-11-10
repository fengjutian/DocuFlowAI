import { evaluatePermission, type AgentOperation, type PolicySnapshot } from "../policy/permission-policy.js";
import type { RuntimePermissionRequest, RuntimePermissionResponse } from "../types/adapter.js";

/**
 * The DocuFlow answer to a tool call the runtime wants to make.
 *
 * DocuFlow's own tools decide for themselves (see `selfEnforcedTools` in the Pi adapter). This
 * handler covers everything else: a tool the runtime brought with it, or one DocuFlow does not
 * model. The decision is the policy engine's, never the runtime's own approval UI (D-003).
 *
 * A question the policy cannot answer on its own stays pending: the request is raised, a human
 * answers it through the API, and the answer reaches the runtime. Answering "approve" on the
 * runtime's behalf would make the human gate decorative (D-020).
 */

/** How a runtime tool name maps to the policy operation it performs. */
export const TOOL_OPERATIONS: Record<string, AgentOperation> = {
  read_file: "read_file",
  search_code: "search_code",
  query_gitnexus: "query_gitnexus",
  generate_diff: "generate_diff",
  modify_worktree: "modify_worktree",
  delete_file: "delete_file",
  install_dependency: "install_dependency",
  modify_public_api: "modify_public_api",
  modify_database_schema: "modify_database_schema",
  git_merge: "git_merge",
  deploy_production: "deploy_production",
  access_production_data: "access_production_data"
};

export interface RuntimePermissionHandlerDeps {
  /** The policy snapshot the execution runs under. */
  policyFor: (executionId: string) => Promise<PolicySnapshot>;
  /** The operation a tool performs; unknown tools have no operation and are denied. */
  operationFor?: (toolName: string) => AgentOperation | undefined;
  now?: () => Date;
}

export type PermissionHandler = (request: RuntimePermissionRequest) => Promise<RuntimePermissionResponse>;

export function createPolicyPermissionHandler(deps: RuntimePermissionHandlerDeps): PermissionHandler {
  const operationFor = deps.operationFor ?? ((tool: string) => TOOL_OPERATIONS[tool]);
  const now = deps.now ?? (() => new Date());

  return async (request: RuntimePermissionRequest): Promise<RuntimePermissionResponse> => {
    const toolName = request.tool ?? readToolFromPayload(request) ?? "unknown";
    const operation = operationFor(toolName);
    if (!operation) {
      return {
        decision: "deny",
        reason: `Tool '${toolName}' is not a DocuFlow operation and is denied by default`
      };
    }
    const policy = await deps.policyFor(request.executionId);
    const paths = readPaths(request);
    const worktreePath = readWorktree(request);
    const evaluation = evaluatePermission(
      {
        operation,
        ...(paths ? { paths } : {}),
        ...(worktreePath ? { worktreePath } : {})
      },
      policy,
      now()
    );
    if (evaluation.decision === "deny") {
      return { decision: "deny", reason: evaluation.reason };
    }
    if (evaluation.decision === "allow") {
      return { decision: "approve" };
    }
    // `require_approval`: the answer is not ours to give. The promise stays pending until a human
    // responds through the approval endpoint, which resolves the adapter's waiting decision.
    return new Promise<RuntimePermissionResponse>(() => undefined);
  };
}

function readToolFromPayload(request: RuntimePermissionRequest): string | undefined {
  const tool = request.payload["tool"];
  return typeof tool === "string" ? tool : undefined;
}

function readPaths(request: RuntimePermissionRequest): string[] | undefined {
  const paths = request.payload["paths"];
  if (!Array.isArray(paths)) return undefined;
  const list = paths.filter((item): item is string => typeof item === "string");
  return list.length > 0 ? list : undefined;
}

function readWorktree(request: RuntimePermissionRequest): string | undefined {
  const worktree = request.payload["worktreePath"];
  return typeof worktree === "string" ? worktree : undefined;
}
