import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { normalizeBaseUrl, useConnection } from "@/lib/settings";

/**
 * Backend reachability.
 *
 * The desktop app is a client of the lab server, not part of it: when the server is down the honest
 * thing to show is "未连接", not a stale dashboard full of values that may no longer be true.
 */
export type BackendStatus = "checking" | "online" | "offline";

export interface BackendState {
  status: BackendStatus;
  baseUrl: string;
  message?: string;
  refresh: () => void;
}

const PROBE_INTERVAL_MS = 10_000;

export function useBackendStatus(): BackendState {
  const baseUrl = useConnection(state => state.baseUrl);
  const token = useConnection(state => state.token);
  const [status, setStatus] = useState<BackendStatus>("checking");
  const [message, setMessage] = useState<string | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const probe = async () => {
      try {
        await api.listRuntimes();
        if (cancelled) return;
        setStatus("online");
        setMessage(undefined);
      } catch (error) {
        if (cancelled) return;
        setStatus("offline");
        setMessage(error instanceof Error ? error.message : String(error));
      }
    };
    void probe();
    const timer = setInterval(() => void probe(), PROBE_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [baseUrl, token, nonce]);

  return {
    status,
    baseUrl: normalizeBaseUrl(baseUrl),
    ...(message ? { message } : {}),
    refresh: () => setNonce(value => value + 1)
  };
}
