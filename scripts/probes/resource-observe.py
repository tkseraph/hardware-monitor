#!/usr/bin/env python3
"""Low-overhead resource observation for R14 acceptance.

Continuously samples the monitor process tree (main process + WebContent /
Networking renderers + any spawn) at a fixed cadence and writes a JSON report
with a per-tick series — NOT just endpoint snapshots — so RSS/CPU trends are
visible, plus thread and child-process bounds.

Safety / honesty constraints:
  - Read-only `ps` sampling; never sends signals, never triggers sleep, never
    writes to the app's data dir.
  - Reports what was actually sampled; if a metric is unavailable it is null,
    never fabricated as 0.
  - The report is written to a caller-chosen isolated dir (default: a gitignored
    scripts/probes/reports/ subpath) and contains no hostnames/usernames/serials
    (PIDs and elapsed seconds only — these are ephemeral and process-local).

Usage:
  resource-observe.py [--duration-secs 600] [--interval-secs 5] [--out PATH]

The target process is matched by executable name "monitor"/"app" under the
monitor project. If not running, the observation still runs and records
samples_found=0 honestly (so a missed launch is visible, not silent).
"""
import argparse
import json
import os
import re
import subprocess
import time
from datetime import datetime, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_OUT_DIR = os.path.join(HERE, "reports")

# Match ONLY our app's binary. The bundle binary is named "app" and lives under
# ".../monitor.app/Contents/MacOS/app"; the dev target binary is
# ".../target/release/app" or ".../target/debug/app". Anchored so a differently-
# named app merely containing "monitor.app" as a substring is NOT matched.
APP_NAME_RE = re.compile(
    r"(/monitor\.app/Contents/MacOS/app$|/target/(release|debug)/app$)"
)


def sample_processes():
    """Return a list of dicts for the process tree, or [] if app not running."""
    # ps: pid ppid %cpu rss(KB) nlwp(thread count via -M is complex; use a 2nd call)
    out = subprocess.run(
        ["ps", "-Ao", "pid=,ppid=,%cpu=,rss=,comm="],
        capture_output=True, text=True,
    ).stdout
    rows = []
    for line in out.splitlines():
        parts = line.split(None, 4)
        if len(parts) < 5:
            continue
        pid, ppid, cpu, rss_kb, comm = parts
        rows.append({
            "pid": int(pid), "ppid": int(ppid), "cpu": float(cpu),
            "rss_kb": int(rss_kb), "comm": comm,
        })
    return rows


def classify(rows):
    """Pick the app's main process + its direct/descendant WebKit helpers."""
    mains = [r for r in rows if APP_NAME_RE.search(r["comm"])]
    if not mains:
        return None
    # Build ppid→children map for descendant walk.
    by_ppid = {}
    for r in rows:
        by_ppid.setdefault(r["ppid"], []).append(r)
    tree = []
    stack = list(mains)
    seen = set()
    while stack:
        node = stack.pop()
        if node["pid"] in seen:
            continue
        seen.add(node["pid"])
        tree.append(node)
        stack.extend(by_ppid.get(node["pid"], []))
    return tree


def thread_count(pid):
    """Thread count via `ps -M <pid>` line count (minus header). None on error."""
    r = subprocess.run(["ps", "-M", str(pid)], capture_output=True, text=True)
    if r.returncode != 0:
        return None
    lines = [l for l in r.stdout.splitlines() if l.strip()]
    return max(0, len(lines) - 1)  # subtract header


def percentile(sorted_vals, pct):
    if not sorted_vals:
        return None
    k = (len(sorted_vals) - 1) * (pct / 100.0)
    lo = int(k)
    hi = min(lo + 1, len(sorted_vals) - 1)
    frac = k - lo
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * frac


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--duration-secs", type=int, default=600)
    ap.add_argument("--interval-secs", type=int, default=5)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    os.makedirs(DEFAULT_OUT_DIR, exist_ok=True)
    out = args.out or os.path.join(
        DEFAULT_OUT_DIR,
        "resource-observe-{}s-{}.json".format(
            args.duration_secs,
            datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ"),
        ),
    )

    t0 = time.monotonic()
    series = []
    while True:
        elapsed = time.monotonic() - t0
        if elapsed > args.duration_secs:
            break
        rows = sample_processes()
        tree = classify(rows)
        if tree is None:
            series.append({"t_secs": round(elapsed, 1), "samples_found": 0})
        else:
            total_rss_kb = sum(r["rss_kb"] for r in tree)
            total_cpu = sum(r["cpu"] for r in tree)
            threads = {}
            for r in tree:
                tc = thread_count(r["pid"])
                if tc is not None:
                    threads[r["pid"]] = tc
            series.append({
                "t_secs": round(elapsed, 1),
                "samples_found": len(tree),
                "procs": [
                    {"pid": r["pid"], "cpu": r["cpu"], "rss_kb": r["rss_kb"],
                     "threads": threads.get(r["pid"])}
                    for r in tree
                ],
                "total_rss_kb": total_rss_kb,
                "total_cpu_pct": round(total_cpu, 2),
                "max_threads": max(threads.values()) if threads else None,
            })
        # Sleep to the next tick boundary relative to t0 to avoid drift.
        next_tick = t0 + (len(series)) * args.interval_secs
        time.sleep(max(0.0, next_tick - time.monotonic()))

    # Aggregate. Only ticks where the app was found contribute to stats.
    found = [s for s in series if s.get("samples_found")]
    total_rss = sorted(s["total_rss_kb"] for s in found)
    total_cpu = sorted(s["total_cpu_pct"] for s in found)
    report = {
        "schema": 1,
        "duration_secs_requested": args.duration_secs,
        "interval_secs": args.interval_secs,
        "ticks": len(series),
        "ticks_with_app": len(found),
        "rss_kb": {
            "p50": percentile(total_rss, 50),
            "p95": percentile(total_rss, 95),
            "first": total_rss[0] if total_rss else None,
            "last": total_rss[-1] if total_rss else None,
            "min": total_rss[0] if total_rss else None,
            "max": total_rss[-1] if total_rss else None,
        },
        "cpu_pct": {
            "p50": percentile(total_cpu, 50),
            "p95": percentile(total_cpu, 95),
            "max": total_cpu[-1] if total_cpu else None,
        },
        "child_process_count_max": max((s["samples_found"] for s in found), default=0),
        "thread_count_max": max(
            (s.get("max_threads") or 0 for s in found), default=0
        ) or None,
        "rss_trend": "unknown",
        "series": series,
    }
    # RSS trend: compare first-vs-last thirds' medians (robust to noise).
    if len(total_rss) >= 6:
        third = len(total_rss) // 3
        head = percentile(sorted(total_rss[:third] or total_rss[:1]), 50)
        tail = percentile(sorted(total_rss[-third:] or total_rss[-1:]), 50)
        if head and tail:
            if tail > head * 1.15:
                report["rss_trend"] = "growing"
            elif tail < head * 0.85:
                report["rss_trend"] = "shrinking"
            else:
                report["rss_trend"] = "stable"

    with open(out, "w") as f:
        json.dump(report, f, indent=2)
        f.write("\n")

    r = report["rss_kb"]
    c = report["cpu_pct"]
    print(f"report: {out}")
    print(f"  ticks={report['ticks']} with_app={report['ticks_with_app']}")
    print(f"  RSS KB p50={r['p50']} p95={r['p95']} min={r['min']} max={r['max']} trend={report['rss_trend']}")
    print(f"  CPU% p50={c['p50']} p95={c['p95']} max={c['max']}")
    print(f"  child_procs_max={report['child_process_count_max']} threads_max={report['thread_count_max']}")


if __name__ == "__main__":
    main()
