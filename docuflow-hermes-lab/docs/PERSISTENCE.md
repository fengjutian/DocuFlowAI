# Persistence and traceability

Implements the record shapes from design 11.6–11.9 and the queries from section 15, on top of a
file-backed, immutable store.

## Records

| Type | Mirrors | Notes |
|---|---|---|
| `ContextSnapshotRecord` | 11.7 | git base SHA, document versions, selected files, context hash |
| `PromptSnapshotRecord` | 11.8 | template version + hashes; **never** the rendered prompt |
| `PolicySnapshotRecord` | 11.6 | granted/denied capabilities, allowed paths, expiry |
| `ChangeRecord` | 11.9 | already produced by the Code Engine pipeline |

`toPolicySnapshotRecord()` converts the evaluator's `PolicySnapshot` into the persisted row without
losing the snapshot id.

## Store guarantees

`ImmutableRecordStore` writes one JSON file per record with `wx`:

- an existing id is a `RecordConflictError`, never an overwrite;
- ids must be safe file names, checked before any disk access;
- an optional validator runs first, so a malformed snapshot is rejected at the boundary;
- `list()` returns `[]` for a collection that was never created.

`assertPromptSnapshot()` rejects a record carrying `renderedPrompt`, because design 11.8 stores that
field encrypted or by hash only.

## Traceability queries

| Question (design 15) | Call |
|---|---|
| commit → agent run | `byCommit(sha)` |
| execution → changes, prompts, policies, contexts | `byExecution(id)` |
| task → all executions | `byTask(id)` |

Context snapshots carry no `execution_id` (design 11.7), so `byExecution` resolves them through the
prompt snapshots that consumed them — the same link the schema defines.

## Limits

- No database yet: `list()` scans the collection directory, which is fine for a lab and wrong for a
  growing production history. The `RecordStore` interface is the seam where SQLite/Postgres lands.
- No encryption: prompt text is not stored at all, which sidesteps the problem instead of solving it.
- No retention or deletion policy; deleting evidence is deliberately not implemented.
