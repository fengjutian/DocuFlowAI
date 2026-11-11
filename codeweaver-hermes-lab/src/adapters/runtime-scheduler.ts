import {
  AgentAdapterError,
  type AgentCapability,
  type AgentRuntimeAdapter,
  type AgentRuntimeType
} from "../types/adapter.js";
import type { AgentRuntimeRegistration, AgentRuntimeRegistry } from "./runtime-registry.js";

/**
 * Multi-runtime selection (design 12.1 / Phase 4).
 *
 * Selection is health- and capability-aware before it considers load: sending work to a runtime that
 * cannot stream or is unreachable is worse than an unbalanced queue.
 */
export type SchedulingStrategy = "round_robin" | "least_loaded" | "first_healthy";

export interface SelectOptions {
  runtimeType?: AgentRuntimeType;
  requiredCapabilities?: Partial<AgentCapability>;
  /** Skip the health probe when the caller just performed one. */
  skipHealthCheck?: boolean;
}

export interface Selection {
  runtimeId: string;
  adapter: AgentRuntimeAdapter;
  registration: AgentRuntimeRegistration;
}

export class RuntimeScheduler {
  private cursor = 0;
  private readonly load = new Map<string, number>();
  private readonly strategy: SchedulingStrategy;
  private readonly healthTimeoutMs: number;

  constructor(
    private readonly registry: AgentRuntimeRegistry,
    options: { strategy?: SchedulingStrategy; healthTimeoutMs?: number } = {}
  ) {
    this.strategy = options.strategy ?? "least_loaded";
    this.healthTimeoutMs = options.healthTimeoutMs ?? 5_000;
  }

  async select(options: SelectOptions = {}): Promise<Selection> {
    const candidates = this.registry
      .list()
      .filter(entry => !options.runtimeType || entry.runtimeType === options.runtimeType);
    if (candidates.length === 0) {
      throw new AgentAdapterError(
        "invalid_state",
        `No runtime is registered${options.runtimeType ? ` for type '${options.runtimeType}'` : ""}`
      );
    }

    let healthy = candidates;
    if (!options.skipHealthCheck) {
      const report = await this.registry.healthCheckAll(this.healthTimeoutMs);
      const healthyIds = new Set(report.runtimes.filter(item => item.healthy).map(item => item.id));
      healthy = candidates.filter(entry => healthyIds.has(entry.id));
      if (healthy.length === 0) {
        throw new AgentAdapterError(
          "runtime_unreachable",
          `Every candidate runtime is unhealthy: ${candidates.map(entry => entry.id).join(", ")}`
        );
      }
    }

    const capable = await this.filterByCapabilities(healthy, options.requiredCapabilities);
    if (capable.length === 0) {
      throw new AgentAdapterError(
        "unsupported_capability",
        `No healthy runtime satisfies ${describeRequirements(options.requiredCapabilities)}`
      );
    }

    const chosen = this.pick(capable);
    return { runtimeId: chosen.id, adapter: chosen.adapter, registration: chosen };
  }

  /** Called by the Runtime Manager when an execution starts/finishes on a runtime. */
  markBusy(runtimeId: string): void {
    this.load.set(runtimeId, this.loadOf(runtimeId) + 1);
  }

  markIdle(runtimeId: string): void {
    this.load.set(runtimeId, Math.max(0, this.loadOf(runtimeId) - 1));
  }

  loadOf(runtimeId: string): number {
    return this.load.get(runtimeId) ?? 0;
  }

  private async filterByCapabilities(
    candidates: readonly AgentRuntimeRegistration[],
    required: Partial<AgentCapability> | undefined
  ): Promise<AgentRuntimeRegistration[]> {
    if (!required || Object.keys(required).length === 0) return [...candidates];
    const result: AgentRuntimeRegistration[] = [];
    for (const candidate of candidates) {
      try {
        const capabilities = await candidate.adapter.getCapabilities();
        const satisfies = Object.entries(required).every(
          ([key, value]) => capabilities[key as keyof AgentCapability] === value
        );
        if (satisfies) result.push(candidate);
      } catch {
        // A runtime that cannot even answer the capability question is not a candidate.
      }
    }
    return result;
  }

  private pick(candidates: readonly AgentRuntimeRegistration[]): AgentRuntimeRegistration {
    if (this.strategy === "first_healthy") return candidates[0]!;
    if (this.strategy === "round_robin") {
      const chosen = candidates[this.cursor % candidates.length]!;
      this.cursor = (this.cursor + 1) % candidates.length;
      return chosen;
    }
    return [...candidates].sort((a, b) => this.loadOf(a.id) - this.loadOf(b.id))[0]!;
  }
}

function describeRequirements(required: Partial<AgentCapability> | undefined): string {
  if (!required) return "the requested capabilities";
  return Object.entries(required)
    .map(([key, value]) => `${key}=${String(value)}`)
    .join(", ");
}
