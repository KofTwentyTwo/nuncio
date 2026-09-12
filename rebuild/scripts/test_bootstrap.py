"""Exercise the curl-to-Bash bootstrap without network access or installation."""

import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BOOTSTRAP = Path(__file__).resolve().parents[2] / "install-testing.sh"
INSTALLER = Path(__file__).with_name("install-testing.py")
PAYLOAD = b"""import json, os, stat, sys
from pathlib import Path
Path(os.environ["BOOTSTRAP_RECEIPT"]).write_text(json.dumps({
    "arguments": sys.argv[1:], "path": sys.argv[0],
    "mode": stat.S_IMODE(Path(sys.argv[0]).parent.stat().st_mode),
    "isolated": sys.flags.isolated, "stdin": sys.stdin.read(),
}))
sys.exit(int(os.environ.get("PAYLOAD_EXIT", "0")))
"""


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="nuncio-bootstrap-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.scratch = self.root / "scratch"
        self.scratch.mkdir()
        self.receipt = self.root / "receipt.json"
        self.curl_log = self.root / "curl.json"
        self.payload = self.root / "payload.py"
        self.payload.write_bytes(PAYLOAD)
        self.environment = dict(
            os.environ,
            PATH=str(self.bin),
            TMPDIR=str(self.scratch),
            BOOTSTRAP_RECEIPT=str(self.receipt),
            BOOTSTRAP_CURL_LOG=str(self.curl_log),
            BOOTSTRAP_PAYLOAD=str(self.payload),
        )
        for name in ("mktemp", "rm", "shasum"):
            command = shutil.which(name)
            self.assertIsNotNone(command)
            (self.bin / name).symlink_to(command)
        self.command("gh", "#!/bin/sh\nexit 0\n")
        self.command(
            "python3",
            '#!/bin/sh\nif [ "${BOOTSTRAP_OLD_PYTHON:-}" = 1 ]; then exit 1; fi\n'
            f'exec {shlex.quote(sys.executable)} "$@"\n',
        )
        self.command(
            "curl",
            f"#!{sys.executable}\n"
            "import json, os, sys\nfrom pathlib import Path\n"
            "Path(os.environ['BOOTSTRAP_CURL_LOG']).write_text(json.dumps(sys.argv[1:]))\n"
            "target = Path(sys.argv[sys.argv.index('--output') + 1])\n"
            "target.write_bytes(Path(os.environ['BOOTSTRAP_PAYLOAD']).read_bytes())\n"
            "sys.exit(int(os.environ.get('BOOTSTRAP_CURL_EXIT', '0')))\n",
        )

    def command(self, name, text):
        path = self.bin / name
        path.write_text(text)
        path.chmod(0o700)

    def source(self):
        self.assertTrue(BOOTSTRAP.is_file(), "The curl-to-Bash entry point is missing")
        source = BOOTSTRAP.read_text()
        source, count = re.subn(
            r"installer_sha256='[a-f0-9]{64}'",
            "installer_sha256='" + hashlib.sha256(PAYLOAD).hexdigest() + "'",
            source,
        )
        self.assertEqual(count, 1)
        return source

    def run_bootstrap(self, *arguments):
        result = subprocess.run(
            ["/bin/bash", "-s", "--", *arguments],
            input=self.source(),
            text=True,
            capture_output=True,
            env=self.environment,
            timeout=10,
            cwd=self.root,
        )
        self.assertEqual(list(self.scratch.iterdir()), [], "Bootstrap left temporary files")
        return result

    def test_stdin_install_verifies_payload_forwards_arguments_and_cleans_up(self):
        arguments = ["--prefix", str(self.root / "test prefix $(literal)")]
        result = self.run_bootstrap(*arguments)
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(self.receipt.read_text())
        self.assertEqual(receipt["arguments"], arguments)
        self.assertEqual(receipt["mode"], 0o700)
        self.assertEqual(receipt["isolated"], 1)
        self.assertEqual(receipt["stdin"], "")
        self.assertFalse(Path(receipt["path"]).exists())
        command = json.loads(self.curl_log.read_text())
        for flag, value in (("--proto", "=https"), ("--proto-redir", "=https")):
            self.assertEqual(command[command.index(flag) + 1], value)
        self.assertIn("--fail", command)
        self.assertIn("--tlsv1.2", command)
        self.assertRegex(
            command[-1],
            r"^https://raw\.githubusercontent\.com/KofTwentyTwo/nuncio/"
            r"[a-f0-9]{40}/rebuild/scripts/install-testing\.py$",
        )

    def test_default_invocation_preserves_installer_defaults(self):
        result = self.run_bootstrap()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.receipt.read_text())["arguments"], [])

    def test_production_pin_matches_the_reviewed_installer(self):
        self.assertTrue(BOOTSTRAP.is_file())
        source = BOOTSTRAP.read_text()
        digest = re.search(r"installer_sha256='([a-f0-9]{64})'", source)
        self.assertIsNotNone(digest)
        self.assertEqual(digest.group(1), hashlib.sha256(INSTALLER.read_bytes()).hexdigest())
        self.assertRegex(source, r"installer_commit='[a-f0-9]{40}'")

    def test_download_failure_never_executes_partial_payload(self):
        self.environment["BOOTSTRAP_CURL_EXIT"] = "22"
        result = self.run_bootstrap()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.receipt.exists())

    def test_changed_payload_is_rejected_before_execution(self):
        self.payload.write_bytes(PAYLOAD + b"\n# changed\n")
        result = self.run_bootstrap()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum", result.stderr.lower())
        self.assertFalse(self.receipt.exists())

    def test_installer_failure_status_is_preserved(self):
        self.environment["PAYLOAD_EXIT"] = "17"
        result = self.run_bootstrap()
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertTrue(self.receipt.exists())

    def test_missing_prerequisites_stop_before_download(self):
        for name in ("python3", "gh"):
            with self.subTest(name=name):
                path = self.bin / name
                saved = path.read_bytes()
                path.unlink()
                result = self.run_bootstrap()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(name, result.stderr)
                self.assertFalse(self.curl_log.exists())
                self.command(name, saved.decode())

    def test_old_python_stops_with_actionable_message(self):
        self.environment["BOOTSTRAP_OLD_PYTHON"] = "1"
        result = self.run_bootstrap()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("3.11", result.stderr)
        self.assertFalse(self.curl_log.exists())

    def test_truncated_bootstrap_never_executes_payload(self):
        source = self.source().split('nuncio_install_testing "$@"')[0]
        result = subprocess.run(
            ["/bin/bash", "-s"],
            input=source,
            text=True,
            capture_output=True,
            env=self.environment,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.curl_log.exists())
        self.assertFalse(self.receipt.exists())


if __name__ == "__main__":
    unittest.main()
