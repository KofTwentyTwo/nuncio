import importlib.util
import sys
import tempfile
import time
import unittest
from pathlib import Path


class ContractDeadlines(unittest.TestCase):
    def test_timeout_kills_the_owned_descendant_process_and_records_failure(self):
        spec = importlib.util.spec_from_file_location(
            "runner", Path(__file__).with_name("run-tests.py")
        )
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        with tempfile.TemporaryDirectory(prefix="nuncio-runner-") as temporary:
            folder = Path(temporary)
            effect = folder / "unexpected-effect"
            started = folder / "started"
            descendant = "import signal,time,pathlib,sys;signal.signal(signal.SIGTERM,signal.SIG_IGN);pathlib.Path(sys.argv[2]).touch();time.sleep(1);pathlib.Path(sys.argv[1]).write_text('effect')"
            parent = "import subprocess,sys,time;subprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2],sys.argv[3]]);time.sleep(10)"
            with (folder / "log").open("w") as output:
                status = runner.run_command(
                    [sys.executable, "-c", parent, descendant, str(effect), str(started)],
                    output,
                    timeout=0.5,
                )
            self.assertTrue(started.exists(), "test parent never started its descendant")
            self.assertEqual(status, 124)
            self.assertIn("EXIT_STATUS: 124", (folder / "log").read_text())
            time.sleep(0.8)
            self.assertFalse(effect.exists(), "timed-out descendant survived process-group cleanup")


if __name__ == "__main__":
    unittest.main()
