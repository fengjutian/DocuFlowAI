import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { evaluatePermission, type PolicySnapshot } from "../policy/permission-policy.js";
import { buildApplyRecord, type ApprovalRecord, type ChangeRecord } from "./change-record.js";
import { proposeChange, type ChangeProposal } from "./diff-analysis.js";
import { gitOrThrow, type GitRunner } from "./git-runner.js";
import { WorktreeManager, isInside } from "./worktree-manager.js";

/**
 * Design 7.3 execution order: policy check → path check → base SHA check → diff + risk → approval →
 * atomic apply → change record. The pipeline never applies a patch it has not first validated with
 * `git apply --check`, so a rejected patch leaves the worktree byte-identical (AT-203).
 */
export interface ApplyChangeRequest {
  projectId: string;
  taskId: string;
  executionId: string;
  worktreePath: string;
  expectedBaseSha: string;
  patch: string;
  policy: {
    snapshot: PolicySnapshot;
    /** Absolute directories the patch may touch. */
    allowedPaths: string[];
  };
  approval?: ApprovalRecord;
  commit?: { message: string; authorName?: string; authorEmail?: string };
}

export type ApplyOutcome =
  | { status: "applied"; proposal: ChangeProposal; record: ChangeRecord; commitSha?: string }
  | { status: "awaiting_approval"; proposal: ChangeProposal; reasons: string[] }
  | { status: "denied"; proposal: ChangeProposal; reasons: string[] };

export interface CodeChangePipelineOptions {
  now?: () => Date;
  recordIdFactory?: () => string;
}

export class CodeChangePipeline {
  private readonly now: () => Date;

  constructor(
    private readonly git: GitRunner,
    private readonly worktrees: WorktreeManager,
    private readonly options: CodeChangePipelineOptions = {}
  ) {
    this.now = options.now ?? (() => new Date());
  }

  /** Non-mutating inspection, used by `request_diff` before a caller decides to apply. */
  propose(patch: string): ChangeProposal {
    return proposeChange(patch);
  }

  async apply(request: ApplyChangeRequest): Promise<ApplyOutcome> {
    const proposal = this.propose(request.patch);

    const structural = proposal.risks.filter(risk => risk.blocked).map(risk => risk.message);
    if (structural.length > 0) return { status: "denied", proposal, reasons: structural };

    const targets = proposal.files.map(file => resolve(request.worktreePath, file));
    const outside = targets.filter(target =>
      !isInside(request.worktreePath, target) ||
      !request.policy.allowedPaths.some(root => isInside(root, target))
    );
    if (outside.length > 0) {
      return {
        status: "denied",
        proposal,
        reasons: outside.map(path => `Path '${path}' is outside the authorized worktree scope`)
      };
    }

    // AT-105: a drifted worktree must not receive a patch computed against a different tree.
    try {
      await this.worktrees.assertBaseSha(request.worktreePath, request.expectedBaseSha);
    } catch (error) {
      return { status: "denied", proposal, reasons: [errorMessage(error)] };
    }

    const evaluation = evaluatePermission(
      { operation: "modify_worktree", paths: targets, worktreePath: request.worktreePath },
      request.policy.snapshot,
      this.now()
    );
    if (evaluation.decision === "deny") {
      return { status: "denied", proposal, reasons: [evaluation.reason] };
    }

    const approvalReasons = [
      ...(proposal.requiresApproval ? proposal.risks.map(risk => risk.message) : []),
      ...(evaluation.decision === "require_approval" ? [evaluation.reason] : [])
    ];
    if (approvalReasons.length > 0 && !request.approval) {
      return { status: "awaiting_approval", proposal, reasons: approvalReasons };
    }

    const check = await this.git.run(["apply", "--check", "--whitespace=nowarn", "-"], {
      cwd: request.worktreePath,
      stdin: request.patch
    });
    if (check.code !== 0) {
      return {
        status: "denied",
        proposal,
        reasons: [`Patch does not apply cleanly: ${check.stderr.trim() || "git apply --check failed"}`]
      };
    }

    const applied = await this.git.run(["apply", "--whitespace=nowarn", "-"], {
      cwd: request.worktreePath,
      stdin: request.patch
    });
    if (applied.code !== 0) {
      // git apply is transactional, so no partial write is expected; surface it as a hard error.
      throw new Error(`git apply failed after a successful check: ${applied.stderr.trim()}`);
    }

    let commitSha: string | undefined;
    if (request.commit) {
      await gitOrThrow(this.git, ["add", "-A"], { cwd: request.worktreePath });
      await gitOrThrow(this.git, [
        "-c",
        `user.name=${request.commit.authorName ?? "CodeWeaver Code Engine"}`,
        "-c",
        `user.email=${request.commit.authorEmail ?? "code-engine@codeweaver.local"}`,
        "commit",
        "-m",
        request.commit.message
      ], { cwd: request.worktreePath });
      commitSha = (await gitOrThrow(this.git, ["rev-parse", "HEAD"], { cwd: request.worktreePath })).stdout.trim();
    }

    const record = buildApplyRecord({
      projectId: request.projectId,
      taskId: request.taskId,
      executionId: request.executionId,
      gitBaseSha: request.expectedBaseSha,
      diffHash: proposal.diffHash,
      ...(commitSha ? { gitCommitSha: commitSha } : {}),
      ...(request.approval ? { approval: request.approval } : {}),
      createdAt: this.now().toISOString(),
      idFactory: this.options.recordIdFactory ?? (() => `chg_${randomUUID()}`)
    });

    return { status: "applied", proposal, record, ...(commitSha ? { commitSha } : {}) };
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
