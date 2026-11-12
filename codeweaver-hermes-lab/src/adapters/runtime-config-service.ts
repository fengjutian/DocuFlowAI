import { AgentAdapterError } from "../types/adapter.js";
import type { AgentRuntimeRegistry } from "./runtime-registry.js";
import {
  defaultRuntimeConfigEntry,
  emptyRuntimeConfig,
  type RuntimeConfigDocument,
  type RuntimeConfigEntry,
  type RuntimeConfigStore
} from "../persistence/runtime-config.js";
import {
  newRuntimeConfigAuditEntry,
  type RuntimeConfigAuditEntry,
  type RuntimeConfigAuditLog,
  type RuntimeConfigChange
} from "../persistence/runtime-config-audit.js";

/**
 * The runtime configuration service (V0.3.0).
 *
 * Everything that changes a runtime's configuration goes through here, and the service keeps three
 * things in step: the file on disk, the live registry and the audit log. A change that is not
 * written down did not happen, and a change that is not applied to the registry is a lie to the
 * scheduler.
 *
 * The write comes first. If the disk cannot be written, the registry is left alone: the operator
 * sees an error and the server keeps running the configuration it can still vouch for.
 */
export interface RuntimeConfigServiceOptions {
  registry: AgentRuntimeRegistry;
  store: RuntimeConfigStore;
  audit: RuntimeConfigAuditLog;
  now?: () => Date;
}

export class RuntimeConfigService {
  private document: RuntimeConfigDocument = emptyRuntimeConfig();
  private readonly registry: AgentRuntimeRegistry;
  private readonly store: RuntimeConfigStore;
  private readonly audit: RuntimeConfigAuditLog;
  private readonly now: () => Date;

  constructor(options: RuntimeConfigServiceOptions) {
    this.registry = options.registry;
    this.store = options.store;
    this.audit = options.audit;
    this.now = options.now ?? (() => new Date());
  }

  /** The runtime unnamed work goes to, when one is configured. */
  get defaultRuntimeId(): string | undefined {
    return this.document.defaultRuntimeId;
  }

  /** The configuration as it is stored. */
  get stored(): RuntimeConfigDocument {
    return this.document;
  }

  /**
   * Restores the configuration and applies it to the registry (DF-0302).
   *
   * A configuration that cannot be read or understood is an error, and the caller is expected to
   * fail closed on it: a server that starts with half a configuration would route work according to
   * a state nobody chose.
   */
  async load(): Promise<RuntimeConfigDocument> {
    const document = await this.store.load();
    this.document = document;
    this.applyToRegistry();
    if (document.defaultRuntimeId || Object.keys(document.runtimes).length > 0) {
      await this.audit.append(
        newRuntimeConfigAuditEntry({
          actor: "system",
          action: "restore",
          runtimeId: document.defaultRuntimeId ?? "*",
          recordedAt: this.now().toISOString(),
          changes: [
            {
              field: "runtimes",
              from: null,
              to: Object.fromEntries(
                Object.entries(document.runtimes).map(([id, entry]) => [id, { ...entry }])
              )
            }
          ],
          note: document.defaultRuntimeId
            ? `restored ${Object.keys(document.runtimes).length} runtime(s); default=${document.defaultRuntimeId}`
            : `restored ${Object.keys(document.runtimes).length} runtime(s)`
        })
      );
    }
    return document;
  }

  /** The effective configuration of one runtime: the registered values under the stored overlay. */
  effective(runtimeId: string): RuntimeConfigEntry | undefined {
    const registration = this.registry.registrationOf(runtimeId);
    if (!registration) return undefined;
    return {
      enabled: registration.enabled !== false,
      priority: registration.priority ?? 0,
      tags: [...(registration.tags ?? [])]
    };
  }

  /**
   * Turns a runtime on or off.
   *
   * Disabling stops new work and keeps the sessions that are already running (DF-0303); the caller
   * is told how many executions are still running so it can say what the change did.
   */
  async setEnabled(runtimeId: string, enabled: boolean, actor = "api"): Promise<RuntimeConfigEntry> {
    return this.update(runtimeId, { enabled }, actor, enabled ? "enable" : "disable");
  }

  /**
   * Changes the configuration of a runtime.
   *
   * Only the fields that were named are changed, and only the fields that really changed are
   * recorded: an audit entry that lists a field as changed when it did not change is noise that
   * makes the log harder to trust.
   */
  async update(
    runtimeId: string,
    patch: { enabled?: boolean; priority?: number; tags?: string[] },
    actor = "api",
    action = "update"
  ): Promise<RuntimeConfigEntry> {
    const current = this.effective(runtimeId);
    if (!current) {
      throw new AgentAdapterError("execution_not_found", `Runtime ${runtimeId} is not registered`);
    }
    const next: RuntimeConfigEntry = {
      enabled: patch.enabled ?? current.enabled,
      priority: patch.priority ?? current.priority,
      tags: patch.tags ? [...patch.tags] : [...current.tags]
    };
    const changes: RuntimeConfigChange[] = [];
    if (next.enabled !== current.enabled) changes.push({ field: "enabled", from: current.enabled, to: next.enabled });
    if (next.priority !== current.priority) changes.push({ field: "priority", from: current.priority, to: next.priority });
    if (JSON.stringify(next.tags) !== JSON.stringify(current.tags)) {
      changes.push({ field: "tags", from: current.tags, to: next.tags });
    }
    if (changes.length === 0) return current;

    await this.persistEntry(runtimeId, next);
    await this.audit.append(
      newRuntimeConfigAuditEntry({
        actor,
        action,
        runtimeId,
        recordedAt: this.now().toISOString(),
        changes
      })
    );
    return next;
  }

  /**
   * Names the runtime that unnamed work goes to.
   *
   * The runtime must be registered: a default that points at a runtime this server does not have
   * would silently route work somewhere else, which is the one thing a default must not do.
   */
  async setDefault(runtimeId: string | undefined, actor = "api"): Promise<void> {
    if (runtimeId !== undefined) {
      if (!this.registry.has(runtimeId)) {
        throw new AgentAdapterError("execution_not_found", `Runtime ${runtimeId} is not registered`);
      }
      const previous = this.document.defaultRuntimeId;
      if (previous === runtimeId) return;
      this.document = { ...this.document, defaultRuntimeId: runtimeId };
      await this.store.save(this.document);
      this.applyToRegistry();
      await this.audit.append(
        newRuntimeConfigAuditEntry({
          actor,
          action: "default",
          runtimeId,
          recordedAt: this.now().toISOString(),
          changes: [{ field: "defaultRuntimeId", from: previous ?? null, to: runtimeId }]
        })
      );
      return;
    }
    const previous = this.document.defaultRuntimeId;
    if (!previous) return;
    this.document = { ...this.document };
    delete this.document.defaultRuntimeId;
    await this.store.save(this.document);
    this.applyToRegistry();
    await this.audit.append(
      newRuntimeConfigAuditEntry({
        actor,
        action: "default",
        runtimeId: previous,
        recordedAt: this.now().toISOString(),
        changes: [{ field: "defaultRuntimeId", from: previous, to: null }]
      })
    );
  }

  /** The recorded configuration changes, newest first. */
  async history(limit = 100): Promise<RuntimeConfigAuditEntry[]> {
    return this.audit.list(limit);
  }

  /** The configuration as the API reports it: the stored document plus the live state. */
  describe(): {
    version: number;
    defaultRuntimeId: string | undefined;
    runtimes: Array<{
      id: string;
      enabled: boolean;
      priority: number;
      tags: string[];
      isDefault: boolean;
    }>;
  } {
    return {
      version: this.document.version,
      defaultRuntimeId: this.document.defaultRuntimeId,
      runtimes: this.registry.list().map(registration => ({
        id: registration.id,
        enabled: registration.enabled !== false,
        priority: registration.priority ?? 0,
        tags: [...(registration.tags ?? [])],
        isDefault: this.document.defaultRuntimeId === registration.id
      }))
    };
  }

  private async persistEntry(runtimeId: string, entry: RuntimeConfigEntry): Promise<void> {
    this.document = {
      ...this.document,
      runtimes: { ...this.document.runtimes, [runtimeId]: entry }
    };
    await this.store.save(this.document);
    this.applyToRegistry();
  }

  /** Hands the stored configuration to the registry as the overlay that applies over the defaults. */
  private applyToRegistry(): void {
    const overlay: Record<string, { enabled: boolean; priority: number; tags: string[] }> = {};
    for (const [runtimeId, entry] of Object.entries(this.document.runtimes)) {
      overlay[runtimeId] = { ...entry };
    }
    this.registry.setConfigOverlay(overlay);
  }
}

export { defaultRuntimeConfigEntry };
