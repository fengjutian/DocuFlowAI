export interface HermesRunCreateRequest {
  /** Prompt text submitted to the Hermes runtime. */
  input: string;
  /** Optional model override supported by the pinned server version. */
  model?: string;
  /** Optional metadata for correlation; verify support on the pinned version. */
  metadata?: Record<string, string>;
}

export interface HermesRun {
  id: string;
  status?: string;
  [key: string]: unknown;
}

export interface HermesCapabilities {
  [key: string]: unknown;
}

export interface HermesSseEvent {
  event: string;
  data: unknown;
  id?: string;
  retry?: number;
}

export type TestStatus = "passed" | "failed" | "blocked" | "not_run";

export interface TestResult {
  id: string;
  title: string;
  status: TestStatus;
  startedAt: string;
  finishedAt: string;
  evidence: string[];
  notes?: string;
}
