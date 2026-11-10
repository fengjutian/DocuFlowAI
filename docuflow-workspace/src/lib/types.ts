/**
 * Wire types for the DocuFlow lab server (`docuflow-hermes-lab`), design V1.0 sections 12 and 13.
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

export interface ExecutionDetail {
  executionId: string;
  status: string;
  checkpoint: ExecutionCheckpoint;
  pendingApprovals: RuntimePermissionRequest[];
  completion: CompletionGate;
}

/** Unified event envelope (design 13). `data` is the runtime payload, kept loose on purpose. */
export interface DocuFlowEvent<T = Record<string, unknown>> {
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
}

export interface RecoveryPlan {
  action: "stop" | "compensate" | "resume" | "rebuild";
  reason: string;
  fromSequence: number;
  checkpoint: ExecutionCheckpoint;
}

export interface RetryResponse {
  previousExecutionId: string;
  execution: AgentExecution;
  plan: RecoveryPlan;
}

export interface ResumeResponse {
  execution: AgentExecution;
  plan: RecoveryPlan;
}

export interface UsageRecord {
  id: string;
  executionId: string;
  model: string;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  durationMs: number;
  startedAt?: string;
  finishedAt?: string;
}

export interface UsageSummary {
  executions: number;
  promptTokens: number;
  completionTokens: number;
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
  checksum: string;
  createdAt: string;
  deprecated?: boolean;
}

export interface SkillBinding {
  skillId: string;
  version: string;
  executionId?: string;
  status?: string;
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

export interface ExecutionTrace {
  executionId: string;
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
