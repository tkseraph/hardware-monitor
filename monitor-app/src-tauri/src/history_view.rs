//! Versioned display projection. Detect boundaries before selecting display vertices.
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize)]
pub struct Point {
    pub t: i64,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub count: u64,
    pub granularity_secs: u32,
    pub first_ts: i64,
    pub last_ts: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct HistoryView {
    pub version: u32,
    pub segments: Vec<Vec<Point>>,
    pub input_points: usize,
    pub downsampled: bool,
    pub omitted_segments: usize,
    pub aggregated: bool,
    pub continuity: &'static str,
}

fn segments(points: &[Point]) -> Vec<Vec<usize>> {
    let mut result: Vec<Vec<usize>> = Vec::new();
    for (i, point) in points.iter().enumerate() {
        let mut boundary = i == 0 || point.granularity_secs > 1;
        if i > 0 {
            let previous = &points[i - 1];
            boundary |= previous.granularity_secs > 1;
            if !boundary {
                let mut adjacent = Vec::new();
                for j in i.saturating_sub(2)..=(i + 2).min(points.len() - 1) {
                    if j == 0
                        || j == i
                        || points[j].granularity_secs > 1
                        || points[j - 1].granularity_secs > 1
                    {
                        continue;
                    }
                    let gap = points[j].t.saturating_sub(points[j - 1].t);
                    if gap > 0 {
                        adjacent.push(gap);
                    }
                }
                adjacent.sort_unstable();
                // Insufficient context is shown as separate points, not invented continuity.
                let threshold = adjacent
                    .get(adjacent.len() / 2)
                    .map(|v| v.saturating_mul(3).max(3));
                boundary = threshold.is_none_or(|limit| point.t.saturating_sub(previous.t) > limit);
            }
        }
        if boundary {
            result.push(Vec::new());
        }
        result.last_mut().unwrap().push(i);
    }
    result
}

pub fn project(points: Vec<Point>, budget: usize) -> HistoryView {
    let groups = segments(&points);
    let count = points.len();
    let aggregated = points.iter().any(|p| p.granularity_secs > 1 || p.count > 1);
    let mut selected = BTreeSet::new();
    if count <= budget {
        selected.extend(0..count);
    } else if count > 0 && budget > 0 {
        let low = (0..count)
            .min_by(|&a, &b| points[a].min.total_cmp(&points[b].min))
            .unwrap();
        let high = (0..count)
            .max_by(|&a, &b| points[a].max.total_cmp(&points[b].max))
            .unwrap();
        for index in [count - 1, 0, low, high] {
            if selected.len() < budget {
                selected.insert(index);
            }
        }
        let ends: BTreeSet<_> = groups
            .iter()
            .flat_map(|g| [g[0], g[g.len() - 1]])
            .chain(selected.iter().copied())
            .collect();
        if ends.len() <= budget {
            selected = ends;
        }
        let buckets = (budget - selected.len()) / 2;
        for bucket in 0..buckets {
            let start = bucket * count / buckets;
            let end = (bucket + 1) * count / buckets;
            if start >= end {
                continue;
            }
            let min = (start..end)
                .min_by(|&a, &b| points[a].min.total_cmp(&points[b].min))
                .unwrap();
            let max = (start..end)
                .max_by(|&a, &b| points[a].max.total_cmp(&points[b].max))
                .unwrap();
            selected.insert(min);
            selected.insert(max);
        }
    }
    let mut omitted = 0;
    let output = groups
        .into_iter()
        .filter_map(|group| {
            let subset: Vec<_> = group
                .into_iter()
                .filter(|i| selected.contains(i))
                .map(|i| points[i].clone())
                .collect();
            if subset.is_empty() {
                omitted += 1;
                None
            } else {
                Some(subset)
            }
        })
        .collect();
    HistoryView {
        version: 2,
        segments: output,
        input_points: count,
        downsampled: selected.len() < count,
        omitted_segments: omitted,
        aggregated,
        continuity: "inferred_raw_intervals;aggregate_points_disconnected",
    }
}

/// Compatibility endpoint still returns tuples, but no longer drops extrema/endpoints by stride.
pub fn legacy_points(points: Vec<(i64, f64)>, budget: usize) -> Vec<(i64, f64)> {
    project(
        points
            .into_iter()
            .map(|(t, value)| Point {
                t,
                value,
                min: value,
                max: value,
                count: 1,
                granularity_secs: 1,
                first_ts: t,
                last_ts: t,
            })
            .collect(),
        budget,
    )
    .segments
    .into_iter()
    .flatten()
    .map(|p| (p.t, p.value))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(t: i64, value: f64) -> Point {
        Point {
            t,
            value,
            min: value,
            max: value,
            count: 1,
            granularity_secs: 1,
            first_ts: t,
            last_ts: t,
        }
    }
    #[test]
    fn continuous_hour_does_not_gain_gaps_after_reduction() {
        let view = project((0..3600).map(|t| point(t, 40.0)).collect(), 100);
        assert_eq!(view.segments.len(), 1);
        assert!(view.segments[0].len() <= 100);
        assert_eq!(view.segments[0].first().unwrap().t, 0);
        assert_eq!(view.segments[0].last().unwrap().t, 3599);
    }
    #[test]
    fn actual_gap_extrema_and_latest_point_survive() {
        let points = (0..5000)
            .filter(|t| *t < 1000 || *t > 2000)
            .map(|t| {
                point(
                    t,
                    if t == 103 {
                        90.0
                    } else if t == 4500 {
                        -5.0
                    } else {
                        40.0
                    },
                )
            })
            .collect();
        let view = project(points, 80);
        assert_eq!(view.segments.len(), 2);
        assert_eq!(view.segments[0].last().unwrap().t, 999);
        assert_eq!(view.segments[1][0].t, 2001);
        let flat: Vec<_> = view.segments.iter().flatten().collect();
        assert!(flat.iter().any(|p| p.value == 90.0));
        assert!(flat.iter().any(|p| p.value == -5.0));
        assert_eq!(flat.last().unwrap().t, 4999);
    }
    #[test]
    fn stable_thirty_second_cadence_is_not_a_gap() {
        let view = project((0..100).map(|i| point(i * 30, 1.0)).collect(), 20);
        assert_eq!(view.segments.len(), 1);
    }
    #[test]
    fn coarse_buckets_never_claim_internal_continuity_and_keep_ranges() {
        let points = (0..100)
            .map(|i| {
                let mut p = point(i * 60, 40.0);
                p.granularity_secs = 60;
                p.count = 20;
                if i == 53 {
                    p.max = 99.0;
                }
                p
            })
            .collect();
        let view = project(points, 20);
        assert!(view.aggregated);
        assert!(view.omitted_segments > 0);
        assert!(view.segments.iter().all(|s| s.len() == 1));
        assert!(view.segments.iter().flatten().any(|p| p.max == 99.0));
    }
    #[test]
    fn all_budgets_are_bounded_and_zero_does_not_panic() {
        for budget in 0..15 {
            let view = project((0..80).map(|i| point(i, 1.0)).collect(), budget);
            assert!(view.segments.iter().map(Vec::len).sum::<usize>() <= budget);
        }
    }
}
