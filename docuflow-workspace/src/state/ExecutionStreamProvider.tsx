import { useEffect, useMemo, useState, type ReactNode } from "react";
import { subscribeEvents, type StreamStatus } from "@/lib/events";
import type { DocuFlowEvent } from "@/lib/types";
import { StreamContext, type ExecutionSummary, type StreamContextValue } from "@/state/execution-stream";

/**
 * Global event stream.
 *
 * The lab server has no "list executions" endpoint: the journal is per execution and the live stream
 * is the only place where every execution shows up. The provider therefore keeps one subscription to
 * `GET /api/v1/events` and derives the execution list from what arrives, which is exactly the source
 * the API itself uses for live state.
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
  const [events, setEvents] = useState<DocuFlowEvent[]>([]);
  const [streamStatus, setStreamStatus] = useState<StreamStatus>("connecting");
  const [streamMessage, setStreamMessage] = useState<string | undefined>(undefined);

  useEffect(() => {
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
      }
    });
    return unsubscribe;
  }, []);

  const executions = useMemo(() => {
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
    return [...byExecution.values()].sort((a, b) =>
      (b.lastTimestamp ?? "").localeCompare(a.lastTimestamp ?? "")
    );
  }, [events]);

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
