import { createHash } from "node:crypto";
import type { PolicySnapshot } from "../policy/permission-policy.js";
import type { RecordStore } from "../persistence/record-store.js";
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
 *
 * The registry holds the manifest *and* its body: a skill the server cannot read back is a skill
 * it cannot deliver, and the body is what makes the checksum mean anything.
 */
export class SkillRegistry {
  private readonly skills = new Map<string, Map<string, SkillManifest>>();
  private readonly deprecated = new Set<string>();

  constructor(private readonly store?: RecordStore<SkillManifestRecord>) {}

  /**
   * Reads the registered manifests back from disk.
   *
   * Without this a restart empties the skill library: the console would show no skills and every
   * task would run without the instructions the user wrote.
   */
  async load(): Promise<void> {
    if (!this.store) return;
    for (const record of await this.store.list()) {
      // A manifest that no longer validates is not delivered. The failure is loud: a skill that
      // silently disappears would make a task run without instructions the user believes are bound.
      assertValidSkill(record.manifest);
      this.assertNotRegistered(record.manifest);
      this.setInMemory(record.manifest);
    }
  }

  async register(manifest: SkillManifest): Promise<void> {
    assertValidSkill(manifest);
    this.assertNotRegistered(manifest);
    // The manifest is on disk before the server believes it exists: a skill the server cannot read
    // back after a restart is a skill it cannot promise to deliver.
    await this.store?.put({
      id: skillRecordId(manifest.skillId, manifest.version),
      manifest,
      registeredAt: manifest.createdAt
    });
    this.setInMemory(manifest);
  }

  private assertNotRegistered(manifest: SkillManifest): void {
    if (this.skills.get(manifest.skillId)?.has(manifest.version)) {
      throw new SkillConflictError(
        `Skill ${manifest.skillId}@${manifest.version} is already registered; publish a new version`
      );
    }
  }

  private setInMemory(manifest: SkillManifest): void {
    const versions = this.skills.get(manifest.skillId) ?? new Map<string, SkillManifest>();
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
   *
   * An unknown runtime denies a runtime-specific skill: "we could not check" is not "it is fine".
   */
  authorize(
    manifest: SkillManifest,
    context: { runtimeId: string | undefined; policy: PolicySnapshot; now?: Date }
  ): SkillAuthorization {
    const reasons: string[] = [];
    const allowedRuntime =
      manifest.allowedRuntimes.includes("*") ||
      (context.runtimeId !== undefined && manifest.allowedRuntimes.includes(context.runtimeId));
    if (!allowedRuntime) {
      return {
        decision: "deny",
        reasons: [
          context.runtimeId
            ? `Skill ${manifest.skillId} may not run on runtime '${context.runtimeId}'`
            : `Skill ${manifest.skillId} names the runtimes it may run on, and the runtime of this task is unknown`
        ]
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

/** The stored form of a manifest: the manifest itself, under a file-safe id. */
export interface SkillManifestRecord {
  id: string;
  manifest: SkillManifest;
  registeredAt: string;
}

/**
 * A file-safe id for a skill version.
 *
 * The record id is a digest rather than the name because `@` is not a safe file name character and
 * skill ids are caller-supplied.
 */
export function skillRecordId(skillId: string, version: string): string {
  const digest = createHash("sha256").update(`${skillId}@${version}`, "utf8").digest("hex");
  return `skill_${digest.slice(0, 24)}`;
}
