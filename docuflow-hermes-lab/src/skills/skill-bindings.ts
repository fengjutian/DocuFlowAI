import { randomUUID } from "node:crypto";
import type { SkillManifest } from "./skill.js";
import type { SkillAuthorization } from "./skill-registry.js";

/** Mirrors design 11.5: an execution is bound to an exact skill version. */
export interface SkillBinding {
  id: string;
  executionId: string;
  skillId: string;
  skillVersion: string;
  policyId: string;
  enabled: boolean;
  createdAt: string;
}

export class SkillBindingError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SkillBindingError";
  }
}

/**
 * Records which skill versions an execution used.
 *
 * Binding a denied skill is an error, not a silent no-op, and binding never inherits permissions: the
 * authorization was computed against the execution's own policy snapshot and is passed in explicitly.
 */
export class SkillBindingStore {
  private readonly bindings = new Map<string, SkillBinding>();

  bind(input: {
    executionId: string;
    manifest: SkillManifest;
    policyId: string;
    authorization: SkillAuthorization;
    now?: () => Date;
    idFactory?: () => string;
  }): SkillBinding {
    if (input.authorization.decision === "deny") {
      throw new SkillBindingError(
        `Cannot bind denied skill ${input.manifest.skillId}@${input.manifest.version}: ${input.authorization.reasons.join("; ")}`
      );
    }
    const key = `${input.executionId}:${input.manifest.skillId}@${input.manifest.version}`;
    const existing = this.bindings.get(key);
    if (existing) return existing;

    const binding: SkillBinding = {
      id: (input.idFactory ?? (() => `bind_${randomUUID()}`))(),
      executionId: input.executionId,
      skillId: input.manifest.skillId,
      skillVersion: input.manifest.version,
      policyId: input.policyId,
      enabled: true,
      createdAt: (input.now ?? (() => new Date()))().toISOString()
    };
    this.bindings.set(key, Object.freeze(binding));
    return binding;
  }

  forExecution(executionId: string): SkillBinding[] {
    return [...this.bindings.values()]
      .filter(binding => binding.executionId === executionId)
      .sort((a, b) => a.skillId.localeCompare(b.skillId));
  }

  /** Disabling keeps the record: design 11.5 has an `enabled` flag rather than a delete. */
  setEnabled(executionId: string, skillId: string, version: string, enabled: boolean): SkillBinding {
    const key = `${executionId}:${skillId}@${version}`;
    const existing = this.bindings.get(key);
    if (!existing) throw new SkillBindingError(`No binding for ${skillId}@${version} in ${executionId}`);
    const updated: SkillBinding = Object.freeze({ ...existing, enabled });
    this.bindings.set(key, updated);
    return updated;
  }

  list(): SkillBinding[] {
    return [...this.bindings.values()];
  }
}
