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

/** Identify actual gaps BEFORE discarding any display vertices.
 * Preserve each segment's endpoints even if many gaps exceed the soft budget.
 */
export function temperatureSegments(points: [number, number][], gapSecs = 15, budget = 400): [number, number][][] {
  const segments: [number, number][][] = [];
  points.forEach((p, i) => {
    if (!i || p[0] - points[i - 1][0] > gapSecs) segments.push([]);
    segments[segments.length - 1].push(p);
  });
  return segments.map(segment => downsamplePreserveExtremes(segment,
    Math.max(4, Math.floor(budget * segment.length / points.length))));
}
