import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { subscribeEvents, type StreamStatus } from "@/lib/events";
import { api } from "@/lib/api";
import type { CodeWeaverEvent, ExecutionSummaryRecord } from "@/lib/types";
import {
  StreamContext,
  mergeExecutionSummaries,
  type ExecutionSummary,
  type StreamContextValue
} from "@/state/execution-stream";

/**
 * Global event stream.
 *
 * The provider keeps one subscription to `GET /api/v1/events` and derives the live state from what
 * arrives. The live stream only knows the executions it has watched, so the execution list is also
 * read from the server's own journal summary (`GET /api/v1/executions`): that snapshot is what puts
 * back the executions that started and finished while the console was disconnected.
 *
 * The snapshot is therefore pulled once at start and again after every reconnect, and the two
 * sources are merged by execution id.
 *
 * The stream is a notification channel, never a verdict: an execution is only "successful" when its
 * terminal state and the evaluation gate say so.
 */
const MAX_EVENTS = 800;

const STATUS_BY_EVENT: Record<string, string> = {
  "agent.execution.started": "running",
  "agent.execution.waiting": "waiting_approval",
  "agent.execution.completed": "succeeded",
  "agent.execution.failed": "failed",
  "agent.execution.cancelled": "cancelled"
};

export function ExecutionStreamProvider({ children }: { children: ReactNode }) {
  const [events, setEvents] = useState<CodeWeaverEvent[]>([]);
  const [snapshot, setSnapshot] = useState<ExecutionSummaryRecord[]>([]);
  const [streamStatus, setStreamStatus] = useState<StreamStatus>("connecting");
  const [streamMessage, setStreamMessage] = useState<string | undefined>(undefined);

  /**
   * Reads the server's execution list.
   *
   * A failure here is not fatal: the live stream still carries the executions it watches, so the
   * console keeps working and the next reconnect tries again.
   */
  const refreshSnapshot = useCallback(() => {
    void api
      .listExecutions()
      .then(result => setSnapshot(result.executions ?? []))
      .catch(() => {
        // The list endpoint is the recovery path, not the only path; the live stream still runs.
      });
  }, []);

  useEffect(() => {
    refreshSnapshot();
    const unsubscribe = subscribeEvents({
      onEvent: event => {
        setEvents(current => {
          const next = [...current, event];
          return next.length > MAX_EVENTS ? next.slice(next.length - MAX_EVENTS) : next;
        });
      },
      onStatus: (status, detail) => {
        setStreamStatus(status);
        setStreamMessage(detail);
      },
      // The socket only carries what happens while it listens, so a reconnect is the moment to read
      // back what was missed.
      onReconnect: refreshSnapshot
    });
    return unsubscribe;
  }, [refreshSnapshot]);

  const liveExecutions = useMemo(() => {
    const byExecution = new Map<string, ExecutionSummary>();
    for (const event of events) {
      if (!event.executionId) continue;
      const existing = byExecution.get(event.executionId);
      const status = STATUS_BY_EVENT[event.type] ?? existing?.status ?? "running";
      byExecution.set(event.executionId, {
        executionId: event.executionId,
        projectId: event.projectId || existing?.projectId || "",
        ...(event.taskId ? { taskId: event.taskId } : existing?.taskId ? { taskId: existing.taskId } : {}),
        status,
        lastEventType: event.type,
        lastTimestamp: event.timestamp,
        firstTimestamp: existing?.firstTimestamp ?? event.timestamp,
        eventCount: (existing?.eventCount ?? 0) + 1
      });
    }
    return [...byExecution.values()];
  }, [events]);

  const executions = useMemo(
    () => mergeExecutionSummaries(snapshot, liveExecutions),
    [snapshot, liveExecutions]
  );

  const value = useMemo<StreamContextValue>(
    () => ({
      events,
      executions,
      streamStatus,
      ...(streamMessage ? { streamMessage } : {})
    }),
    [events, executions, streamStatus, streamMessage]
  );

  return <StreamContext.Provider value={value}>{children}</StreamContext.Provider>;
}
