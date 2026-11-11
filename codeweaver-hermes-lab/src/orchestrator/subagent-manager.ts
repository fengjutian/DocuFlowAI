import { randomUUID } from "node:crypto";
import type { Worktree } from "../code-engine/worktree-manager.js";
import type { AgentExecution, AgentTaskInput } from "../types/adapter.js";
import type { ExecutionCoordinator } from "./execution-coordinator.js";

/**
 * Parent/child executions (design 10 / Phase 4 multi-agent).
 *
 * A child is a full execution of its own: it has its own id, journal and events. What the manager adds
 * is lineage (which parent spawned it) and the worktree decision — isolated by default, shared only on
 * explicit request. Events stay attributed to the execution that produced them; `rootOf()` answers
 * which task tree they belong to.
 */
export type WorktreeMode = "isolated" | "shared";

export interface SpawnRequest {
  parentExecutionId: string;
  /** Child input; `workingDirectory` is decided by the manager unless worktree is "shared". */
  taskInput: AgentTaskInput;
  worktree?: WorktreeMode;
  /** Required for "isolated": the commit the child worktree is created at. */
  baseSha?: string;
  label?: string;
}

export interface SpawnedChild {
  execution: AgentExecution;
  worktreeMode: WorktreeMode;
  workingDirectory: string;
}

/** Structural subset of WorktreeManager, so tests can substitute a fake. */
export interface WorktreeProvider {
  create(baseSha: string, label?: string): Promise<Worktree>;
}

export class SubagentManager {
  private readonly parentOfChild = new Map<string, string>();
  private readonly childWorktrees = new Map<string, string>();

  constructor(
    private readonly deps: {
      coordinator: ExecutionCoordinator;
      worktrees?: WorktreeProvider;
    }
  ) {}

  async spawn(request: SpawnRequest): Promise<SpawnedChild> {
    const parentInput = this.deps.coordinator.getTaskInput(request.parentExecutionId);
    if (!parentInput) {
      throw new Error(`Parent execution ${request.parentExecutionId} is not tracked by this coordinator`);
    }
    if (request.taskInput.runId === parentInput.runId) {
      throw new Error("A child execution must use a different run id than its parent");
    }

    const mode: WorktreeMode = request.worktree ?? "isolated";
    let workingDirectory: string;
    if (mode === "shared") {
      workingDirectory = parentInput.workingDirectory;
    } else {
      if (!this.deps.worktrees) throw new Error("Isolated subagents need a worktree provider");
      if (!request.baseSha) throw new Error("Isolated subagents need the base SHA to branch from");
      const worktree = await this.deps.worktrees.create(request.baseSha, request.label ?? "subagent");
      workingDirectory = worktree.path;
      this.childWorktrees.set(request.taskInput.runId, worktree.path);
    }

    const childInput: AgentTaskInput = {
      ...request.taskInput,
      projectId: parentInput.projectId,
      workingDirectory
    };
    // The child runs where the parent ran. Delegating to a child is not a request to move the work
    // to another runtime: the caller named the runtime for this task, and a child that silently
    // landed on the server's default would run under a different policy, a different tool surface
    // and a different model (D-047, D-055).
    const parentAdapter = this.deps.coordinator.adapterFor(request.parentExecutionId);
    const execution = await this.deps.coordinator.start(childInput, parentAdapter);
    this.parentOfChild.set(execution.executionId, request.parentExecutionId);

    const runtimeId = this.deps.coordinator.runtimeIdOf(parentAdapter);
    await this.audit(request.parentExecutionId, "subagent.spawned", {
      childExecutionId: execution.executionId,
      childTaskId: childInput.taskId,
      worktreeMode: mode,
      ...(runtimeId ? { runtimeId } : {}),
      ...(this.childWorktrees.has(childInput.runId)
        ? { worktreePath: this.childWorktrees.get(childInput.runId) }
        : {})
    });
    await this.audit(execution.executionId, "subagent.parent", {
      parentExecutionId: request.parentExecutionId
    });

    return { execution, worktreeMode: mode, workingDirectory };
  }

  parentOf(executionId: string): string | undefined {
    return this.parentOfChild.get(executionId);
  }

  /** Walks up the lineage so any event can be attributed to the task tree that caused it. */
  rootOf(executionId: string): string {
    let current = executionId;
    const seen = new Set<string>();
    while (true) {
      const parent = this.parentOfChild.get(current);
      if (!parent) return current;
      if (seen.has(parent)) return parent;
      seen.add(current);
      current = parent;
    }
  }

  childrenOf(parentExecutionId: string): string[] {
    return [...this.parentOfChild.entries()]
      .filter(([, parent]) => parent === parentExecutionId)
      .map(([child]) => child)
      .sort();
  }

  lineageOf(executionId: string): string[] {
    const chain: string[] = [executionId];
    let current = executionId;
    while (true) {
      const parent = this.parentOfChild.get(current);
      if (!parent) return chain;
      chain.push(parent);
      current = parent;
    }
  }

  private audit(executionId: string, category: string, payload: Record<string, unknown>): Promise<void> {
    // Goes through the coordinator so journal writes stay serialized with runtime events.
    return this.deps.coordinator.recordAuditFact(executionId, category, payload);
  }
}

export function newChildRunId(): string {
  return `run_child_${randomUUID()}`;
}
