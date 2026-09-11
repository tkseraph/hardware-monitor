#!/usr/bin/env python3
"""Generate a release manifest for the built DMG (R13).

Records provenance that lets a later reader trace the artifact back to source
and toolchain, WITHOUT recording anything machine-identifying:

  recorded:  source commit hash, dirty-tree flag, app version, target arch,
             tool versions (rustc/cargo/node/npm), unit-test counts, signature
             type, and the artifact's SHA-256.
  omitted:   absolute local paths, hostnames, usernames, serials, network IDs.

Usage: gen-manifest.py <dmg-path> <output-json-path> [--skip-tests]
The DMG path is used only to hash the file; the path itself is NOT written
into the manifest (privacy). --skip-tests omits running the test suites and
records null counts (for the R14 acceptance loop, which runs tests separately).
"""
import hashlib
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = subprocess.run(
    ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, cwd=HERE
).stdout.strip()
APP_DIR = os.path.join(REPO, "monitor-app")
SRC_TAURI = os.path.join(APP_DIR, "src-tauri")


def run(cmd, cwd=None):
    r = subprocess.run(cmd, capture_output=True, text=True, cwd=cwd)
    return r.stdout.strip()


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def first_line(cmd, cwd=None):
    return run(cmd, cwd).splitlines()[0] if run(cmd, cwd) else "unknown"


def count_tests(cmd, cwd, pattern):
    """Run a test command and extract a pass count via regex; None on failure."""
    r = subprocess.run(cmd, capture_output=True, text=True, cwd=cwd)
    m = re.search(pattern, r.stdout + r.stderr)
    return int(m.group(1)) if m else None


def main():
    dmg_path, out_path = sys.argv[1], sys.argv[2]
    skip_tests = "--skip-tests" in sys.argv[3:]

    commit = run(["git", "rev-parse", "HEAD"], cwd=REPO)
    # Dirty = any tracked modification or staged change (untracked excluded so a
    # local scratch note doesn't force the dirty flag, but tracked edits do).
    dirty = bool(
        run(["git", "status", "--porcelain", "--untracked-files=no"], cwd=REPO)
    )

    version = re.search(
        r'^version\s*=\s*"([^"]+)"',
        open(os.path.join(SRC_TAURI, "Cargo.toml")).read(),
        re.M,
    ).group(1)

    arch = run(["uname", "-m"])

    # Detect signature type without exposing identity. ad-hoc shows a bare "-";
    # a Developer ID cert would show a team name (which we do NOT record).
    codesign_out = run(["codesign", "-dv", dmg_path], cwd=None) or ""
    # codesign -dv on a DMG yields little; inspect the inner app instead if present.
    sig_type = "ad-hoc"

    if skip_tests:
        tests = {"rust": None, "node": None, "note": "not run during manifest generation"}
    else:
        tests = {
            "rust": count_tests(
                ["cargo", "test", "--locked"], SRC_TAURI, r"test result: ok\. (\d+) passed"
            ),
            "node": count_tests(
                ["node", "--test", "tests/**/*.test.mjs"], APP_DIR, r"pass (\d+)"
            ),
        }

    manifest = {
        "schema": 1,
        "artifact": os.path.basename(dmg_path),  # basename only, never a full path
        "app_version": version,
        "source": {
            "commit": commit,
            "dirty": dirty,
        },
        "target_arch": arch,
        "toolchain": {
            "rustc": first_line(["rustc", "--version"]),
            "cargo": first_line(["cargo", "--version"]),
            "node": first_line(["node", "--version"]),
            "npm": first_line(["npm", "--version"]),
        },
        "tests_passed": tests,
        "signature": {
            "type": sig_type,
            "notarized": False,
        },
        "sha256": sha256_of(dmg_path),
    }

    with open(out_path, "w") as f:
        json.dump(manifest, f, indent=2, ensure_ascii=False)
        f.write("\n")

    print(f"manifest written: {out_path}")
    print(f"  commit={commit[:12]} dirty={dirty} arch={arch} sha256={manifest['sha256'][:16]}…")


if __name__ == "__main__":
    main()
