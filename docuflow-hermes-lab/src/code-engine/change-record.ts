import { randomUUID } from "node:crypto";

/**
 * Immutable change records (design 11.9). A record is evidence: it is created once, frozen, and
 * never rewritten. A rollback creates a *new* record that points at the one it reverts, which is why
 * `operation` has `revert` rather than letting history be edited.
 */
export type ChangeOperation = "apply" | "revert" | "restore";

export interface ApprovalRecord {
  approvedBy: string;
  reason: string;
  approvedAt: string;
}

export interface ChangeRecord {
  id: string;
  projectId: string;
  taskId: string;
  executionId: string;
  operation: ChangeOperation;
  gitBaseSha: string;
  diffHash: string;
  gitCommitSha?: string;
  revertsChangeRecordId?: string;
  revertsCommitSha?: string;
  approval?: ApprovalRecord;
  createdAt: string;
}

export interface BuildApplyRecordInput {
  projectId: string;
  taskId: string;
  executionId: string;
  gitBaseSha: string;
  diffHash: string;
  gitCommitSha?: string;
  approval?: ApprovalRecord;
  createdAt?: string;
  idFactory?: () => string;
}

export interface BuildRevertRecordInput {
  /** The record whose change is being undone; must carry the commit that landed on the branch. */
  original: ChangeRecord;
  diffHash: string;
  gitCommitSha: string;
  approval: ApprovalRecord;
  createdAt?: string;
  idFactory?: () => string;
}

export function buildApplyRecord(input: BuildApplyRecordInput): ChangeRecord {
  requireText(input.projectId, "projectId");
  requireText(input.taskId, "taskId");
  requireText(input.executionId, "executionId");
  requireSha(input.gitBaseSha, "gitBaseSha");
  requireSha(input.diffHash, "diffHash");
  if (input.gitCommitSha !== undefined) requireSha(input.gitCommitSha, "gitCommitSha");

  const record: ChangeRecord = {
    id: (input.idFactory ?? (() => `chg_${randomUUID()}`))(),
    projectId: input.projectId,
    taskId: input.taskId,
    executionId: input.executionId,
    operation: "apply",
    gitBaseSha: input.gitBaseSha.toLowerCase(),
    diffHash: input.diffHash.toLowerCase(),
    ...(input.gitCommitSha ? { gitCommitSha: input.gitCommitSha.toLowerCase() } : {}),
    ...(input.approval ? { approval: Object.freeze({ ...input.approval }) } : {}),
    createdAt: input.createdAt ?? new Date().toISOString()
  };
  return Object.freeze(record);
}

export function buildRevertRecord(input: BuildRevertRecordInput): ChangeRecord {
  const original = input.original;
  if (original.operation !== "apply" && original.operation !== "restore") {
    throw new Error(`Cannot revert a record with operation '${original.operation}'`);
  }
  if (!original.gitCommitSha) {
    throw new Error("Cannot revert a change that never produced a commit");
  }
  requireSha(input.gitCommitSha, "gitCommitSha");
  requireSha(input.diffHash, "diffHash");
  requireText(input.approval.approvedBy, "approval.approvedBy");
  requireText(input.approval.reason, "approval.reason");

  const record: ChangeRecord = {
    id: (input.idFactory ?? (() => `chg_${randomUUID()}`))(),
    projectId: original.projectId,
    taskId: original.taskId,
    executionId: original.executionId,
    operation: "revert",
    gitBaseSha: original.gitCommitSha,
    diffHash: input.diffHash.toLowerCase(),
    gitCommitSha: input.gitCommitSha.toLowerCase(),
    revertsChangeRecordId: original.id,
    revertsCommitSha: original.gitCommitSha,
    approval: Object.freeze({ ...input.approval }),
    createdAt: input.createdAt ?? new Date().toISOString()
  };
  return Object.freeze(record);
}

function requireText(value: string, field: string): void {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`Change record field '${field}' must be a non-empty string`);
  }
}

function requireSha(value: string, field: string): void {
  if (!/^[0-9a-f]{7,128}$/i.test(value)) {
    throw new Error(`Change record field '${field}' must be a hexadecimal hash, received '${value}'`);
  }
}
