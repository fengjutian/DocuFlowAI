import type { ChangeRecord } from "../code-engine/change-record.js";
import type { PolicySnapshot } from "../policy/permission-policy.js";

/**
 * Persisted entities mirroring design 11.6–11.9.
 *
 * Two rules apply to every record here:
 * 1. Immutable: `put` inserts, it never updates. A correction is a new record with a new id.
 * 2. No plaintext secrets: prompt snapshots keep hashes, never the rendered prompt itself.
 */

export interface ContextSnapshotRecord {
  id: string;
  projectId: string;
  taskId: string;
  executionId?: string;
  gitBaseSha: string;
  documentVersions: Record<string, string>;
  architectureVersion: string;
  gitnexusIndexVersion?: string;
  selectedFiles: string[];
  contextHash: string;
  createdAt: string;
}

export interface PromptSnapshotRecord {
  id: string;
  projectId: string;
  taskId: string;
  executionId: string;
  templateName: string;
  templateVersion: string;
  templateContentHash: string;
  /** Hash of the rendered prompt; the prompt text stays encrypted elsewhere. */
  renderedPromptHash: string;
  contextSnapshotId: string;
  modelConfig: Record<string, unknown>;
  createdAt: string;
}

export interface PolicySnapshotRecord {
  id: string;
  projectId: string;
  taskId: string;
  executionId: string;
  policyVersion: string;
  grantedCapabilities: string[];
  deniedCapabilities: string[];
  allowedPaths: string[];
  expiresAt: string;
  createdAt: string;
}

export type { ChangeRecord, PolicySnapshot };

/** Converts an evaluator snapshot into the persisted shape, keeping the same id and fields. */
export function toPolicySnapshotRecord(
  snapshot: PolicySnapshot,
  options: { createdAt?: string; deniedCapabilities?: string[] } = {}
): PolicySnapshotRecord {
  return {
    id: snapshot.snapshotId,
    projectId: snapshot.projectId,
    taskId: snapshot.taskId,
    executionId: snapshot.executionId,
    policyVersion: snapshot.policyVersion,
    grantedCapabilities: [...snapshot.grantedCapabilities],
    deniedCapabilities: [...(options.deniedCapabilities ?? [])],
    allowedPaths: [...snapshot.allowedPaths],
    expiresAt: snapshot.expiresAt,
    createdAt: options.createdAt ?? new Date().toISOString()
  };
}

export function assertContextSnapshot(record: ContextSnapshotRecord): void {
  requireText(record.id, "id");
  requireText(record.projectId, "projectId");
  requireText(record.taskId, "taskId");
  requireSha(record.gitBaseSha, "gitBaseSha");
  requireSha(record.contextHash, "contextHash");
  if (!Array.isArray(record.selectedFiles)) throw new Error("selectedFiles must be an array");
}

export function assertPromptSnapshot(record: PromptSnapshotRecord): void {
  requireText(record.id, "id");
  requireText(record.projectId, "projectId");
  requireText(record.taskId, "taskId");
  requireText(record.executionId, "executionId");
  requireText(record.templateName, "templateName");
  requireText(record.templateVersion, "templateVersion");
  requireSha(record.templateContentHash, "templateContentHash");
  requireSha(record.renderedPromptHash, "renderedPromptHash");
  requireText(record.contextSnapshotId, "contextSnapshotId");
  if ("renderedPrompt" in record) {
    throw new Error("Prompt snapshots must not store the rendered prompt in plaintext");
  }
}

export function assertPolicySnapshot(record: PolicySnapshotRecord): void {
  requireText(record.id, "id");
  requireText(record.projectId, "projectId");
  requireText(record.executionId, "executionId");
  requireText(record.policyVersion, "policyVersion");
  if (Number.isNaN(Date.parse(record.expiresAt))) {
    throw new Error("expiresAt must be a timestamp");
  }
}

function requireText(value: string, field: string): void {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`Record field '${field}' must be a non-empty string`);
  }
}

function requireSha(value: string, field: string): void {
  if (!/^[0-9a-f]{7,128}$/i.test(value)) {
    throw new Error(`Record field '${field}' must be a hexadecimal hash, received '${value}'`);
  }
}
