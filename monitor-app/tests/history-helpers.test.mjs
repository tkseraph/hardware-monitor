import { test } from "node:test";
import assert from "node:assert/strict";
import { downsamplePreserveExtremes, gapThresholdSecs } from "../src/history-helpers.mjs";

test("within budget returns copy unchanged", () => {
  const pts = [[0, 1], [10, 2], [20, 3]];
  const out = downsamplePreserveExtremes(pts, 10);
  assert.deepEqual(out, pts);
  assert.notEqual(out, pts, "returns a copy, not the same array");
});

test("always preserves first and last points", () => {
  const pts = [];
  for (let i = 0; i < 1000; i++) pts.push([i, Math.sin(i / 10) * 50 + 50]);
  const out = downsamplePreserveExtremes(pts, 100);
  assert.equal(out[0][0], pts[0][0], "first point preserved");
  assert.equal(out[out.length - 1][0], pts[pts.length - 1][0], "last point preserved");
  assert.ok(out.length <= 100, "respects budget");
});

test("preserves a spike's min and max", () => {
  // One huge spike in the middle; a naive decimation would drop it.
  const pts = [];
  for (let i = 0; i < 500; i++) pts.push([i, 10]);
  pts[250] = [250, 99]; // spike
  const out = downsamplePreserveExtremes(pts, 50);
  const maxV = Math.max(...out.map(([, v]) => v));
  assert.equal(maxV, 99, "spike peak survives downsampling");
});

test("emits points in non-decreasing time order", () => {
  const pts = [];
  for (let i = 0; i < 800; i++) pts.push([i, (i * 7) % 100]);
  const out = downsamplePreserveExtremes(pts, 64);
  for (let i = 1; i < out.length; i++) {
    assert.ok(out[i][0] >= out[i - 1][0], "time order preserved (path never draws backwards)");
  }
});

test("gap threshold scales with range granularity", () => {
  assert.equal(gapThresholdSecs(3600), 15);
  assert.equal(gapThresholdSecs(86400), 45);
  assert.equal(gapThresholdSecs(604800), 200);
  assert.ok(gapThresholdSecs(604800) > gapThresholdSecs(3600), "coarser range tolerates wider gaps");
});

test("tiny or empty input never throws", () => {
  assert.deepEqual(downsamplePreserveExtremes([], 100), []);
  assert.deepEqual(downsamplePreserveExtremes([[5, 1]], 4), [[5, 1]]);
});
