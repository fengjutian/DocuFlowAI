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
  /** SHA-256 of the skill content, so a version cannot be silently edited. */
  checksum: string;
  createdAt: string;
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
