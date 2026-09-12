// Shared history-query hook and pure chart helpers (R10).
//
// Goals:
//  - single-flight: a new fetch supersedes any in-flight one; a stale response
//    can never overwrite newer data (sequence guard + cancellation).
//  - empty / loading / error are distinct states the UI renders honestly —
//    no fabricated points, no stale curve shown for a different selection.
//  - pause polling while the page/tab is hidden so the WebView does no useless
//    work; backend sampling is independent and unaffected.

import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

export type HistoryStatus = "idle" | "loading" | "ok" | "error";

export interface HistoryQuery {
  points: [number, number][];
  status: HistoryStatus;
  /** True once at least one fetch completed (ok or error) for this key. */
  loaded: boolean;
}

/**
 * Poll `get_history` for one metric/object/range. The query is keyed by
 * (metricId, objectId, durationSecs): changing any of them cancels the old
 * flight and starts clean, so a switched disk never shows the previous disk's
 * curve (R10 cache isolation).
 */
export function useHistoryQuery(
  metricId: string,
  objectId: string,
  durationSecs: number,
  pollMs = 5000,
): HistoryQuery {
  const [points, setPoints] = useState<[number, number][]>([]);
  const [status, setStatus] = useState<HistoryStatus>("idle");
  const [loaded, setLoaded] = useState(false);
  // Monotonic sequence: only the latest issued fetch may commit its result.
  const seq = useRef(0);

  useEffect(() => {
    if (!isTauri() || !objectId) {
      setPoints([]);
      setStatus("idle");
      setLoaded(false);
      return;
    }
    // New key: clear the previous curve immediately so we never render stale.
    setPoints([]);
    setStatus("loading");
    setLoaded(false);
    let cancelled = false;

    const fetchOnce = async () => {
      const mySeq = ++seq.current;
      try {
        const data = await invoke<[number, number][]>("get_history", {
          metricId,
          objectId,
          durationSecs,
        });
        if (cancelled || mySeq !== seq.current) return; // superseded
        setPoints(data);
        setStatus("ok");
        setLoaded(true);
      } catch (err) {
        if (cancelled || mySeq !== seq.current) return;
        setPoints([]);
        setStatus("error");
        setLoaded(true);
        console.error("Failed to fetch history:", err);
      }
    };

    fetchOnce();
    const interval = setInterval(() => {
      // Skip polling while hidden; the next visible tick refetches.
      if (typeof document !== "undefined" && document.visibilityState === "hidden") return;
      fetchOnce();
    }, pollMs);

    const onVisible = () => { if (document.visibilityState === "visible") fetchOnce(); };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      cancelled = true;
      clearInterval(interval);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [metricId, objectId, durationSecs, pollMs]);

  return { points, status, loaded };
}

export { downsamplePreserveExtremes, gapThresholdSecs } from "./history-data";
