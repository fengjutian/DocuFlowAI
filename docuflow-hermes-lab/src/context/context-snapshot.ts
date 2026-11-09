import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { isInside } from "../code-engine/worktree-manager.js";
import { gitOrThrow, type GitRunner } from "../code-engine/git-runner.js";
import type { ContextSnapshotRecord } from "../persistence/records.js";

export class ContextCaptureError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ContextCaptureError";
  }
}

export interface ContextCaptureInput {
  projectId: string;
  taskId: string;
  executionId?: string;
  /** Isolated worktree the context is captured from. */
  worktreePath: string;
  git: GitRunner;
  /** Authoritative documents (L1–L3 context) that must exist and be hashed. */
  documents: string[];
  /** Files the agent is expected to read; hashed into the context hash, not stored. */
  selectedFiles: string[];
  architectureVersion: string;
  gitnexusIndexVersion?: string;
  now?: () => Date;
  idFactory?: () => string;
}

/**
 * Captures the context snapshot from design 9.3.
 *
 * `context_hash` covers the git base SHA, the document hashes, the selected file contents and the
 * version fields, so two snapshots that look alike but were taken from different code are different
 * hashes. Nothing here trusts the caller's claim about file contents: every path is resolved inside
 * the worktree and re-read before hashing.
 */
export async function captureContextSnapshot(input: ContextCaptureInput): Promise<ContextSnapshotRecord> {
  const baseSha = (await gitOrThrow(input.git, ["rev-parse", "HEAD"], { cwd: input.worktreePath })).stdout.trim();

  const documentVersions: Record<string, string> = {};
  for (const document of [...new Set(input.documents)].sort()) {
    const path = safeResolve(input.worktreePath, document);
    documentVersions[document] = `sha256:${await hashFile(path)}`;
  }

  const fileHashes: Array<{ path: string; hash: string }> = [];
  for (const file of [...new Set(input.selectedFiles)].sort()) {
    const path = safeResolve(input.worktreePath, file);
    fileHashes.push({ path: file, hash: await hashFile(path) });
  }

  const contextHash = hashContext({
    projectId: input.projectId,
    taskId: input.taskId,
    gitBaseSha: baseSha,
    architectureVersion: input.architectureVersion,
    gitnexusIndexVersion: input.gitnexusIndexVersion ?? null,
    documentVersions,
    files: fileHashes
  });

  return {
    id: (input.idFactory ?? (() => `ctx_${createHash("sha256").update(`${baseSha}:${Date.now()}`).digest("hex").slice(0, 16)}`))(),
    projectId: input.projectId,
    taskId: input.taskId,
    ...(input.executionId ? { executionId: input.executionId } : {}),
    gitBaseSha: baseSha,
    documentVersions,
    architectureVersion: input.architectureVersion,
    ...(input.gitnexusIndexVersion ? { gitnexusIndexVersion: input.gitnexusIndexVersion } : {}),
    selectedFiles: [...new Set(input.selectedFiles)].sort(),
    contextHash,
    createdAt: (input.now ?? (() => new Date()))().toISOString()
  };
}

/**
 * Recomputes the hash from the current worktree and compares it with the recorded snapshot.
 * Used before resuming or rebuilding an execution: a changed context must be a decision, not a
 * surprise.
 */
export async function verifyContextSnapshot(
  record: ContextSnapshotRecord,
  input: Pick<ContextCaptureInput, "git" | "worktreePath" | "documents" | "selectedFiles">
): Promise<{ matches: boolean; actualHash: string; differences: string[] }> {
  const differences: string[] = [];
  const documentVersions: Record<string, string> = {};
  for (const document of [...new Set(input.documents)].sort()) {
    documentVersions[document] = `sha256:${await hashFile(safeResolve(input.worktreePath, document))}`;
  }
  const files: Array<{ path: string; hash: string }> = [];
  for (const file of [...new Set(input.selectedFiles)].sort()) {
    files.push({ path: file, hash: await hashFile(safeResolve(input.worktreePath, file)) });
  }

  for (const [document, version] of Object.entries(record.documentVersions)) {
    if (documentVersions[document] !== version) differences.push(`document changed: ${document}`);
  }
  for (const document of Object.keys(documentVersions)) {
    if (!(document in record.documentVersions)) differences.push(`document added: ${document}`);
  }

  const actualHash = hashContext({
    projectId: record.projectId,
    taskId: record.taskId,
    gitBaseSha: record.gitBaseSha,
    architectureVersion: record.architectureVersion,
    gitnexusIndexVersion: record.gitnexusIndexVersion ?? null,
    documentVersions,
    files
  });
  return { matches: actualHash === record.contextHash, actualHash, differences };
}

export interface ContextHashInput {
  projectId: string;
  taskId: string;
  gitBaseSha: string;
  architectureVersion: string;
  gitnexusIndexVersion: string | null;
  documentVersions: Record<string, string>;
  files: Array<{ path: string; hash: string }>;
}

/** Canonical JSON with sorted keys and sorted arrays, so the hash is order-independent. */
export function hashContext(input: ContextHashInput): string {
  const canonical = {
    projectId: input.projectId,
    taskId: input.taskId,
    gitBaseSha: input.gitBaseSha,
    architectureVersion: input.architectureVersion,
    gitnexusIndexVersion: input.gitnexusIndexVersion,
    documentVersions: Object.fromEntries(
      Object.entries(input.documentVersions).sort(([a], [b]) => a.localeCompare(b))
    ),
    files: [...input.files].sort((a, b) => a.path.localeCompare(b.path))
  };
  return createHash("sha256").update(JSON.stringify(canonical), "utf8").digest("hex");
}

async function hashFile(path: string): Promise<string> {
  let content: Buffer;
  try {
    content = await readFile(path);
  } catch (error) {
    throw new ContextCaptureError(
      `Cannot read context file '${path}': ${error instanceof Error ? error.message : String(error)}`
    );
  }
  return createHash("sha256").update(content).digest("hex");
}

function safeResolve(root: string, relativePath: string): string {
  const path = resolve(root, relativePath);
  if (!isInside(root, path)) {
    throw new ContextCaptureError(`Context path '${relativePath}' escapes the worktree`);
  }
  return path;
}
