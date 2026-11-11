import type { JournalEntry, JournalStore } from "../journal/journal.js";
import { CODEWEAVER_TOOL_NAMES } from "../tools/codeweaver-tools.js";
import { NO_CAPABILITIES, type AgentCapability, type AgentRuntimeType } from "../types/adapter.js";
import type { AgentRuntimeRegistry } from "./runtime-registry.js";
import {
  compareRuntimeScores,
  observationsFromJournal,
  scoreRuntime,
  type RuntimeCapabilityEvidence,
  type RuntimeComparison,
  type RuntimeExecutionObservation,
  type RuntimeScore
} from "./runtime-scoring.js";

/**
 * Turns the recorded runs into a runtime score report.
 *
 * The scorer itself is pure: it takes observations and returns a number. This module is the part
 * that has to look at the world, and it looks at exactly two things:
 *
 *  - the journal, which is what the runtimes actually emitted, and
 *  - the registry, which is what each runtime says about itself right now.
 *
 * Nothing here invents evidence. A runtime that never ran has no executions, and a runtime whose
 * journal says nothing about a capability has not demonstrated it.
 */

export interface RuntimeScoreCollectorDeps {
  registry: AgentRuntimeRegistry;
  journal: JournalStore;
}

export interface RuntimeScoreReport {
  /** One score per runtime, highest first. */
  runtimes: RuntimeScore[];
  /** The same runtimes as a table, for a console or a report. */
  comparison: RuntimeComparison;
  /** Executions in the journal that named no runtime; counted, attributed to nobody. */
  unattributedExecutions: number;
  /** How many executions the journal holds in total. */
  executionsInJournal: number;
}

/**
 * Collects the score of every registered runtime, plus every runtime the journal remembers.
 *
 * A runtime that is no longer registered still has a history, and that history is evidence: a
 * restart with a different configuration must not erase what the previous runtime did. Such
 * runtimes are reported with the `external` type, because their real type is no longer knowable.
 */
export async function collectRuntimeScores(deps: RuntimeScoreCollectorDeps): Promise<RuntimeScoreReport> {
  const executionIds = await deps.journal.listExecutions();
  const entriesByExecution = new Map<string, JournalEntry[]>();
  for (const executionId of executionIds) {
    entriesByExecution.set(executionId, await deps.journal.read(executionId));
  }

  const entriesByRuntime = new Map<string, JournalEntry[]>();
  let unattributed = 0;
  for (const entries of entriesByExecution.values()) {
    const runtimeId = runtimeIdOf(entries);
    if (!runtimeId) {
      unattributed += 1;
      continue;
    }
    entriesByRuntime.set(runtimeId, [...(entriesByRuntime.get(runtimeId) ?? []), ...entries]);
  }

  const registrations = new Map<string, { runtimeType: AgentRuntimeType; version?: string }>();
  for (const entry of deps.registry.list()) {
    registrations.set(entry.id, {
      runtimeType: entry.runtimeType,
      ...(entry.version ? { version: entry.version } : {})
    });
  }
  // The journal may name a runtime this process does not have registered.
  for (const runtimeId of entriesByRuntime.keys()) {
    if (!registrations.has(runtimeId)) registrations.set(runtimeId, { runtimeType: "external" });
  }

  const scores: RuntimeScore[] = [];
  for (const [runtimeId, registration] of registrations) {
    const entries = entriesByRuntime.get(runtimeId) ?? [];
    const observations = observationsFromJournal(entries);
    const declared = await declaredCapabilities(deps.registry, runtimeId);
    const capabilityEvidence: RuntimeCapabilityEvidence = {
      declared,
      exercised: exercisedCapabilities(entries),
      observed: observedCapabilities(observations)
    };
    scores.push(
      scoreRuntime({
        runtimeId,
        runtimeType: registration.runtimeType,
        ...(registration.version ? { version: registration.version } : {}),
        capabilityEvidence,
        executions: observations
      })
    );
  }

  return {
    runtimes: scores,
    comparison: compareRuntimeScores(scores),
    unattributedExecutions: unattributed,
    executionsInJournal: entriesByExecution.size
  };
}

/** The runtime a run was attributed to, as the journal recorded it. */
function runtimeIdOf(entries: readonly JournalEntry[]): string | undefined {
  const fact = entries.find(entry => entry.category === "execution.runtime");
  const runtimeId = fact?.payload["runtimeId"];
  return typeof runtimeId === "string" && runtimeId !== "" ? runtimeId : undefined;
}

/**
 * What the runtime claims about itself.
 *
 * The claim comes from the runtime, never from a table in this repository (D-043). A runtime that
 * cannot answer is treated as claiming nothing, which is the same answer an unregistered runtime
 * gets: no capability means no capability.
 */
async function declaredCapabilities(
  registry: AgentRuntimeRegistry,
  runtimeId: string
): Promise<AgentCapability> {
  try {
    return await registry.capabilities(runtimeId);
  } catch {
    return { ...NO_CAPABILITIES };
  }
}

/**
 * The capabilities the workload gave the runtime a chance to demonstrate.
 *
 * "Exercised" is a property of the task, not of the runtime: a runtime cannot be credited or
 * blamed for a capability the task never asked of it. The rules, one per capability:
 *
 *  - streaming: every execution asks the runtime for an answer, so it is always exercised.
 *  - toolEvents: the task bound at least one tool.
 *  - permissionEvents: the task bound a tool the policy has to gate, which is any tool that is
 *    not one of CodeWeaver's own self-enforced tools.
 *  - skills: the task bound a skill.
 *  - subAgents: the task delegated to a subagent.
 *  - cancellation: CodeWeaver asked the execution to stop.
 *  - sessionResume: the execution was resumed, which the journal shows as a second start.
 */
export function exercisedCapabilities(entries: readonly JournalEntry[]): Partial<AgentCapability> {
  const exercised: Partial<AgentCapability> = { streaming: true };
  const input = entries.find(entry => entry.category === "execution.input");
  const allowedTools = Array.isArray(input?.payload["allowedTools"])
    ? input.payload["allowedTools"].map(item => String(item))
    : [];
  if (allowedTools.length > 0) exercised.toolEvents = true;
  if (allowedTools.some(name => !SELF_ENFORCED_TOOLS.has(name))) exercised.permissionEvents = true;
  if (entries.some(entry => entry.category === "skill.bound")) exercised.skills = true;
  if (entries.some(entry => entry.category === "subagent.spawned")) exercised.subAgents = true;
  if (entries.some(entry => entry.category === "execution.cancel_requested")) exercised.cancellation = true;
  if (entries.filter(entry => entry.category === "execution.started").length > 1) exercised.sessionResume = true;
  return exercised;
}

/**
 * The capabilities the runtime actually demonstrated.
 *
 * Only what the runtime emitted counts (D-008): boundary evidence — a field in a request, a
 * capability in a registration — is not a demonstration.
 */
export function observedCapabilities(
  observations: readonly RuntimeExecutionObservation[]
): Partial<AgentCapability> {
  const observed: Partial<AgentCapability> = {};
  if (observations.some(item => item.messageDeltaCount > 0 || item.messageCompletedCount > 0)) {
    observed.streaming = true;
  }
  if (observations.some(item => item.toolEventCount > 0)) observed.toolEvents = true;
  if (observations.some(item => item.permissionEventCount > 0)) observed.permissionEvents = true;
  if (observations.some(item => item.cancelConfirmedByRuntime)) observed.cancellation = true;
  if (observations.some(item => item.resumeObserved)) observed.sessionResume = true;
  if (observations.some(item => item.skillBound)) observed.skills = true;
  if (observations.some(item => item.subAgentObserved)) observed.subAgents = true;
  return observed;
}

const SELF_ENFORCED_TOOLS = new Set<string>(CODEWEAVER_TOOL_NAMES);
