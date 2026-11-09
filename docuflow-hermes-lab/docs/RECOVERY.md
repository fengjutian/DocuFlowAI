# Execution journal, checkpoints and recovery

Implements design 14.3 and 15: an append-only record of what happened, a checkpoint that can be
handed to a fresh execution, and a recovery decision that is not "call the model again".

## Journal

`src/journal/journal.ts` defines `JournalEntry` and two stores: `InMemoryJournalStore` for tests and
`JsonlJournalStore`, one append-only file per execution.

```ts
{ executionId, sequence, kind: "runtime.event" | "audit", category, recordedAt, payload }
```

Rules:

- **Append-only.** There is no update or delete. A correction is a new entry.
- **Sequence is unique, not necessarily contiguous.** Gaps are the signal compensation acts on.
- **A duplicate sequence is a conflict**, not an overwrite.
- **A malformed line is an error with its line number**, never silently skipped.
- Execution ids must be safe as file names; `../escape` is rejected before any write.
- `sequence` reuses the runtime event sequence, so journal order and runtime order are the same.

`recordRuntimeEvent()` maps an `AgentRuntimeEvent` into the journal; `recordAudit()` records a
DocuFlow fact (change applied, policy denied) in the same timeline.

## Checkpoint (design 14.3)

`buildCheckpoint()` produces: last sequence, event count, terminal state and the terminal event,
last event type, context/policy snapshot ids, base SHA, worktree id, external session id, applied
change record ids, unresolved permission request ids, and detected gaps.

## Recovery decision

`planRecovery()` returns one of four actions:

| Action | Condition | Caller behavior |
|---|---|---|
| `stop` | the execution is already terminal | record it, do nothing |
| `compensate` | the journal has gaps | re-fetch events from `fromSequence`, merge with `mergeCompensated()` |
| `resume` | the runtime advertises `sessionResume` **and** this session is still alive | continue from `lastSequence + 1` |
| `rebuild` | otherwise | create a new execution, inject the checkpoint, never re-apply an applied diff |

Hermes over HTTP has no resume, so it always lands on `rebuild` — the same conclusion as
`HermesAdapter.resume()`, now expressed as a decision instead of an exception.

`mergeCompensated()` deduplicates by sequence and throws on conflicting content, so a re-fetch can
fill gaps but cannot rewrite what was already recorded.

## Not implemented yet

- Durable storage for change records, policy snapshots and prompt snapshots (they are still objects).
- Automatic scheduling of recovery actions; the planner returns a decision, the orchestrator acts.
- Checkpoint injection into a rebuilt execution is a prompt/context concern and belongs to the
  Context Engine.
