import type { ApplyOutcome } from "../code-engine/code-change-pipeline.js";
import type { ChangeProposal } from "../code-engine/diff-analysis.js";
import type { EvaluationCheckSpec, EvaluationResult } from "../evaluation/evaluation.js";
import type { PolicySnapshot } from "../policy/permission-policy.js";
import type { WorktreeScope } from "../policy/worktree-scope.js";
import type { AgentTaskInput } from "../types/adapter.js";
import { createCodeWeaverTools, type CodeWeaverTool, type CodeWeaverToolContext } from "./codeweaver-tools.js";

/**
 * Binds the CodeWeaver tool surface to one execution.
 *
 * The tools are not a global capability: they are the capability one execution was granted, bound to
 * that execution's worktree, policy snapshot and acceptance checks. This binder is the only place
 * that knows how a runtime's tool call reaches the Code Engine and the Evaluation Engine, so both
 * runtimes share one implementation of the rules.
 */

/**
 * The part of the task service the tool surface needs.
 *
 * It is structural so the binding can be tested without the whole service, and so the service stays
 * the only owner of the Code Engine and the stores.
 */
export interface ExecutionToolService {
  /** The policy snapshot an execution runs under, reconstructed for the policy engine. */
  policyFor(executionId: string): Promise<PolicySnapshot>;
  /** The worktree scope of an execution: its own directory and the repository's worktrees. */
  worktreeScope(worktreePath: string): Promise<WorktreeScope>;
  /** The acceptance checks the execution was started with. */
  evaluationChecksFor(executionId: string): readonly EvaluationCheckSpec[];
  /** Applies a proposed patch through the Code Engine. */
  applyProposedChange(
    executionId: string,
    patch: string,
    options?: { commitMessage?: string }
  ): Promise<ApplyOutcome>;
  /** Runs independent evaluation checks. */
  evaluateChange(
    executionId: string,
    checks: readonly EvaluationCheckSpec[]
  ): Promise<EvaluationResult>;
}

export interface CodeWeaverToolBinderOptions {
  service: ExecutionToolService;
  /** Non-mutating diff inspection, used by `request_diff`. */
  propose: (patch: string) => ChangeProposal;
  now?: () => Date;
}

export class CodeWeaverToolBinder {
  constructor(private readonly options: CodeWeaverToolBinderOptions) {}

  /**
   * The tool set for one execution.
   *
   * The execution id is part of the binding because the policy snapshot, the worktree and the
   * acceptance checks all belong to the execution, not to the task input that started it.
   *
   * Only the tools the execution is allowed to use are exposed. Hiding a tool is better than
   * exposing it and refusing every call: the model should not see a capability it cannot have.
   */
  bind(input: AgentTaskInput, executionId: string): CodeWeaverTool[] {
    const context: CodeWeaverToolContext = {
      worktreePath: input.workingDirectory,
      // Resolved on every call: the policy snapshot is written after the run starts, and the tool
      // must see the snapshot the execution actually runs under.
      policy: () => this.options.service.policyFor(executionId),
      // Resolved on every call: the worktrees of the repository change while an execution runs.
      worktreeScope: () => this.options.service.worktreeScope(input.workingDirectory),
      propose: patch => this.options.propose(patch),
      apply: (patch, applyOptions) =>
        this.options.service.applyProposedChange(executionId, patch, applyOptions ?? {}),
      evaluationChecks: () => this.options.service.evaluationChecksFor(executionId),
      evaluate: checks => this.options.service.evaluateChange(executionId, checks),
      ...(this.options.now ? { now: this.options.now } : {})
    };
    const allowed = new Set(input.allowedTools);
    return createCodeWeaverTools(context).filter(tool => allowed.has(tool.name));
  }
}
