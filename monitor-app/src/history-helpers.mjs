// Pure chart helpers, JS mirror of history-hooks.ts so Node's built-in test
// runner can exercise them without a TS compile step (the crate uses noEmit).
// Keep logic identical to history-hooks.ts.

export function downsamplePreserveExtremes(points, budget) {
  const n = points.length;
  if (n <= budget || budget < 4) return points.slice();
  const out = [points[0]];
  const interior = budget - 2;
  const buckets = Math.max(1, Math.floor(interior / 2));
  const span = points.length - 2;
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
    if (minP[0] <= maxP[0]) { out.push(minP); if (maxP !== minP) out.push(maxP); }
    else { out.push(maxP); if (minP !== maxP) out.push(minP); }
  }
  out.push(points[n - 1]);
  return out;
}

export function gapThresholdSecs(durationSecs) {
  if (durationSecs <= 3600) return 15;
  if (durationSecs <= 86400) return 45;
  return 200;
}
