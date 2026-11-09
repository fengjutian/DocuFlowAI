import {
  AgentAdapterError,
  type AgentCapability,
  type AgentRuntimeAdapter,
  type AgentRuntimeType
} from "../types/adapter.js";

export interface AgentRuntimeRegistration {
  /** Stable DocuFlow runtime id, for example `hermes-local`. */
  id: string;
  runtimeType: AgentRuntimeType;
  adapter: AgentRuntimeAdapter;
  /** Version string of the runtime binary or API the adapter was pinned to. */
  version?: string;
  metadata?: Record<string, string>;
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

  constructor(options: { now?: () => Date } = {}) {
    this.now = options.now ?? (() => new Date());
  }

  register(registration: AgentRuntimeRegistration): void {
    if (!registration.id.trim()) throw new AgentAdapterError("invalid_state", "Runtime id must not be empty");
    if (this.registrations.has(registration.id)) {
      throw new AgentAdapterError("invalid_state", `Runtime ${registration.id} is already registered`);
    }
    this.registrations.set(registration.id, registration);
  }

  unregister(id: string): boolean {
    return this.registrations.delete(id);
  }

  has(id: string): boolean {
    return this.registrations.has(id);
  }

  get(id: string): AgentRuntimeAdapter | undefined {
    return this.registrations.get(id)?.adapter;
  }

  require(id: string): AgentRuntimeAdapter {
    const registration = this.registrations.get(id);
    if (!registration) {
      throw new AgentAdapterError("execution_not_found", `Runtime ${id} is not registered`);
    }
    return registration.adapter;
  }

  list(): AgentRuntimeRegistration[] {
    return [...this.registrations.values()];
  }

  async capabilities(id: string): Promise<AgentCapability> {
    return this.require(id).getCapabilities();
  }

  /** Checks every registered runtime; a hung or crashing adapter becomes `healthy: false`. */
  async healthCheckAll(timeoutMs = 5_000): Promise<RuntimeHealthReport> {
    const checkedAt = this.now().toISOString();
    const runtimes = await Promise.all(
      this.list().map(async registration => this.checkOne(registration, timeoutMs, checkedAt))
    );
    return {
      checkedAt,
      healthy: runtimes.every(item => item.healthy),
      runtimes
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
