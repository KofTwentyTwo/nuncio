"""Required CI jobs must propagate a real subprocess failure and stop."""

import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("verify", Path(__file__).with_name("verify.py"))
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


class VerifyTests(unittest.TestCase):
    def test_each_required_job_fails_when_its_suite_fails(self):
        for job, suites in verify.JOBS.items():
            with self.subTest(job=job), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                marker = root / "must-not-run"
                checks = [
                    ("setup", [sys.executable, "-c", "pass"]),
                    (suites[0] if suites else job, [sys.executable, "-c", "raise SystemExit(17)"]),
                    ("later", [sys.executable, "-c", f"open({str(marker)!r}, 'w').close()"]),
                ]
                self.assertEqual(verify.run_checks(checks, root, dict(os.environ), root), 1)
                self.assertFalse(marker.exists())
                results = json.loads((root / "results.json").read_text())
                self.assertEqual([r["exit_status"] for r in results], [0, 17])

    def test_ci_jobs_cover_every_required_suite(self):
        suites = {suite for group in verify.JOBS.values() for suite in group}
        self.assertEqual(suites, set(verify.REQUIRED_SUITES))
        self.assertEqual(len(verify.JOBS), 6)


if __name__ == "__main__":
    unittest.main()
