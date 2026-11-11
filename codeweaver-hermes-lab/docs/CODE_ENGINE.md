# Code Engine change pipeline (Phase 2 core)

Implements design 7.3: path check → base SHA check → diff + risk analysis → policy → approval →
atomic apply → change record. V1 uses Mode A (the agent proposes a patch; Code Engine applies it),
so the shared checkout is never the agent's write target.

## Modules

| File | Responsibility |
|---|---|
| `git-runner.ts` | git without a shell; patches travel over stdin, never a temp file |
| `worktree-manager.ts` | create/remove/list worktrees pinning a base SHA, and `assertBaseSha` |
| `diff-analysis.ts` | parse a unified diff, compute its hash, classify risk |
| `change-record.ts` | immutable `change_records` rows and `revert` linkage |
| `code-change-pipeline.ts` | orchestration and outcomes |

## Outcomes

| Outcome | When |
|---|---|
| `denied` | structural risk (path escape, absolute path, `.git/`), path outside the authorized scope, base SHA drift, policy deny, or `git apply --check` failure |
| `awaiting_approval` | risk that needs a human (`package.json`, `.env`, migrations, deletions, binary, oversized) or a policy decision of `require_approval`, with no approval record |
| `applied` | check passed, patch written, optional commit created, change record returned |

A `denied` outcome is produced before any write, and even the apply itself is guarded by
`git apply --check` first — a rejected patch leaves the worktree byte-identical (AT-203).

## Risk rules

Blocking: `path_escape`, `absolute_path`, `git_metadata`, `no_files`.

Approval: `secret_file`, `dependency_change`, `migration`, `deletion`, `binary_patch`,
`large_patch` (>200k characters by default).

## Base SHA binding (AT-105)

`assertBaseSha` compares the worktree `HEAD` with the snapshot's `git_base_sha` before every apply.
A worktree that moved, or a patch computed against a different tree, is rejected instead of being
force-applied.

## Change records

Records follow design 11.9 and are frozen. A rollback is a new record with `operation = "revert"`,
`reverts_change_record_id` and `reverts_commit_sha`; nothing rewrites the original (design 9.5).
`diff_content` is not stored in plaintext here — only `diff_hash`; encrypted retention belongs to
the persistence layer.

## Deliberate limits

- Patches are applied into an isolated worktree, not merged into a shared branch. Merge remains a P3
  action that needs approval.
- `git apply` is used rather than `git am`; agent-supplied commit messages are not trusted.
- Binary replacement is treated as an approval-required change rather than being applied blindly.
- No automatic `git revert` invocation yet; `buildRevertRecord` records the intent and the resulting
  commit, the caller performs the revert.
