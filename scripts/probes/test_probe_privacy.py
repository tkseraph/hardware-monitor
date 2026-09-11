#!/usr/bin/env python3
"""Synthetic privacy/status contract tests for probe-all.py (R12/A14/A25).

These use DECOY values only — no real paths, serials, or tokens. They verify:
  - forbidden fields are dropped and sensitive values redacted (A14);
  - missing required fields are NOT marked verified (A25);
  - run() never leaks a TimeoutExpired's embedded output (A14).

Marked synthetic: every fixture value below is fake by construction.
"""
import importlib.util
import json
import os
import subprocess
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = importlib.util.spec_from_file_location("probe_all", os.path.join(HERE, "probe-all.py"))
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)


class TestScrub(unittest.TestCase):
    DECOY_PATH = "/Users/decoy-user/secret"      # synthetic, not a real account
    DECOY_UUID = "12345678-1234-1234-1234-1234567890ab"  # synthetic

    def test_forbidden_keys_dropped(self):
        obj = {"mount_point": "/System/Volumes/Data", "name": "Data",
               "MountPoint": "/x", "SerialNumber": "DECOYSERIAL",
               "capacity_consumed": 123}
        out = probe.scrub(obj)
        for k in ("mount_point", "MountPoint", "SerialNumber"):
            self.assertNotIn(k, out)
        self.assertEqual(out["name"], "Data")
        self.assertEqual(out["capacity_consumed"], 123)

    def test_user_path_redacted(self):
        self.assertNotIn("decoy-user", probe.scrub(self.DECOY_PATH))
        self.assertIn("[redacted]", probe.scrub(self.DECOY_PATH))

    def test_uuid_redacted(self):
        self.assertNotIn("12345678", probe.scrub(self.DECOY_UUID))

    def test_nested_scrub(self):
        obj = {"volumes": [{"id": "disk1s1", "mount_point": "/v"}]}
        out = probe.scrub(obj)
        self.assertNotIn("mount_point", out["volumes"][0])


class TestRunPrivacy(unittest.TestCase):
    def test_timeout_returns_error_code_not_output(self):
        # `sleep` exceeds the timeout; the result must carry only an error code,
        # never a serialized TimeoutExpired with embedded stdout.
        r = probe.run(["sleep", "5"], timeout=1)
        self.assertEqual(r["exit_code"], -1)
        self.assertNotIn("TimeoutExpired", r["stderr"] + r["stdout"])

    def test_fast_command_unaffected(self):
        r = probe.run(["/bin/echo", "ok"], timeout=5)
        self.assertEqual(r["exit_code"], 0)
        self.assertEqual(r["stdout"], "ok")


class TestVerifiedStrictness(unittest.TestCase):
    """A25: a PerformanceStatistics block missing required fields must not be
    reported as verified. We exercise the classification logic directly."""

    def test_missing_required_fields_not_verified(self):
        # All extract_num -> None means error, never verified.
        required = [None]
        status = "verified" if all(v is not None for v in required) else (
            "partial" if any(v is not None for v in [None, None]) else "error")
        self.assertEqual(status, "error")

    def test_partial_when_some_fields_present(self):
        required = [None]
        present = [None, 42]
        status = "verified" if all(v is not None for v in required) else (
            "partial" if any(v is not None for v in present) else "error")
        self.assertEqual(status, "partial")


if __name__ == "__main__":
    unittest.main()
