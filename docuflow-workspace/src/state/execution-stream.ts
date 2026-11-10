import { createContext, useContext, useMemo } from "react";
import type { StreamStatus } from "@/lib/events";
import type { DocuFlowEvent } from "@/lib/types";

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
  events: DocuFlowEvent[];
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
export function useExecutionEvents(executionId: string | undefined): DocuFlowEvent[] {
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
