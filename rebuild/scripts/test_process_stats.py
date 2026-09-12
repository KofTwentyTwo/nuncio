"""Verify the external sampler observes resident memory in an owned subprocess."""

import subprocess
import sys
import unittest

from process_stats import process_stats


class ProcessStatsTests(unittest.TestCase):
    def test_observes_touched_allocation_and_refuses_exited_process(self):
        program = (
            "import sys; print('ready',flush=True); sys.stdin.readline(); "
            "data=bytearray(32*1024*1024); data[:]=b'x'*len(data); "
            "print('allocated',flush=True); sys.stdin.readline()"
        )
        with subprocess.Popen(
            [sys.executable, "-c", program],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        ) as child:
            try:
                self.assertEqual(child.stdout.readline().strip(), "ready")
                before = process_stats(child.pid)
                child.stdin.write("allocate\n")
                child.stdin.flush()
                self.assertEqual(child.stdout.readline().strip(), "allocated")
                after = process_stats(child.pid)
                self.assertGreaterEqual(after["rss_kib"] - before["rss_kib"], 24 * 1024)
                self.assertGreaterEqual(after["threads"], 1)
            finally:
                child.terminate()
                child.wait(timeout=5)
        with self.assertRaises(OSError):
            process_stats(child.pid)

    def test_refuses_process_group_identifiers(self):
        for pid in (0, -1):
            with self.subTest(pid=pid), self.assertRaises(ValueError):
                process_stats(pid)


if __name__ == "__main__":
    unittest.main()
