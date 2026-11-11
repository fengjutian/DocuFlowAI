import { createHash } from "node:crypto";

/**
 * Skill manifest (design 10.3).
 *
 * A skill is not trusted because it is Markdown. It declares the permissions it needs, the runtimes
 * it may run on, its risk level and an approval policy; the registry refuses anything undeclared.
 */
export type SkillKind = "platform" | "engineering" | "hermes-native";

export type SkillPermission =
  | "read_file"
  | "search_code"
  | "modify_worktree"
  | "delete_file"
  | "install_dependency"
  | "run_command"
  | "network_access"
  | "git_merge";

export type RiskLevel = "low" | "medium" | "high";
export type SkillApprovalPolicy = "auto" | "require_approval";

export interface SkillManifest {
  skillId: string;
  version: string;
  name: string;
  description?: string;
  kind: SkillKind;
  permissions: SkillPermission[];
  /** Runtime ids this skill may execute on, or `*`. */
  allowedRuntimes: string[];
  riskLevel: RiskLevel;
  approvalPolicy: SkillApprovalPolicy;
  /**
   * The skill body: the Markdown the model is supposed to follow.
   *
   * A skill without a body is a permission declaration with nothing to say, so the body is part of
   * the manifest rather than a separate document. It is what the execution actually delivers.
   */
  content: string;
  /** SHA-256 of `content`, so a version cannot be silently edited. */
  checksum: string;
  createdAt: string;
}

/**
 * The largest skill body the registry accepts.
 *
 * A skill is injected into the prompt of the run that binds it, so an unbounded body is an unbounded
 * prompt. The limit is generous enough for a real playbook and small enough that one skill cannot
 * crowd out the task.
 */
export const MAX_SKILL_CONTENT_BYTES = 64 * 1024;

/** The checksum of a skill body: SHA-256 over its UTF-8 bytes. */
export function skillContentChecksum(content: string): string {
  return createHash("sha256").update(content, "utf8").digest("hex");
}

export const KNOWN_PERMISSIONS: readonly SkillPermission[] = [
  "read_file",
  "search_code",
  "modify_worktree",
  "delete_file",
  "install_dependency",
  "run_command",
  "network_access",
  "git_merge"
];

export class SkillValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SkillValidationError";
  }
}

export function assertValidSkill(manifest: SkillManifest): void {
  requireText(manifest.skillId, "skillId");
  requireText(manifest.name, "name");
  requireVersion(manifest.version);
  requireSkillContent(manifest.content);
  if (manifest.kind !== "platform" && manifest.kind !== "engineering" && manifest.kind !== "hermes-native") {
    throw new SkillValidationError(`Unknown skill kind '${String(manifest.kind)}'`);
  }
  if (!Array.isArray(manifest.permissions)) {
    throw new SkillValidationError("permissions must be an array");
  }
  for (const permission of manifest.permissions) {
    if (!KNOWN_PERMISSIONS.includes(permission)) {
      throw new SkillValidationError(`Unknown skill permission '${String(permission)}'`);
    }
  }
  if (!Array.isArray(manifest.allowedRuntimes) || manifest.allowedRuntimes.length === 0) {
    throw new SkillValidationError("allowedRuntimes must declare at least one runtime (use '*' for any)");
  }
  if (!/^[0-9a-f]{64}$/i.test(manifest.checksum)) {
    throw new SkillValidationError("checksum must be a SHA-256 hex digest");
  }
  // The checksum is the skill body's hash, so it can be checked. A checksum that does not match the
  // body means the body changed after the checksum was computed, which is exactly the silent edit
  // the immutable versions are supposed to prevent.
  if (manifest.checksum.toLowerCase() !== skillContentChecksum(manifest.content)) {
    throw new SkillValidationError(
      "checksum does not match the skill content; the content was edited after the checksum was computed"
    );
  }
  if (manifest.riskLevel !== "low" && manifest.riskLevel !== "medium" && manifest.riskLevel !== "high") {
    throw new SkillValidationError(`Unknown risk level '${String(manifest.riskLevel)}'`);
  }
  if (manifest.approvalPolicy !== "auto" && manifest.approvalPolicy !== "require_approval") {
    throw new SkillValidationError(`Unknown approval policy '${String(manifest.approvalPolicy)}'`);
  }
  if (Number.isNaN(Date.parse(manifest.createdAt))) {
    throw new SkillValidationError("createdAt must be a timestamp");
  }
}

function requireSkillContent(content: string): void {
  if (typeof content !== "string" || content.trim() === "") {
    throw new SkillValidationError("Skill field 'content' must be a non-empty string");
  }
  const bytes = Buffer.byteLength(content, "utf8");
  if (bytes > MAX_SKILL_CONTENT_BYTES) {
    throw new SkillValidationError(
      `Skill content is ${bytes} bytes, which is over the ${MAX_SKILL_CONTENT_BYTES} byte limit`
    );
  }
}

export interface Semver {
  major: number;
  minor: number;
  patch: number;
  prerelease: string | null;
}

const SEMVER = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/;

export function parseVersion(version: string): Semver {
  const match = SEMVER.exec(version.trim());
  if (!match) throw new SkillValidationError(`'${version}' is not a semantic version`);
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] ?? null
  };
}

/** Returns a negative number when `a < b`, 0 when equal, positive when `a > b`. */
export function compareVersions(a: string, b: string): number {
  const left = parseVersion(a);
  const right = parseVersion(b);
  for (const key of ["major", "minor", "patch"] as const) {
    if (left[key] !== right[key]) return left[key] - right[key];
  }
  if (left.prerelease === right.prerelease) return 0;
  if (left.prerelease === null) return 1;
  if (right.prerelease === null) return -1;
  return left.prerelease.localeCompare(right.prerelease);
}

function requireVersion(version: string): void {
  parseVersion(version);
}

function requireText(value: string, field: string): void {
  if (typeof value !== "string" || value.trim() === "") {
    throw new SkillValidationError(`Skill field '${field}' must be a non-empty string`);
  }
}
