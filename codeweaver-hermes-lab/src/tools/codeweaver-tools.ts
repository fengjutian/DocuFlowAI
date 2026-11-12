import { readFile } from "node:fs/promises";
import { resolve as resolvePath } from "node:path";
import { evaluatePermission, isWithin, type PolicySnapshot } from "../policy/permission-policy.js";
import { scopeViolationFor, type WorktreeScope } from "../policy/worktree-scope.js";
import type { ChangeProposal } from "../code-engine/diff-analysis.js";
import type { ApplyOutcome } from "../code-engine/code-change-pipeline.js";
import type { EvaluationCheckResult, EvaluationCheckSpec, EvaluationResult } from "../evaluation/evaluation.js";

/**
 * The tool surface CodeWeaver exposes to a runtime (design 7.2).
 *
 * A runtime may read, propose a diff and ask for a diff to be applied — but every write goes
 * through the Code Engine, and every read is bounded by the same policy snapshot that governs the
 * execution. The runtime never touches the filesystem or the shared checkout itself.
 *
 * The tool shape is Pi's: `{ name, description, parameters, execute }`. Hermes reaches the same
 * functions through MCP, so both runtimes share one implementation of the rules.
 */

export interface CodeWeaverToolContext {
  /** The authorized worktree. Reads and writes are bounded by it. */
  worktreePath: string;
  /**
   * The policy snapshot governing this execution.
   *
   * Resolved lazily because the snapshot record is written after the run starts, and the tool must
   * see the same snapshot the execution runs under — not a guess and not a stale one.
   */
  policy: () => PolicySnapshot | Promise<PolicySnapshot>;
  /**
   * The worktree scope of this execution: its own directory and the repository's worktrees.
   *
   * Resolved on every call, not once: a worktree can appear while the execution runs — a delegated
   * child gets its own — and a scope that does not see it cannot keep the execution out of it.
   */
  worktreeScope?: () => Promise<WorktreeScope>;
  /** Proposes a diff without touching the worktree. */
  propose: (patch: string) => ChangeProposal;
  /** Applies a patch through the Code Engine. */
  apply: (patch: string, options?: { commitMessage?: string }) => Promise<ApplyOutcome>;
  /**
   * The acceptance checks this execution was started with.
   *
   * The checks are declared by the task, never by the model: a runtime that could name its own
   * command would be grading itself. The model may narrow the set, not extend it.
   */
  evaluationChecks: () => readonly EvaluationCheckSpec[];
  /** Runs independent evaluation checks. */
  evaluate: (checks: readonly EvaluationCheckSpec[]) => Promise<EvaluationResult>;
  now?: () => Date;
}

/** The result shape Pi expects from a tool. */
export interface ToolResult {
  content: Array<{ type: "text"; text: string }>;
  details: Record<string, unknown>;
}

export interface CodeWeaverTool {
  name: string;
  label: string;
  description: string;
  parameters: Record<string, unknown>;
  execute: (
    toolCallId: string,
    args: Record<string, unknown>,
    signal?: AbortSignal,
    onUpdate?: (update: unknown) => void
  ) => Promise<ToolResult>;
}

export const CODEWEAVER_TOOL_NAMES = [
  "read_file",
  "request_diff",
  "request_apply_diff",
  "evaluate_changes"
] as const;

/**
 * Builds the tool set for one execution.
 *
 * The tools are bound to a single execution's policy snapshot and worktree: a tool set is not a
 * global capability, it is the capability this execution was granted.
 */
export function createCodeWeaverTools(context: CodeWeaverToolContext): CodeWeaverTool[] {
  const now = context.now ?? (() => new Date());
  return [
    createReadFileTool(context, now),
    createRequestDiffTool(context),
    createRequestApplyDiffTool(context),
    createEvaluateChangesTool(context)
  ];
}

function createReadFileTool(context: CodeWeaverToolContext, now: () => Date): CodeWeaverTool {
  return {
    name: "read_file",
    label: "Read file",
    description:
      "Read a file from the authorized worktree. Paths outside the worktree or outside the policy's " +
      "allowed paths are denied; the denial is enforced here, not requested from the model.",
    parameters: {
      type: "object",
      properties: {
        path: { type: "string", description: "Absolute path, or a path relative to the worktree." }
      },
      required: ["path"]
    },
    execute: async (_toolCallId, args) => {
      const requested = typeof args["path"] === "string" ? args["path"] : "";
      if (!requested) {
        return textResult("read_file needs a path", { denied: true, reason: "missing_path" });
      }
      const target = resolvePath(context.worktreePath, requested);
      // The worktree boundary is checked here and not left to the policy engine: a read is a P0
      // operation, and P0 is allowed by default — only the path scope keeps it inside the sandbox.
      if (!isWithin(target, context.worktreePath)) {
        return textResult(`read_file denied: '${target}' is outside the authorized worktree`, {
          denied: true,
          reason: "outside_worktree",
          path: target
        });
      }
      // A directory is not a worktree. Another worktree can sit inside this one — a child agent's
      // worktree under the checkout the execution runs in — and then the boundary above would let
      // the read straight into it. The scope asks which worktree owns the path, and the answer has
      // to be this execution's own worktree.
      if (context.worktreeScope) {
        let scope: WorktreeScope;
        try {
          scope = await context.worktreeScope();
        } catch (error) {
          // A scope that cannot be established is not a scope with nothing in it.
          return textResult(
            `read_file denied: the worktree scope could not be verified (${
              error instanceof Error ? error.message : String(error)
            })`,
            { denied: true, reason: "scope_unverifiable", path: target }
          );
        }
        const violation = await scopeViolationFor(target, scope);
        if (violation) {
          return textResult(`read_file denied: ${violation}`, {
            denied: true,
            reason: "outside_worktree_scope",
            path: target
          });
        }
      }
      const evaluation = evaluatePermission(
        { operation: "read_file", paths: [target], worktreePath: context.worktreePath },
        await context.policy(),
        now()
      );
      if (evaluation.decision !== "allow") {
        return textResult(`read_file denied: ${evaluation.reason}`, {
          denied: true,
          reason: evaluation.reason,
          level: evaluation.level
        });
      }
      try {
        const content = await readFile(target, "utf8");
        return textResult(content, { path: target, bytes: content.length });
      } catch (error) {
        return textResult(`read_file failed: ${error instanceof Error ? error.message : String(error)}`, {
          path: target,
          error: true
        });
      }
    }
  };
}

function createRequestDiffTool(context: CodeWeaverToolContext): CodeWeaverTool {
  return {
    name: "request_diff",
    label: "Request diff",
    description:
      "Propose a unified diff and inspect it before applying. Nothing is written to the worktree; the " +
      "result names every file the diff touches and every risk it carries.",
    parameters: {
      type: "object",
      properties: {
        patch: { type: "string", description: "A unified diff." }
      },
      required: ["patch"]
    },
    execute: async (_toolCallId, args) => {
      const patch = typeof args["patch"] === "string" ? args["patch"] : "";
      if (!patch) {
        return textResult("request_diff needs a patch", { denied: true, reason: "missing_patch" });
      }
      const proposal = context.propose(patch);
      return textResult(renderProposal(proposal), {
        diffHash: proposal.diffHash,
        files: proposal.files,
        requiresApproval: proposal.requiresApproval,
        risks: proposal.risks.map(risk => ({ code: risk.code, message: risk.message, blocked: risk.blocked }))
      });
    }
  };
}

function createRequestApplyDiffTool(context: CodeWeaverToolContext): CodeWeaverTool {
  return {
    name: "request_apply_diff",
    label: "Request apply diff",
    description:
      "Apply a unified diff to the authorized worktree through the Code Engine. The Code Engine checks " +
      "the path scope, the base SHA and the policy before writing, and it either applies the change " +
      "atomically or leaves the worktree byte-identical.",
    parameters: {
      type: "object",
      properties: {
        patch: { type: "string", description: "A unified diff." },
        commitMessage: { type: "string", description: "Optional commit message for the applied change." }
      },
      required: ["patch"]
    },
    execute: async (_toolCallId, args) => {
      const patch = typeof args["patch"] === "string" ? args["patch"] : "";
      if (!patch) {
        return textResult("request_apply_diff needs a patch", { denied: true, reason: "missing_patch" });
      }
      const commitMessage = typeof args["commitMessage"] === "string" ? args["commitMessage"] : undefined;
      const outcome = await context.apply(patch, commitMessage ? { commitMessage } : {});
      if (outcome.status === "applied") {
        return textResult(
          `Applied ${outcome.proposal.files.length} file(s) as change ${outcome.record.id}` +
            (outcome.commitSha ? ` (commit ${outcome.commitSha})` : ""),
          {
            applied: true,
            changeRecordId: outcome.record.id,
            diffHash: outcome.record.diffHash,
            ...(outcome.commitSha ? { commitSha: outcome.commitSha } : {})
          }
        );
      }
      if (outcome.status === "awaiting_approval") {
        return textResult(`The change needs human approval: ${outcome.reasons.join("; ")}`, {
          applied: false,
          awaitingApproval: true,
          reasons: outcome.reasons
        });
      }
      return textResult(`The change was denied: ${outcome.reasons.join("; ")}`, {
        applied: false,
        denied: true,
        reasons: outcome.reasons
      });
    }
  };
}

/**
 * Runs the acceptance checks the task was started with.
 *
 * The checks are independent processes (AT-204): the runtime cannot call itself "done", only the
 * checks can. The model may select a subset of the declared checks, but it cannot introduce one.
 */
function createEvaluateChangesTool(context: CodeWeaverToolContext): CodeWeaverTool {
  return {
    name: "evaluate_changes",
    label: "Evaluate changes",
    description:
      "Run the acceptance checks this task was started with. The verdict comes from independent " +
      "commands, not from the agent's own claim; a failed or blocked check does not count as done.",
    parameters: {
      type: "object",
      properties: {
        checkIds: {
          type: "array",
          items: { type: "string" },
          description: "Optional subset of the declared check ids to run."
        }
      }
    },
    execute: async (_toolCallId, args) => {
      const declared = context.evaluationChecks();
      if (declared.length === 0) {
        return textResult("This task declares no acceptance checks, so there is no evidence to collect", {
          verdict: "blocked",
          reason: "no_checks_declared"
        });
      }
      const requested = Array.isArray(args["checkIds"])
        ? (args["checkIds"] as unknown[]).filter((item): item is string => typeof item === "string")
        : [];
      const unknown = requested.filter(id => !declared.some(check => check.id === id));
      if (unknown.length > 0) {
        return textResult(`Unknown check id(s): ${unknown.join(", ")}`, {
          denied: true,
          reason: "unknown_check",
          known: declared.map(check => check.id)
        });
      }
      const selected = requested.length > 0
        ? declared.filter(check => requested.includes(check.id))
        : [...declared];
      const result = await context.evaluate(selected);
      // The agent has to be told why a check did not run: a verdict without the reason looks like
      // a flaky check, and the model cannot answer the task around a failure it cannot see.
      //
      // Four kinds of check need their reason spelled out, and each of them carries it in the
      // output: a refusal (D-075), a resource limit that stopped the check, a check that could
      // not run at all, and a plain failure. A check that ran out of memory and a check that
      // simply failed are different facts, and only one of them tells the agent what to change.
      const explanations = result.checks
        .filter(check => check.refusal || check.limitHit || check.verdict !== "passed")
        .map(check => `${check.name}: ${explainCheck(check)}`);
      const text =
        explanations.length > 0
          ? `${result.verdict}: ${result.summary}\n${explanations.join("\n")}`
          : `${result.verdict}: ${result.summary}`;
      return textResult(text, {
        verdict: result.verdict,
        evaluationId: result.id,
        checks: result.checks.map(check => ({
          id: check.id,
          name: check.name,
          verdict: check.verdict,
          exitCode: check.exitCode,
          output: check.output,
          ...(check.refusal ? { refusal: check.refusal } : {}),
          ...(check.approval
            ? { approval: { approvedBy: check.approval.approvedBy, reason: check.approval.reason } }
            : {})
        }))
      });
    }
  };
}

function renderProposal(proposal: ChangeProposal): string {
  const lines = [
    `Diff ${proposal.diffHash}`,
    `Files: ${proposal.files.length ? proposal.files.join(", ") : "(none)"}`,
    `Requires approval: ${proposal.requiresApproval ? "yes" : "no"}`
  ];
  for (const risk of proposal.risks) {
    lines.push(`- [${risk.blocked ? "blocked" : "approval"}] ${risk.message}`);
  }
  return lines.join("\n");
}

/**
 * The reason one check did not pass, in the words the agent will read.
 *
 * The sandbox note is the line that names the limit, and it sits at the end of the output, so it
 * is pulled out on its own: a model that reads "the check failed" learns nothing, and one that
 * reads "the check exceeded the memory limit of 128 MB" knows what to change.
 */
function explainCheck(check: EvaluationCheckResult): string {
  const sandboxNote = /^\[sandbox\].*$/m.exec(check.output)?.[0];
  if (sandboxNote) return sandboxNote;
  const output = check.output.trim();
  if (!output) return `the check ${check.verdict}`;
  return output.length <= 400 ? output : `${output.slice(0, 400)}...`;
}

function textResult(text: string, details: Record<string, unknown>): ToolResult {
  return { content: [{ type: "text", text }], details };
}

/** True when a path is inside the worktree and inside the policy's allowed paths. */
export async function isPathAuthorized(context: CodeWeaverToolContext, path: string): Promise<boolean> {
  const target = resolvePath(context.worktreePath, path);
  if (!isWithin(target, context.worktreePath)) return false;
  const policy = await context.policy();
  return policy.allowedPaths.some(allowed => isWithin(target, allowed));
}
