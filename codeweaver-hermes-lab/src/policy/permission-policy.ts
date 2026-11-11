/**
 * Policy Engine permission mapping (design V1.0 section 8).
 *
 * The table here is the executable form of 8.1. It exists so that a permission question is answered
 * by code and a policy snapshot, not by the prompt and not by the runtime's own approval UI
 * (decision D-003). Unknown operations are denied, never defaulted to allow.
 */
import { isAbsolute, relative, resolve } from "node:path";

export type PermissionLevel = "P0" | "P1" | "P2" | "P3";
export type PolicyDecision = "allow" | "deny" | "require_approval";

export const OPERATION_LEVELS = {
  read_file: "P0",
  search_code: "P0",
  query_gitnexus: "P0",
  generate_diff: "P1",
  modify_worktree: "P1",
  delete_file: "P2",
  install_dependency: "P2",
  modify_public_api: "P2",
  modify_database_schema: "P2",
  git_merge: "P3",
  deploy_production: "P3",
  access_production_data: "P3"
} as const satisfies Record<string, PermissionLevel>;

export type AgentOperation = keyof typeof OPERATION_LEVELS;

export interface PermissionRequest {
  /** Operation name; must exist in OPERATION_LEVELS or the evaluation denies it. */
  operation: string;
  /** Absolute paths the operation touches. Required for scoped P1 writes. */
  paths?: string[];
  /** Absolute path of the authorized worktree, used to bound P1 writes. */
  worktreePath?: string;
  tool?: string;
}

/** Mirrors the `policy_snapshots` table fields in design section 11.6. */
export interface PolicySnapshot {
  snapshotId: string;
  projectId: string;
  taskId: string;
  executionId: string;
  worktreeId: string;
  policyVersion: string;
  /** Operations granted to this execution; `*` grants everything the table allows. */
  grantedCapabilities: string[];
  /** Absolute path prefixes the execution may modify. */
  allowedPaths: string[];
  /** P2 operations pre-authorized by policy rather than by a human ("策略授权"). */
  autoApprovedOperations?: string[];
  /** ISO timestamp; an expired snapshot grants nothing. */
  expiresAt: string;
}

export interface PolicyEvaluation {
  decision: PolicyDecision;
  level: PermissionLevel | "unknown";
  requiresApproval: boolean;
  reason: string;
}

export function evaluatePermission(
  request: PermissionRequest,
  snapshot: PolicySnapshot,
  now: Date = new Date()
): PolicyEvaluation {
  const expiredAt = Date.parse(snapshot.expiresAt);
  if (Number.isNaN(expiredAt)) {
    return decision("deny", "unknown", "Policy snapshot has an invalid expires_at");
  }
  if (expiredAt <= now.getTime()) {
    return decision("deny", "unknown", `Policy snapshot ${snapshot.snapshotId} expired at ${snapshot.expiresAt}`);
  }

  const level = OPERATION_LEVELS[request.operation as AgentOperation] as PermissionLevel | undefined;
  if (!level) {
    return decision("deny", "unknown", `Unknown operation '${request.operation}' is denied by default`);
  }
  if (!isGranted(request.operation, snapshot)) {
    return decision("deny", level, `Operation '${request.operation}' is not granted by snapshot ${snapshot.snapshotId}`);
  }

  switch (level) {
    case "P0":
      return decision("allow", level, "Read-only project operation is allowed by default");

    case "P1":
      if (request.operation === "generate_diff") {
        return decision("allow", level, "Diff generation is allowed by default");
      }
      return evaluateWorktreeWrite(request, snapshot);

    case "P2":
      if (snapshot.autoApprovedOperations?.includes(request.operation)) {
        return decision("allow", level, `'${request.operation}' is pre-authorized by policy version ${snapshot.policyVersion}`);
      }
      return decision(
        "require_approval",
        level,
        `'${request.operation}' changes state and needs human approval`
      );

    case "P3":
      if (request.operation === "access_production_data") {
        return decision("deny", level, "Production data access is denied by default");
      }
      return decision("require_approval", level, `'${request.operation}' is a release-level action and needs human approval`);
  }
}

function evaluateWorktreeWrite(request: PermissionRequest, snapshot: PolicySnapshot): PolicyEvaluation {
  const paths = request.paths ?? [];
  if (paths.length === 0) {
    return decision("require_approval", "P1", "Worktree modification without explicit paths needs approval");
  }
  for (const path of paths) {
    if (!snapshot.allowedPaths.some(allowed => isWithin(path, allowed))) {
      return decision("deny", "P1", `Path '${path}' is outside every authorized path in the snapshot`);
    }
    if (request.worktreePath && !isWithin(path, request.worktreePath)) {
      return decision("deny", "P1", `Path '${path}' is outside worktree '${request.worktreePath}'`);
    }
  }
  return decision("allow", "P1", `All ${paths.length} path(s) are inside the authorized worktree scope`);
}

function isGranted(operation: string, snapshot: PolicySnapshot): boolean {
  return snapshot.grantedCapabilities.includes("*") || snapshot.grantedCapabilities.includes(operation);
}

function decision(
  policyDecision: PolicyDecision,
  level: PermissionLevel | "unknown",
  reason: string
): PolicyEvaluation {
  return {
    decision: policyDecision,
    level,
    requiresApproval: policyDecision === "require_approval",
    reason
  };
}

/** True when `child` is `parent` itself or strictly inside it, after resolving `.` and `..`. */
export function isWithin(child: string, parent: string): boolean {
  const from = resolve(parent);
  const to = resolve(child);
  const rel = relative(normalizeCase(from), normalizeCase(to));
  if (rel === "") return true;
  if (isAbsolute(rel)) return false;
  return !rel.split(/[\\/]/).includes("..");
}

function normalizeCase(path: string): string {
  return process.platform === "win32" ? path.toLowerCase() : path;
}
