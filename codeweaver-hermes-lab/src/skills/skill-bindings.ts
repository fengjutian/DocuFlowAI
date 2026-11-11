import { createHash } from "node:crypto";
import type { RecordStore } from "../persistence/record-store.js";
import type { SkillManifest } from "./skill.js";
import type { SkillAuthorization } from "./skill-registry.js";

/** Design 11.5 state: an execution is bound to an exact skill version, with an enabled flag. */
export interface SkillBinding {
  id: string;
  executionId: string;
  skillId: string;
  skillVersion: string;
  policyId: string;
  enabled: boolean;
  createdAt: string;
}

/**
 * Append-only binding event. `enabled` is mutable state, so instead of overwriting a record the store
 * appends `enable` / `disable` events and folds them; that keeps the file store immutable.
 */
export type SkillBindingAction = "bind" | "enable" | "disable";

export interface SkillBindingEvent {
  id: string;
  executionId: string;
  skillId: string;
  skillVersion: string;
  policyId: string;
  action: SkillBindingAction;
  createdAt: string;
}

export class SkillBindingError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SkillBindingError";
  }
}

export class SkillBindingStore {
  private readonly bindings = new Map<string, SkillBinding>();
  private sequence = 0;

  constructor(
    private readonly store?: RecordStore<SkillBindingEvent>,
    private readonly options: { now?: () => Date } = {}
  ) {}

  /** Rebuilds current bindings by folding the event log in creation order. */
  async load(): Promise<void> {
    if (!this.store) return;
    const events = (await this.store.list()).sort((a, b) => {
      const byTime = a.createdAt.localeCompare(b.createdAt);
      return byTime !== 0 ? byTime : a.id.localeCompare(b.id);
    });
    for (const event of events) this.apply(event);
  }

  async bind(input: {
    executionId: string;
    manifest: SkillManifest;
    policyId: string;
    authorization: SkillAuthorization;
    idFactory?: () => string;
  }): Promise<SkillBinding> {
    if (input.authorization.decision === "deny") {
      throw new SkillBindingError(
        `Cannot bind denied skill ${input.manifest.skillId}@${input.manifest.version}: ${input.authorization.reasons.join("; ")}`
      );
    }
    const key = bindingKey(input.executionId, input.manifest.skillId, input.manifest.version);
    const existing = this.bindings.get(key);
    if (existing) return existing;

    const event: SkillBindingEvent = {
      id: input.idFactory?.() ?? this.nextEventId("bind", input),
      executionId: input.executionId,
      skillId: input.manifest.skillId,
      skillVersion: input.manifest.version,
      policyId: input.policyId,
      action: "bind",
      createdAt: this.now()
    };
    await this.append(event);
    return this.apply(event);
  }

  /** Disabling keeps the record: design 11.5 has an `enabled` flag rather than a delete. */
  async setEnabled(
    executionId: string,
    skillId: string,
    version: string,
    enabled: boolean
  ): Promise<SkillBinding> {
    const key = bindingKey(executionId, skillId, version);
    const existing = this.bindings.get(key);
    if (!existing) throw new SkillBindingError(`No binding for ${skillId}@${version} in ${executionId}`);

    const event: SkillBindingEvent = {
      id: this.nextEventId(enabled ? "enable" : "disable", { executionId, manifest: { skillId, version } }),
      executionId,
      skillId,
      skillVersion: version,
      policyId: existing.policyId,
      action: enabled ? "enable" : "disable",
      createdAt: this.now()
    };
    await this.append(event);
    return this.apply(event);
  }

  forExecution(executionId: string): SkillBinding[] {
    return [...this.bindings.values()]
      .filter(binding => binding.executionId === executionId)
      .sort((a, b) => a.skillId.localeCompare(b.skillId));
  }

  list(): SkillBinding[] {
    return [...this.bindings.values()];
  }

  private async append(event: SkillBindingEvent): Promise<void> {
    await this.store?.put(event);
  }

  private apply(event: SkillBindingEvent): SkillBinding {
    const key = bindingKey(event.executionId, event.skillId, event.skillVersion);
    const current = this.bindings.get(key);
    const binding: SkillBinding = {
      id: current?.id ?? event.id,
      executionId: event.executionId,
      skillId: event.skillId,
      skillVersion: event.skillVersion,
      policyId: event.policyId,
      enabled: event.action !== "disable",
      createdAt: current?.createdAt ?? event.createdAt
    };
    this.bindings.set(key, Object.freeze(binding));
    return binding;
  }

  private nextEventId(
    action: string,
    input: { executionId: string; manifest: { skillId: string; version: string } }
  ): string {
    this.sequence += 1;
    const digest = createHash("sha256")
      .update(`${input.executionId}|${input.manifest.skillId}@${input.manifest.version}|${action}|${this.sequence}`)
      .digest("hex")
      .slice(0, 24);
    return `bind_${digest}`;
  }

  private now(): string {
    return (this.options.now ?? (() => new Date()))().toISOString();
  }
}

function bindingKey(executionId: string, skillId: string, version: string): string {
  return `${executionId}:${skillId}@${version}`;
}
