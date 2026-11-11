import type { PolicySnapshot } from "../policy/permission-policy.js";
import {
  assertValidSkill,
  compareVersions,
  type SkillManifest
} from "./skill.js";

export class SkillConflictError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SkillConflictError";
  }
}

export class SkillNotFoundError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SkillNotFoundError";
  }
}

export type SkillDecision = "allow" | "deny" | "require_approval";

export interface SkillAuthorization {
  decision: SkillDecision;
  reasons: string[];
}

/**
 * Registry of immutable skill versions (design 10.2).
 *
 * A version is registered once. Publishing a change means publishing a new version, and a binding
 * always names the exact version it used, so "which skill did this run execute" stays answerable.
 */
export class SkillRegistry {
  private readonly skills = new Map<string, Map<string, SkillManifest>>();
  private readonly deprecated = new Set<string>();

  register(manifest: SkillManifest): void {
    assertValidSkill(manifest);
    const versions = this.skills.get(manifest.skillId) ?? new Map<string, SkillManifest>();
    if (versions.has(manifest.version)) {
      throw new SkillConflictError(
        `Skill ${manifest.skillId}@${manifest.version} is already registered; publish a new version`
      );
    }
    versions.set(manifest.version, Object.freeze({ ...manifest }));
    this.skills.set(manifest.skillId, versions);
  }

  /** Without a version, returns the highest non-deprecated version. */
  resolve(skillId: string, version?: string): SkillManifest {
    const versions = this.skills.get(skillId);
    if (!versions || versions.size === 0) {
      throw new SkillNotFoundError(`Skill ${skillId} is not registered`);
    }
    if (version) {
      const found = versions.get(version);
      if (!found) throw new SkillNotFoundError(`Skill ${skillId}@${version} is not registered`);
      return found;
    }
    const latest = this.listVersions(skillId).find(candidate => !this.deprecated.has(key(skillId, candidate)));
    if (!latest) throw new SkillNotFoundError(`Every version of skill ${skillId} is deprecated`);
    return versions.get(latest)!;
  }

  listVersions(skillId: string): string[] {
    return [...(this.skills.get(skillId)?.keys() ?? [])].sort((a, b) => compareVersions(b, a));
  }

  list(): SkillManifest[] {
    const all: SkillManifest[] = [];
    for (const skillId of [...this.skills.keys()].sort()) {
      for (const version of this.listVersions(skillId)) {
        all.push(this.skills.get(skillId)!.get(version)!);
      }
    }
    return all;
  }

  deprecate(skillId: string, version: string): void {
    this.resolve(skillId, version);
    this.deprecated.add(key(skillId, version));
  }

  isDeprecated(skillId: string, version: string): boolean {
    return this.deprecated.has(key(skillId, version));
  }

  /**
   * Design 10.3: declared permissions must already be granted by the execution's policy snapshot,
   * and the skill must be allowed on this runtime. Anything undeclared-but-required is a deny.
   */
  authorize(
    manifest: SkillManifest,
    context: { runtimeId: string; policy: PolicySnapshot; now?: Date }
  ): SkillAuthorization {
    const reasons: string[] = [];
    const allowedRuntime = manifest.allowedRuntimes.includes("*")
      || manifest.allowedRuntimes.includes(context.runtimeId);
    if (!allowedRuntime) {
      return {
        decision: "deny",
        reasons: [`Skill ${manifest.skillId} may not run on runtime '${context.runtimeId}'`]
      };
    }

    const granted = new Set(context.policy.grantedCapabilities);
    const wildcard = granted.has("*");
    const missing = manifest.permissions.filter(permission => !wildcard && !granted.has(permission));
    if (missing.length > 0) {
      return {
        decision: "deny",
        reasons: [`Policy snapshot does not grant: ${missing.join(", ")}`]
      };
    }

    const expiresAt = Date.parse(context.policy.expiresAt);
    if (Number.isNaN(expiresAt) || expiresAt <= (context.now ?? new Date()).getTime()) {
      return { decision: "deny", reasons: [`Policy snapshot expired at ${context.policy.expiresAt}`] };
    }

    const highRisk = manifest.riskLevel === "high" || manifest.approvalPolicy === "require_approval";
    if (highRisk) {
      reasons.push(`Risk level ${manifest.riskLevel} with approval policy '${manifest.approvalPolicy}'`);
      return { decision: "require_approval", reasons };
    }

    if (this.isDeprecated(manifest.skillId, manifest.version)) {
      return {
        decision: "require_approval",
        reasons: [`Skill ${manifest.skillId}@${manifest.version} is deprecated; bind only with approval`]
      };
    }

    reasons.push("All declared permissions are granted and the risk policy is automatic");
    return { decision: "allow", reasons };
  }
}

function key(skillId: string, version: string): string {
  return `${skillId}@${version}`;
}
