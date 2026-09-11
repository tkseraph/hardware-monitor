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

/**
 * Downsample points to at most `budget` vertices while preserving the first
 * and last point and each bucket's min and max (R10: never drop the newest
 * point, never smooth away a spike). Returns the input unchanged when it is
 * already within budget. Pure & unit-testable.
 */
export function downsamplePreserveExtremes(
  points: [number, number][],
  budget: number,
): [number, number][] {
  const n = points.length;
  if (n <= budget || budget < 4) return points.slice();
  const out: [number, number][] = [points[0]];
  // Interior budget excludes the preserved first and last points.
  const interior = budget - 2;
  // Each bucket contributes up to 2 points (min and max).
  const buckets = Math.max(1, Math.floor(interior / 2));
  const span = points.length - 2; // interior points between first and last
  for (let b = 0; b < buckets; b++) {
    const start = 1 + Math.floor((b * span) / buckets);
    const end = 1 + Math.floor(((b + 1) * span) / buckets);
    if (end <= start) continue;
    let minP = points[start];
    let maxP = points[start];
    for (let i = start; i < end; i++) {
      if (points[i][1] < minP[1]) minP = points[i];
      if (points[i][1] > maxP[1]) maxP = points[i];
    }
    // Emit in time order so the path never draws backwards.
    if (minP[0] <= maxP[0]) { out.push(minP); if (maxP !== minP) out.push(maxP); }
    else { out.push(maxP); if (minP !== maxP) out.push(minP); }
  }
  out.push(points[n - 1]);
  return out;
}

/**
 * Gap threshold (seconds) for a given range. Rather than a fixed 5/10s, the
 * break threshold scales with the expected sampling granularity of the range:
 * sub-minute (raw) ranges break at ~3 samples of a 5s cadence; coarser 7-day
 * ranges break proportionally wider so sparse-but-real 60s buckets are not all
 * drawn as faults (R10 gap semantics). Sleep/off gaps still exceed it and break.
 */
export function gapThresholdSecs(durationSecs: number): number {
  if (durationSecs <= 3600) return 15;      // 1h: raw ~1s cadence
  if (durationSecs <= 86400) return 45;     // 24h: 10s buckets
  return 200;                                // 7d: 60s buckets
}
