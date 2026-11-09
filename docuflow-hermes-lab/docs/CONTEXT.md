# Context snapshots (design 9.3)

`src/context/context-snapshot.ts` captures the authoritative context a run was given, and can prove
later whether that context still holds.

## What is captured

| Field | Source |
|---|---|
| `git_base_sha` | `git rev-parse HEAD` in the worktree |
| `document_versions` | content hash (`sha256:<hex>`) of every authoritative document |
| `selected_files` | the file list the agent is expected to read |
| `architecture_version`, `gitnexus_index_version` | supplied versions |
| `context_hash` | canonical hash over all of the above plus each selected file's content |

`context_hash` is computed from JSON with sorted keys and sorted file lists, so ordering never
changes the hash. The file contents themselves are hashed and discarded — the snapshot records what
the run saw without duplicating the repository.

## Why every path is re-read

`captureContextSnapshot` resolves each path inside the worktree and re-reads it before hashing. A
caller cannot claim "this file is unchanged"; the hash is computed from bytes on disk. A path that
escapes the worktree (`../outside.md`) is rejected before any read.

## Verification

```ts
verifyContextSnapshot(record, { worktreePath, git, documents, selectedFiles })
// → { matches, actualHash, differences: ["document changed: spec.md", ...] }
```

Use it before resuming or rebuilding: a context that drifted must be a deliberate decision, not a
surprise. The hash mismatch is the signal; `differences` explains which document moved.

## Limits

- Only L1–L3 material that the caller names is captured; GitNexus index contents are recorded by
  version, not embedded.
- Files are read whole; a very large selected file is fully hashed in memory.
- `executionId` is optional because design 11.7 has no execution column — the execution link is the
  prompt snapshot, as in the traceability query.
