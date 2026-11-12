import {
  AgentAdapterError,
  type AgentCapability,
  type AgentRuntimeAdapter,
  type AgentRuntimeType
} from "../types/adapter.js";

export interface AgentRuntimeRegistration {
  /** Stable CodeWeaver runtime id, for example `hermes-local`. */
  id: string;
  runtimeType: AgentRuntimeType;
  adapter: AgentRuntimeAdapter;
  /** Version string of the runtime binary or API the adapter was pinned to. */
  version?: string;
  metadata?: Record<string, string>;
  /** Disabled runtimes remain visible for audit and can be enabled again, but receive no new work. */
  enabled?: boolean;
  /** Scheduling tier. Only the highest tier among the candidates is eligible (DF-0301). */
  priority?: number;
  /** Free-form labels the operator attached to this runtime. */
  tags?: string[];
}

/**
 * The persisted configuration of a runtime, applied over what was registered.
 *
 * The overlay is a separate layer rather than a field of the registration: the configuration is
 * restored from disk at startup, and a runtime that registers later must receive the same
 * configuration as one that registered before it. Reading the effective value through the overlay
 * makes the order in which runtimes appear irrelevant (DF-0301).
 */
export interface RuntimeConfigOverlay {
  enabled?: boolean;
  priority?: number;
  tags?: string[];
}

export interface RuntimeHealth {
  id: string;
  runtimeType: AgentRuntimeType;
  healthy: boolean;
  checkedAt: string;
  latencyMs: number;
  error?: string;
}

export interface RuntimeHealthReport {
  checkedAt: string;
  healthy: boolean;
  runtimes: RuntimeHealth[];
}

/**
 * Registration surface for Phase 1 item 3: runtime registration and health checks.
 *
 * The registry never decides policy. It only answers "which adapter serves this runtime id" and
 * "is that runtime currently reachable", so the Runtime Manager can fail closed when a runtime is
 * disabled or unhealthy.
 */
export class AgentRuntimeRegistry {
  private readonly registrations = new Map<string, AgentRuntimeRegistration>();
  private readonly now: () => Date;
  /** The configuration that applies over what was registered. */
  private overlay = new Map<string, RuntimeConfigOverlay>();

  constructor(options: { now?: () => Date } = {}) {
    this.now = options.now ?? (() => new Date());
  }

  register(registration: AgentRuntimeRegistration): void {
    if (!registration.id.trim()) throw new AgentAdapterError("invalid_state", "Runtime id must not be empty");
    if (this.registrations.has(registration.id)) {
      throw new AgentAdapterError("invalid_state", `Runtime ${registration.id} is already registered`);
    }
    this.registrations.set(registration.id, { ...registration });
  }

  unregister(id: string): boolean {
    this.overlay.delete(id);
    return this.registrations.delete(id);
  }

  has(id: string): boolean {
    return this.registrations.has(id);
  }

  get(id: string): AgentRuntimeAdapter | undefined {
    const registration = this.registrationOf(id);
    return registration?.enabled === false ? undefined : registration?.adapter;
  }

  require(id: string): AgentRuntimeAdapter {
    const registration = this.registrationOf(id);
    if (!registration) {
      throw new AgentAdapterError("execution_not_found", `Runtime ${id} is not registered`);
    }
    if (registration.enabled === false) {
      throw new AgentAdapterError("invalid_state", `Runtime ${id} is disabled`);
    }
    return registration.adapter;
  }

  /**
   * The adapter behind a runtime, whether it is enabled or not.
   *
   * Disabling a runtime stops new work; it does not cut the sessions that are already running on
   * it (DF-0303). Recovery and follow-up need to reach the adapter of a disabled runtime for exactly
   * that reason: the session it belongs to still exists.
   */
  adapterOf(id: string): AgentRuntimeAdapter | undefined {
    return this.registrations.get(id)?.adapter;
  }

  /** The registration as it is configured right now, including a disabled one. */
  registrationOf(id: string): AgentRuntimeRegistration | undefined {
    const registration = this.registrations.get(id);
    if (!registration) return undefined;
    return this.withOverlay(registration);
  }

  /**
   * Applies the persisted configuration.
   *
   * The overlay replaces whatever was applied before: a configuration entry that was removed from
   * disk must stop applying, so the runtime falls back to what it was registered with.
   */
  setConfigOverlay(overlay: Record<string, RuntimeConfigOverlay>): void {
    this.overlay = new Map(Object.entries(overlay));
  }

  /** The configuration currently applied, as the API reports it. */
  configOverlay(): Record<string, RuntimeConfigOverlay> {
    return Object.fromEntries(this.overlay.entries());
  }

  setEnabled(id: string, enabled: boolean): AgentRuntimeRegistration {
    return this.update(id, { enabled });
  }

  /**
   * Changes the configuration of a runtime.
   *
   * The change is recorded in the overlay rather than in the registration, so a later restore of the
   * persisted configuration can still override it.
   */
  update(id: string, patch: RuntimeConfigOverlay): AgentRuntimeRegistration {
    const registration = this.registrations.get(id);
    if (!registration) {
      throw new AgentAdapterError("execution_not_found", `Runtime ${id} is not registered`);
    }
    const current = this.overlay.get(id) ?? {};
    this.overlay.set(id, { ...current, ...patch });
    return this.withOverlay(registration);
  }

  list(): AgentRuntimeRegistration[] {
    return [...this.registrations.values()].map(registration => this.withOverlay(registration));
  }

  async capabilities(id: string): Promise<AgentCapability> {
    return this.require(id).getCapabilities();
  }

  /** Checks every registered runtime; a hung or crashing adapter becomes `healthy: false`. */
  async healthCheckAll(timeoutMs = 5_000): Promise<RuntimeHealthReport> {
    const checkedAt = this.now().toISOString();
    const runtimes = await Promise.all(
      this.list().map(async registration => registration.enabled === false
        ? {
            id: registration.id,
            runtimeType: registration.runtimeType,
            healthy: false,
            checkedAt,
            latencyMs: 0,
            error: "Runtime is disabled"
          }
        : this.checkOne(registration, timeoutMs, checkedAt))
    );
    return {
      checkedAt,
      healthy: runtimes.every(item => item.healthy),
      runtimes
    };
  }

  /** The registration as it is configured right now: the registered values under the overlay. */
  private withOverlay(registration: AgentRuntimeRegistration): AgentRuntimeRegistration {
    const overlay = this.overlay.get(registration.id);
    return {
      ...registration,
      enabled: overlay?.enabled ?? registration.enabled ?? true,
      priority: overlay?.priority ?? registration.priority ?? 0,
      tags: overlay?.tags ? [...overlay.tags] : [...(registration.tags ?? [])]
    };
  }

  private async checkOne(
    registration: AgentRuntimeRegistration,
    timeoutMs: number,
    checkedAt: string
  ): Promise<RuntimeHealth> {
    const startedAt = Date.now();
    try {
      const healthy = await withTimeout(registration.adapter.healthCheck(), timeoutMs);
      return {
        id: registration.id,
        runtimeType: registration.runtimeType,
        healthy,
        checkedAt,
        latencyMs: Date.now() - startedAt,
        ...(healthy ? {} : { error: "healthCheck() returned false" })
      };
    } catch (error) {
      return {
        id: registration.id,
        runtimeType: registration.runtimeType,
        healthy: false,
        checkedAt,
        latencyMs: Date.now() - startedAt,
        error: error instanceof Error ? error.message : String(error)
      };
    }
  }
}

async function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new AgentAdapterError("invalid_state", "timeoutMs must be a positive number");
  }
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new AgentAdapterError("runtime_unreachable", `Health check timed out after ${timeoutMs}ms`)),
          timeoutMs
        );
      })
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}
