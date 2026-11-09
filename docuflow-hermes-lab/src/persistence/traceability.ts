import type { ChangeRecord } from "../code-engine/change-record.js";
import type {
  ContextSnapshotRecord,
  PolicySnapshotRecord,
  PromptSnapshotRecord
} from "./records.js";
import type { RecordStore } from "./record-store.js";

export interface TraceStores {
  changeRecords: RecordStore<ChangeRecord>;
  contextSnapshots: RecordStore<ContextSnapshotRecord>;
  promptSnapshots: RecordStore<PromptSnapshotRecord>;
  policySnapshots: RecordStore<PolicySnapshotRecord>;
}

/**
 * Answers the traceability questions from design 15:
 * commit → agent run, execution → context/policy/prompt/diff, task → all executions.
 */
export interface ExecutionTrace {
  executionId: string;
  changeRecords: ChangeRecord[];
  contextSnapshots: ContextSnapshotRecord[];
  promptSnapshots: PromptSnapshotRecord[];
  policySnapshots: PolicySnapshotRecord[];
}

export interface CommitTrace {
  changeRecord: ChangeRecord;
  trace: ExecutionTrace;
}

export class TraceabilityQuery {
  constructor(private readonly stores: TraceStores) {}

  async byExecution(executionId: string): Promise<ExecutionTrace> {
    const [changes, prompts, policies] = await Promise.all([
      this.stores.changeRecords.list(),
      this.stores.promptSnapshots.list(),
      this.stores.policySnapshots.list()
    ]);
    const executionPrompts = prompts.filter(record => record.executionId === executionId);
    return {
      executionId,
      changeRecords: changes.filter(record => record.executionId === executionId),
      contextSnapshots: await this.resolveContexts(executionPrompts),
      promptSnapshots: executionPrompts,
      policySnapshots: policies.filter(record => record.executionId === executionId)
    };
  }

  /** Design 15: "从 Git Commit 反查对应 Agent Run". */
  async byCommit(commitSha: string): Promise<CommitTrace | undefined> {
    const wanted = commitSha.toLowerCase();
    const changes = await this.stores.changeRecords.list();
    const changeRecord = changes.find(record => record.gitCommitSha?.toLowerCase() === wanted);
    if (!changeRecord) return undefined;
    return { changeRecord, trace: await this.byExecution(changeRecord.executionId) };
  }

  async byTask(taskId: string): Promise<ExecutionTrace[]> {
    const [changes, prompts, policies] = await Promise.all([
      this.stores.changeRecords.list(),
      this.stores.promptSnapshots.list(),
      this.stores.policySnapshots.list()
    ]);
    const executionIds = new Set<string>([
      ...changes.filter(record => record.taskId === taskId).map(record => record.executionId),
      ...prompts.filter(record => record.taskId === taskId).map(record => record.executionId),
      ...policies.filter(record => record.taskId === taskId).map(record => record.executionId)
    ]);
    return Promise.all([...executionIds].sort().map(executionId => this.byExecution(executionId)));
  }

  /**
   * Context snapshots have no execution id in design 11.7; the link runs through the prompt
   * snapshot that consumed them.
   */
  private async resolveContexts(prompts: readonly PromptSnapshotRecord[]): Promise<ContextSnapshotRecord[]> {
    const ids = [...new Set(prompts.map(prompt => prompt.contextSnapshotId))];
    const contexts: ContextSnapshotRecord[] = [];
    for (const id of ids) {
      const record = await this.stores.contextSnapshots.get(id);
      if (record) contexts.push(record);
    }
    return contexts;
  }
}
