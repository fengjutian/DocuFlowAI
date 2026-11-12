/**
 * Wire types for the CodeWeaver lab server (`codeweaver-hermes-lab`), design V1.0 sections 12 and 13.
 *
 * These mirror the server responses exactly. The UI never invents a field the server does not send,
 * and every optional field is optional here for the same reason it is optional there.
 */

export interface RuntimeEntry {
  id: string;
  runtimeType: string;
  version: string | null;
  metadata: Record<string, unknown>;
}

export interface RuntimeHealth {
  id: string;
  runtimeType: string;
  healthy: boolean;
  checkedAt: string;
  latencyMs: number;
  error?: string;
}

/**
 * One scoring dimension of a runtime.
 *
 * A dimension with no evidence is `measured: false` and carries no weight: an unmeasured runtime
 * is not a bad runtime, it is an unmeasured one. `detail` says what was measured and why the
 * dimension is weighted the way it is.
 */
export interface RuntimeScoreDimension {
  key: string;
  weight: number;
  effectiveWeight: number;
  measured: boolean;
  /** 0..1, or null when the dimension had no evidence. */
  score: number | null;
  detail: string;
  raw: Record<string, unknown>;
}

/** The raw counters behind a runtime's score. Every number here comes from a record. */
export interface RuntimeMetrics {
  executions: number;
  succeeded: number;
  failed: number;
  cancelled: number;
  interrupted: number;
  unfinished: number;
  successRate: number | null;
  cancelRequests: number;
  cancelConfirmed: number;
  cancelConfirmationRate: number | null;
  toolEventsObserved: boolean;
  toolEventCount: number;
  executionsWithToolEvents: number;
  permissionEventsObserved: boolean;
  permissionEventCount: number;
  executionsWithPermissionEvents: number;
  messageDeltas: number;
  averageDurationMs: number | null;
  totalTokens: number;
  averageTokensPerExecution: number | null;
  tokensPerSuccessfulExecution: number | null;
  evaluations: number;
  evaluationsPassed: number;
  evaluationPassRate: number | null;
  declaredCapabilities: string[];
  observedCapabilities: string[];
  /** Declared true, but the records show no evidence of it. */
  declaredNotObserved: string[];
  /** Demonstrated in the records, but never declared by the runtime. */
  observedNotDeclared: string[];
  /** Declared true, put in play by the task, and demonstrated. */
  realizedCapabilities: string[];
}

export interface RuntimeScore {
  runtimeId: string;
  runtimeType: string;
  version?: string;
  /** 0..100, or null when no dimension had any evidence at all. */
  score: number | null;
  dimensions: RuntimeScoreDimension[];
  metrics: RuntimeMetrics;
  /** Dimensions that had no evidence and therefore carried no weight. */
  unmeasuredDimensions: string[];
}

export interface RuntimeComparisonRow {
  runtimeId: string;
  runtimeType: string;
  version?: string;
  score: number | null;
  successRate: number | null;
  cancelConfirmationRate: number | null;
  capabilityRealization: number | null;
  evaluationPassRate: number | null;
  averageDurationMs: number | null;
  tokensPerSuccessfulExecution: number | null;
  declaredCapabilities: string[];
  observedCapabilities: string[];
}

export interface RuntimeComparison {
  /** Highest score first; runtimes without a score come last. */
  rows: RuntimeComparisonRow[];
  lines: string[];
}

export interface RuntimeScoreReport {
  runtimes: RuntimeScore[];
  comparison: RuntimeComparison;
  /** Executions in the journal that named no runtime; counted, attributed to nobody. */
  unattributedExecutions: number;
  executionsInJournal: number;
}

export type AgentRuntimeType = "hermes" | "native" | "acp" | "external";

export type AgentExecutionStatus =
  | "queued"
  | "starting"
  | "running"
  | "waiting_approval"
  | "cancelling"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "interrupted";

export interface AgentExecution {
  executionId: string;
  runtimeType: AgentRuntimeType;
  status: AgentExecutionStatus;
  externalSessionId?: string;
  externalRunId?: string;
}

export interface ExecutionCheckpoint {
  executionId: string;
  lastSequence: number;
  eventCount: number;
  terminal: boolean;
  terminalEventType?: string;
  lastEventType?: string;
  appliedChangeRecordIds: string[];
  pendingPermissionRequestIds: string[];
  gaps: number[];
  createdAt: string;
  contextSnapshotId?: string;
  policySnapshotId?: string;
  gitBaseSha?: string;
  worktreeId?: string;
  externalSessionId?: string;
  currentStep?: string;
}

export interface CompletionGate {
  allowed: boolean;
  reason: string;
}

export interface RuntimePermissionRequest {
  requestId: string;
  executionId: string;
  payload: Record<string, unknown>;
  tool?: string;
}

/**
 * One execution as the server's journal knows it.
 *
 * The live stream cannot tell the console about executions that ran while the console was not
 * listening, so the journal publishes this list instead: after a reconnect the snapshot is what
 * puts the missed executions back on screen.
 */
export interface ExecutionSummaryRecord {
  executionId: string;
  taskId?: string;
  projectId?: string;
  runtimeId?: string;
  status: string;
  eventCount: number;
  firstRecordedAt?: string;
  lastRecordedAt?: string;
}

export interface ExecutionDetail {
  executionId: string;
  /** The registered runtime that ran this execution, when the server can name it. */
  runtimeId?: string;
  /**
   * Whether this server process still holds the live session.
   *
   * A recorded execution can outlive the process that ran it: the journal survives a restart, so
   * the timeline and the final status are still readable while nothing about it runs any more.
   */
  live?: boolean;
  status: string;
  checkpoint: ExecutionCheckpoint;
  pendingApprovals: RuntimePermissionRequest[];
  completion: CompletionGate;
}

/** Unified event envelope (design 13). `data` is the runtime payload, kept loose on purpose. */
export interface CodeWeaverEvent<T = Record<string, unknown>> {
  id: string;
  type: string;
  projectId: string;
  taskId?: string;
  executionId?: string;
  sequence: number;
  timestamp: string;
  data: T;
}

export interface StartTaskResponse {
  execution: AgentExecution;
  contextSnapshotId: string;
  policySnapshotId: string;
  promptSnapshotId: string;
}

export interface PolicySnapshotInput {
  snapshotId: string;
  projectId: string;
  taskId: string;
  executionId: string;
  worktreeId: string;
  policyVersion: string;
  grantedCapabilities: string[];
  allowedPaths: string[];
  autoApprovedOperations?: string[];
  expiresAt: string;
}

export interface TemplateInput {
  name: string;
  version: string;
  contentHash: string;
}

export interface StartTaskInput {
  projectId: string;
  prompt: string;
  worktreePath: string;
  documents: string[];
  selectedFiles: string[];
  architectureVersion: string;
  policy: PolicySnapshotInput;
  template: TemplateInput;
  modelConfig?: Record<string, unknown>;
  correlationId?: string;
  timeoutSeconds?: number;
  allowedTools?: string[];
  skills?: Array<{ skillId: string; version?: string }>;
  approvedSkills?: string[];
  /**
   * The registered runtime that runs this task.
   *
   * `auto` lets the server pick a healthy runtime; omitting it uses the server's default one.
   * Naming a runtime is honoured or refused, never quietly redirected (D-047).
   */
  runtimeId?: string;
  /**
   * The independent checks that decide whether the task passed.
   *
   * Without them the completion gate has nothing to decide on: a run that ends with a model
   * saying "done" is not a finished task, and the gate says so.
   */
  evaluationChecks?: Array<{
    id: string;
    name: string;
    command: string;
    args: string[];
    cwd?: string;
  }>;
}

export interface RecoveryPlan {
  action: "stop" | "compensate" | "resume" | "rebuild";
  reason: string;
  fromSequence: number;
  checkpoint: ExecutionCheckpoint;
}

export interface RetryResponse {
  previousExecutionId: string;
  /**
   * The new execution, when the recovery plan actually created one.
   *
   * A terminal execution has nothing to recover: the plan says `stop` and no execution is created.
   */
  execution?: AgentExecution;
  plan: RecoveryPlan;
}

export interface ResumeResponse {
  execution?: AgentExecution;
  plan: RecoveryPlan;
}

export interface UsageRecord {
  id: string;
  executionId: string;
  model: string;
  promptTokens: number;
  completionTokens: number;
  /** Tokens served from the provider's cache, when the runtime reported them separately. */
  cachedTokens?: number;
  /** Tokens written into the provider's cache, when the runtime reported them separately. */
  cacheWriteTokens?: number;
  totalTokens: number;
  durationMs: number;
  startedAt?: string;
  finishedAt?: string;
}

export interface UsageSummary {
  executions: number;
  promptTokens: number;
  completionTokens: number;
  cachedTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  durationMs: number;
  byModel: Array<{ model: string; executions: number; totalTokens: number }>;
}

export type SkillKind = "platform" | "engineering" | "hermes-native";
export type RiskLevel = "low" | "medium" | "high";
export type SkillApprovalPolicy = "auto" | "require_approval";

export interface SkillManifest {
  skillId: string;
  version: string;
  name: string;
  description?: string;
  kind: SkillKind;
  permissions: string[];
  allowedRuntimes: string[];
  riskLevel: RiskLevel;
  approvalPolicy: SkillApprovalPolicy;
  /**
   * The skill body: the Markdown that is injected into the prompt of a run that binds it.
   *
   * It is part of the manifest because it is what the checksum covers, and the checksum is only
   * evidence when there is something to check it against.
   */
  content: string;
  checksum: string;
  createdAt: string;
  deprecated?: boolean;
}

/**
 * A skill bound to one execution.
 *
 * The field is `skillVersion`, not `version`: the binding names the exact skill version the
 * execution ran, and a binding that cannot name it cannot answer "which skill did this run use".
 */
export interface SkillBinding {
  id?: string;
  skillId: string;
  skillVersion: string;
  executionId?: string;
  policyId?: string;
  enabled?: boolean;
  createdAt?: string;
  [key: string]: unknown;
}

export interface ChangeRecord {
  id?: string;
  executionId: string;
  taskId?: string;
  projectId?: string;
  gitCommitSha?: string;
  [key: string]: unknown;
}

export interface ContextSnapshotRecord {
  id: string;
  projectId: string;
  taskId: string;
  executionId?: string;
  gitBaseSha: string;
  documentVersions: Record<string, string>;
  architectureVersion: string;
  gitnexusIndexVersion?: string;
  selectedFiles: string[];
  contextHash: string;
  createdAt: string;
}

export interface PromptSnapshotRecord {
  id: string;
  projectId: string;
  taskId: string;
  executionId: string;
  templateName: string;
  templateVersion: string;
  templateContentHash: string;
  renderedPromptHash: string;
  contextSnapshotId: string;
  modelConfig: Record<string, unknown>;
  createdAt: string;
}

export interface PolicySnapshotRecord {
  id: string;
  projectId: string;
  taskId: string;
  executionId: string;
  policyVersion: string;
  grantedCapabilities: string[];
  deniedCapabilities: string[];
  allowedPaths: string[];
  expiresAt: string;
  createdAt: string;
}

export interface EvaluationResult {
  executionId: string;
  verdict: "passed" | "failed" | "blocked";
  summary: string;
  checks: Array<{ name: string; status: string; [key: string]: unknown }>;
  [key: string]: unknown;
}

/** A permission request and the decision it was answered with. */
export interface TracedPermission {
  requestId: string;
  decision: "approve" | "deny";
  tool?: string;
  toolCallId?: string;
  reason?: string;
}

/** One tool call, with the decision that gated it. */
export interface TracedToolCall {
  toolCallId: string;
  toolName: string;
  sequence: number;
  status: "started" | "completed" | "failed";
  args?: Record<string, unknown>;
  permission?: TracedPermission;
}

/**
 * The whole chain of one execution: Task → Run → Tool → Change → Commit → Test.
 *
 * The task, the run and the tool calls are read off the timeline; the change records and the
 * evaluations are read off the record stores. A missing link is shown as missing rather than
 * hidden — a chain with a hole in it is information.
 */
export interface ExecutionTrace {
  executionId: string;
  taskIds: string[];
  runtimeId?: string;
  externalRunId?: string;
  toolCalls: TracedToolCall[];
  permissions: TracedPermission[];
  gitCommits: string[];
  changeRecords: ChangeRecord[];
  contextSnapshots: ContextSnapshotRecord[];
  promptSnapshots: PromptSnapshotRecord[];
  policySnapshots: PolicySnapshotRecord[];
  evaluations: EvaluationResult[];
}

export interface CommitTrace {
  changeRecord: ChangeRecord;
  trace: ExecutionTrace;
}

/** Every execution a task produced. */
export interface TaskTrace {
  taskId: string;
  executions: ExecutionTrace[];
}

/**
 * One recorded audit event on the timeline.
 *
 * The timeline is read from the server's journal, so an item here is exactly one recorded line:
 * the same thing the live stream replays.
 */
export interface AuditTimelineItem {
  executionId: string;
  sequence: number;
  kind: "runtime.event" | "audit";
  category: string;
  recordedAt: string;
  taskId?: string;
  parentExecutionId?: string;
  payload: Record<string, unknown>;
}

/** One execution's place in the timeline: what it was, and which stages it went through. */
export interface AuditTimelineExecution {
  executionId: string;
  taskIds: string[];
  parentExecutionId?: string;
  childExecutionIds: string[];
  runtimeId?: string;
  eventCount: number;
  firstRecordedAt: string;
  lastRecordedAt: string;
  /** Journal positions missing from this execution's recorded range. */
  gaps: number[];
  /** The stages the recorded events reconstruct, in the order they happened. */
  phases: string[];
  /** False when the recorded timestamps disagree with the recorded order (two different clocks). */
  timestampsMonotonic: boolean;
}

/**
 * The audit timeline: the recorded events of every execution, correlated and ordered.
 *
 * This is what design 15 asks for — "从审计事件还原完整执行过程" — and it is a different thing from
 * the trace: a trace is the set of records that belong together, a timeline is the sequence they
 * happened in.
 */
export interface AuditTimeline {
  items: AuditTimelineItem[];
  executions: AuditTimelineExecution[];
  taskIds: string[];
  categories: Record<string, number>;
  gaps: Array<{ executionId: string; sequences: number[] }>;
  truncated: boolean;
}
