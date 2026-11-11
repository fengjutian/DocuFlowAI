import test from "node:test";
import assert from "node:assert/strict";
import {
  executePiSubagentPhase0,
  type PiSubagentClient,
  type SpawnOutcome,
  type SubagentLineage
} from "../../runners/pi-subagent-phase0.js";

/**
 * The delegation probe is judged on what the server actually did, so these tests drive that
 * judgement with a fake client: a delegation that worked, one that never happened, and the ones
 * that look fine until the lineage or the runtime is checked.
 */

const PARENT_ID = "exec-parent";
const CHILD_ID = "exec-child";

interface FakeOptions {
  /** The status of the delegation request; a non-201 means no child was created. */
  spawnStatus?: number;
  spawnError?: string;
  /** The child's terminal status; a non-terminal status means the child never finished. */
  childStatus?: string;
  parentRuntimeId?: string;
  childRuntimeId?: string;
  /** `null` means the fact is missing from the journal. */
  spawnedFact?: Record<string, unknown> | null;
  /** `null` means the fact is missing from the journal. */
  parentFact?: Record<string, unknown> | null;
  /** The chain the server reports for the child. */
  childLineage?: string[];
  /** The root the server resolves for the child. */
  childRootExecutionId?: string;
  /** The children the server lists for the parent. */
  parentChildren?: string[];
  /** Whether the parent produced output at all. */
  streaming?: boolean;
}

function client(options: FakeOptions = {}): PiSubagentClient {
  const spawnedFact =
    options.spawnedFact === undefined
      ? { childExecutionId: CHILD_ID, childTaskId: "task-child", worktreeMode: "isolated" }
      : options.spawnedFact;
  const parentFact =
    options.parentFact === undefined ? { parentExecutionId: PARENT_ID } : options.parentFact;

  const spawn: SpawnOutcome =
    options.spawnStatus !== undefined && options.spawnStatus !== 201
      ? {
          status: options.spawnStatus,
          ...(options.spawnError ? { error: options.spawnError } : {})
        }
      : {
          status: 201,
          childExecutionId: CHILD_ID,
          worktreeMode: "isolated",
          workingDirectory: "/tmp/subagent-child",
          rootExecutionId: PARENT_ID,
          lineage: [CHILD_ID, PARENT_ID]
        };

  const facts: Record<string, Record<string, Record<string, unknown> | undefined>> = {
    [PARENT_ID]: { "subagent.spawned": spawnedFact ?? undefined },
    [CHILD_ID]: { "subagent.parent": parentFact ?? undefined }
  };

  return {
    startParent: async () => PARENT_ID,
    waitForOutput: async () => options.streaming !== false,
    status: async () => options.childStatus ?? "succeeded",
    waitForTerminal: async () => options.childStatus ?? "succeeded",
    runtimeId: async executionId =>
      executionId === PARENT_ID
        ? options.parentRuntimeId ?? "pi-local"
        : options.childRuntimeId ?? "pi-local",
    spawnChild: async () => spawn,
    fact: async (executionId, category) => facts[executionId]?.[category],
    lineage: async (executionId): Promise<SubagentLineage> => {
      if (executionId === CHILD_ID) {
        return {
          children: [],
          lineage: options.childLineage ?? [CHILD_ID, PARENT_ID],
          rootExecutionId: options.childRootExecutionId ?? PARENT_ID
        };
      }
      return {
        children: options.parentChildren ?? [CHILD_ID],
        lineage: [PARENT_ID],
        rootExecutionId: PARENT_ID
      };
    }
  };
}

test("passes when the child ran on the parent's runtime and both sides are journaled", async () => {
  const results = await executePiSubagentPhase0(client());

  assert.equal(results.length, 1);
  assert.equal(results[0]!.id, "P0-008");
  assert.equal(results[0]!.status, "passed");
  assert.ok(results[0]!.evidence.some(item => item.includes("parentRuntimeId=pi-local")));
  assert.ok(results[0]!.evidence.some(item => item.includes("childRuntimeId=pi-local")));
  assert.ok(results[0]!.evidence.some(item => item.includes("childStatus=succeeded")));
  assert.ok(results[0]!.evidence.some(item => item.includes("childRootExecutionId=exec-parent")));
});

test("fails when the parent's journal has no 'subagent.spawned' fact", async () => {
  const results = await executePiSubagentPhase0(client({ spawnedFact: null }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /no 'subagent\.spawned' fact/);
});

test("fails when the child's journal has no 'subagent.parent' fact", async () => {
  const results = await executePiSubagentPhase0(client({ parentFact: null }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /no 'subagent\.parent' fact/);
});

test("fails when the child never reached a terminal state", async () => {
  const results = await executePiSubagentPhase0(client({ childStatus: "running" }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /never reached a terminal state/);
});

test("fails when the child ran on another runtime than its parent", async () => {
  const results = await executePiSubagentPhase0(client({ childRuntimeId: "hermes-local" }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /instead of the parent's 'pi-local'/);
});

test("fails when the lineage does not reach the parent", async () => {
  const results = await executePiSubagentPhase0(client({ childLineage: [CHILD_ID] }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /lineage does not reach the parent/);
});

test("fails when the lineage resolves to the wrong root", async () => {
  const results = await executePiSubagentPhase0(client({ childRootExecutionId: "exec-other" }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /resolves to root 'exec-other'/);
});

test("fails when the parent does not list the child among its children", async () => {
  const results = await executePiSubagentPhase0(client({ parentChildren: [] }));

  assert.equal(results[0]!.status, "failed");
  assert.match(results[0]!.notes ?? "", /does not list the child/);
});

test("is blocked when the server never created a child", async () => {
  const results = await executePiSubagentPhase0(
    client({ spawnStatus: 500, spawnError: "runtime unreachable" })
  );

  assert.equal(results[0]!.status, "blocked");
  assert.match(results[0]!.notes ?? "", /HTTP 500 — runtime unreachable/);
  assert.ok(results[0]!.evidence.some(item => item.includes("spawnStatus=500")));
});

test("is blocked when the parent never produced any output", async () => {
  const results = await executePiSubagentPhase0(client({ streaming: false }));

  assert.equal(results[0]!.status, "blocked");
  assert.match(results[0]!.notes ?? "", /never produced output/);
});
