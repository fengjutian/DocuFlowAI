import { createContext, useContext, useMemo } from "react";
import type { StreamStatus } from "@/lib/events";
import type { CodeWeaverEvent, ExecutionSummaryRecord } from "@/lib/types";

/**
 * Shared state of the global event stream.
 *
 * Kept apart from the provider component so that React Fast Refresh does not have to invalidate the
 * module every time a hook changes.
 */
export interface ExecutionSummary {
  executionId: string;
  projectId: string;
  taskId?: string;
  status: string;
  lastEventType?: string;
  lastTimestamp?: string;
  firstTimestamp?: string;
  eventCount: number;
}

export interface StreamContextValue {
  events: CodeWeaverEvent[];
  executions: ExecutionSummary[];
  streamStatus: StreamStatus;
  streamMessage?: string;
}

export const StreamContext = createContext<StreamContextValue | undefined>(undefined);

export function useExecutionStream(): StreamContextValue {
  const value = useContext(StreamContext);
  if (!value) throw new Error("useExecutionStream must be used inside ExecutionStreamProvider");
  return value;
}

/** Events of one execution, oldest first. */
export function useExecutionEvents(executionId: string | undefined): CodeWeaverEvent[] {
  const { events } = useExecutionStream();
  return useMemo(
    () => (executionId ? events.filter(event => event.executionId === executionId) : []),
    [events, executionId]
  );
}

/**
 * Permission requests that were raised and not yet resolved.
 *
 * The count comes from the event stream rather than from a separate endpoint: the server keeps the
 * authoritative pending list per execution, and the stream is what the overview can afford to read.
 */
export function usePendingPermissionCount(): number {
  const { events } = useExecutionStream();
  return useMemo(() => {
    const requested = new Set<string>();
    const resolved = new Set<string>();
    for (const event of events) {
      const requestId = event.data?.["requestId"];
      if (typeof requestId !== "string") continue;
      if (event.type === "agent.permission.requested") requested.add(requestId);
      if (event.type === "agent.permission.resolved") resolved.add(requestId);
    }
    return [...requested].filter(id => !resolved.has(id)).length;
  }, [events]);
}

/** How the stream state reads to a user; the reconnecting state is the one that needs saying out loud. */
export function streamStatusLabel(status: StreamStatus): string {
  switch (status) {
    case "connecting":
      return "连接中";
    case "open":
      return "已连接";
    case "reconnecting":
      return "重连中";
    case "polling":
      return "轮询";
    case "closed":
      return "已断开";
    case "error":
      return "异常";
  }
}

/**
 * Merges the server's execution list with what the live stream saw.
 *
 * The snapshot is the server's own account of its journal, so it knows every execution — including
 * the ones that ran while this console was not listening. The live stream only knows the executions
 * it has watched, but it knows their state as of right now. One execution is one row: the two sides
 * are combined, never listed twice.
 */
export function mergeExecutionSummaries(
  snapshot: ExecutionSummaryRecord[],
  live: ExecutionSummary[]
): ExecutionSummary[] {
  const byExecution = new Map<string, ExecutionSummary>();

  for (const record of snapshot) {
    byExecution.set(record.executionId, {
      executionId: record.executionId,
      projectId: record.projectId ?? "",
      ...(record.taskId ? { taskId: record.taskId } : {}),
      status: record.status,
      eventCount: record.eventCount,
      ...(record.firstRecordedAt ? { firstTimestamp: record.firstRecordedAt } : {}),
      ...(record.lastRecordedAt ? { lastTimestamp: record.lastRecordedAt } : {})
    });
  }

  for (const item of live) {
    const existing = byExecution.get(item.executionId);
    if (!existing) {
      byExecution.set(item.executionId, item);
      continue;
    }
    // Both sides saw this execution. The state comes from whichever evidence is newer: the live
    // frame wins while it is current, but the snapshot wins when it was taken after the last live
    // frame — that is the reconnect case, where the journal moved on while the socket was down.
    const liveIsCurrent = compareTime(item.lastTimestamp, existing.lastTimestamp) >= 0;
    byExecution.set(item.executionId, {
      executionId: item.executionId,
      projectId: item.projectId || existing.projectId,
      ...(item.taskId ?? existing.taskId ? { taskId: item.taskId ?? existing.taskId } : {}),
      status: liveIsCurrent ? item.status : existing.status,
      ...(item.lastEventType ?? existing.lastEventType
        ? { lastEventType: item.lastEventType ?? existing.lastEventType }
        : {}),
      lastTimestamp: laterTime(item.lastTimestamp, existing.lastTimestamp),
      firstTimestamp: earlierTime(item.firstTimestamp, existing.firstTimestamp),
      // The snapshot counts every journal entry of the execution; the live side only counted what
      // it received, so the larger number is the truthful one.
      eventCount: Math.max(item.eventCount, existing.eventCount)
    });
  }

  return [...byExecution.values()].sort((a, b) =>
    (b.lastTimestamp ?? "").localeCompare(a.lastTimestamp ?? "")
  );
}

/** Compares two timestamps; an unknown timestamp is older than a known one. */
function compareTime(left: string | undefined, right: string | undefined): number {
  if (!left && !right) return 0;
  if (!left) return -1;
  if (!right) return 1;
  const leftTime = Date.parse(left);
  const rightTime = Date.parse(right);
  if (Number.isNaN(leftTime) && Number.isNaN(rightTime)) return 0;
  if (Number.isNaN(leftTime)) return -1;
  if (Number.isNaN(rightTime)) return 1;
  return leftTime - rightTime;
}

function laterTime(left: string | undefined, right: string | undefined): string | undefined {
  if (!left) return right;
  if (!right) return left;
  return compareTime(left, right) >= 0 ? left : right;
}

function earlierTime(left: string | undefined, right: string | undefined): string | undefined {
  if (!left) return right;
  if (!right) return left;
  return compareTime(left, right) <= 0 ? left : right;
}
