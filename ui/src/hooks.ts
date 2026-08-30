import { useCallback, useEffect, useRef, useState } from "react";

/** Run `fn` immediately and then every `ms` milliseconds. */
export function useInterval(fn: () => void, ms: number): void {
  const ref = useRef(fn);
  useEffect(() => {
    ref.current = fn;
  }, [fn]);
  useEffect(() => {
    ref.current();
    const t = window.setInterval(() => ref.current(), ms);
    return () => window.clearInterval(t);
  }, [ms]);
}

export interface Poll<T> {
  data: T | null;
  error: string | null;
  refresh: () => Promise<void>;
}

/** Poll an async fetcher on an interval; keeps the last good data on errors. */
export function usePoll<T>(fetcher: () => Promise<T>, ms: number): Poll<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const fetcherRef = useRef(fetcher);
  useEffect(() => {
    fetcherRef.current = fetcher;
  }, [fetcher]);
  const inFlight = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const result = await fetcherRef.current();
      setData(result);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      inFlight.current = false;
    }
  }, []);

  useInterval(() => {
    void refresh();
  }, ms);

  return { data, error, refresh };
}

/** Format an epoch-seconds timestamp as a local HH:MM:SS clock time. */
export function fmtTime(epochSeconds: number): string {
  return new Date(epochSeconds * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}
