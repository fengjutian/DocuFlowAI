import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Small async data hook.
 *
 * The loader is kept in a ref so that a page can pass an inline arrow function without
 * re-fetching on every render; `deps` decides when the request actually runs.
 */
export interface ApiDataState<T> {
  data: T | undefined;
  error: Error | undefined;
  loading: boolean;
  refresh: () => void;
  lastUpdated: Date | undefined;
}

export function useApiData<T>(
  loader: () => Promise<T>,
  deps: unknown[],
  options: { pollMs?: number; enabled?: boolean } = {}
): ApiDataState<T> {
  const { pollMs, enabled = true } = options;
  const [data, setData] = useState<T | undefined>(undefined);
  const [error, setError] = useState<Error | undefined>(undefined);
  const [loading, setLoading] = useState(enabled);
  const [lastUpdated, setLastUpdated] = useState<Date | undefined>(undefined);
  const [nonce, setNonce] = useState(0);

  const loaderRef = useRef(loader);
  loaderRef.current = loader;

  const refresh = useCallback(() => setNonce(value => value + 1), []);

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const run = async () => {
      try {
        const result = await loaderRef.current();
        if (cancelled) return;
        setData(result);
        setError(undefined);
        setLastUpdated(new Date());
      } catch (caught) {
        if (cancelled) return;
        setError(caught instanceof Error ? caught : new Error(String(caught)));
      } finally {
        if (!cancelled) {
          setLoading(false);
          if (pollMs) timer = setTimeout(() => void run(), pollMs);
        }
      }
    };

    setLoading(true);
    void run();

    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, nonce, pollMs, enabled]);

  return { data, error, loading, refresh, lastUpdated };
}
