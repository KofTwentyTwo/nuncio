"""Exercise the actual standalone mock process over private control pipes."""

import base64
import imaplib
import json
import selectors
import smtplib
import ssl
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from services import run_directory


class StandaloneMock(unittest.TestCase):
    def test_sigterm_during_backend_reset_finishes_reset_then_cleans_owned_containers(self):
        here = Path(__file__).resolve().parent
        runs = run_directory()
        directory = Path(tempfile.mkdtemp(prefix="signal-", dir=runs)) / "services"
        with (directory.parent / "stderr.log").open("wb") as errors:
            child = subprocess.Popen(
                [sys.executable, str(here / "mock-mailplus.py"), "--directory", str(directory)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=errors,
            )
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(child.stdout, selectors.EVENT_READ)
                    self.assertTrue(selector.select(40), "mock readiness timeout")
                self.assertEqual(
                    json.loads(child.stdout.readline())["ready_file"],
                    str(directory / "mock-ready.json"),
                )
                child.stdin.write(b'{"id":"reset","control":{"command":"seed"}}\n')
                child.stdin.flush()
                deadline = time.monotonic() + 20
                reset_started = False
                while time.monotonic() < deadline:
                    try:
                        commands = json.loads((directory / "commands.json").read_text())
                        reset_started = any("down" in value["command"] for value in commands)
                    except json.JSONDecodeError:
                        pass
                    if reset_started:
                        break
                    time.sleep(0.01)
                self.assertTrue(reset_started, "backend reset never began")
                child.terminate()
                self.assertEqual(child.wait(timeout=30), 0)
                identity = json.loads((directory / "project.json").read_text())
                remaining = subprocess.check_output(
                    [
                        "docker",
                        "--host",
                        identity["docker_endpoint"],
                        "ps",
                        "-a",
                        "--filter",
                        "label=com.docker.compose.project=" + identity["project"],
                        "-q",
                    ]
                )
                self.assertEqual(remaining.strip(), b"")
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)
                    subprocess.run(
                        [
                            sys.executable,
                            str(here / "services.py"),
                            "stop",
                            "--directory",
                            str(directory),
                        ],
                        check=True,
                        timeout=45,
                    )
                child.stdin.close()
                child.stdout.close()

    def test_ready_strict_controls_remote_observation_reset_and_eof_cleanup(self):
        here = Path(__file__).resolve().parent
        runs = run_directory()
        directory = Path(tempfile.mkdtemp(prefix="standalone-", dir=runs)) / "services"
        with (directory.parent / "stderr.log").open("wb") as errors:
            child = subprocess.Popen(
                [
                    sys.executable,
                    str(here / "mock-mailplus.py"),
                    "--directory",
                    str(directory),
                    "--server-auto-sent",
                ],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=errors,
            )
            try:

                def read():
                    with selectors.DefaultSelector() as selector:
                        selector.register(child.stdout, selectors.EVENT_READ)
                        self.assertTrue(selector.select(40), "mock response timeout")
                    line = child.stdout.readline()
                    self.assertTrue(line, "mock exited before response; see retained stderr.log")
                    return json.loads(line)

                first = read()
                self.assertEqual(first, {"ready_file": str(directory / "mock-ready.json")})
                ready = json.loads(Path(first["ready_file"]).read_text())
                self.assertEqual(ready["schema_version"], 1)
                self.assertTrue(ready["server_auto_sent"])
                self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
                credentials = Path(ready["credentials_file"])
                self.assertEqual(credentials.stat().st_mode & 0o777, 0o600)
                passwords = json.loads(credentials.read_text())
                tls = ssl.create_default_context(cafile=ready["ca_file"])

                def call(control):
                    child.stdin.write(
                        json.dumps({"id": "request", "control": control}).encode() + b"\n"
                    )
                    child.stdin.flush()
                    value = read()
                    self.assertEqual(value["id"], "request")
                    self.assertTrue(value["ok"], value)
                    return value["result"]

                inbox = {"command": "mailbox", "account": "alpha@example.test", "mailbox": "INBOX"}
                before = call(inbox)
                self.assertEqual([v["uid"] for v in before["messages"]], [1, 2])
                expected = base64.b64decode(call(dict(inbox, command="raw", uid=1))["raw_base64"])
                mail = imaplib.IMAP4_SSL(
                    "127.0.0.1", ready["ports"]["imaps"], ssl_context=tls, timeout=5
                )
                try:
                    mail.login("alpha@example.test", passwords["alpha@example.test"])
                    mail.select("INBOX")
                    self.assertEqual(mail.uid("fetch", "1", "(BODY.PEEK[])")[1][0][1], expected)
                    self.assertEqual(mail.uid("store", "1", "+FLAGS.SILENT", "(\\Seen)")[0], "OK")
                    self.assertEqual(call(inbox)["messages"][0]["flags"], ["\\Seen"])
                    self.assertEqual(call({"command": "snapshot"})["requests"]["imap UID FETCH"], 1)
                    smtp = smtplib.SMTP_SSL(
                        "127.0.0.1", ready["ports"]["smtps"], context=tls, timeout=5
                    )
                    try:
                        smtp.login("alpha@example.test", passwords["alpha@example.test"])
                        self.assertEqual(
                            smtp.sendmail(
                                "alpha@example.test", ["recipient@example.test"], expected
                            ),
                            {},
                        )
                    finally:
                        smtp.close()
                    self.assertEqual(
                        call({"command": "wait_server_sent", "count": 1}),
                        {"enabled": True, "completed": 1, "failed": False},
                    )
                    self.assertEqual(len(call(dict(inbox, mailbox="Sent"))["messages"]), 1)
                    self.assertEqual(call({"command": "smtp", "transport": "tls"})["total"], 1)
                    child.stdin.write(
                        b'{"id":"bad","control":{"command":"seed","command":"seed"}}\n'
                    )
                    child.stdin.flush()
                    self.assertEqual(read(), {"id": None, "ok": False, "error": "invalid_control"})
                    self.assertEqual(call(inbox)["messages"][0]["flags"], ["\\Seen"])
                    call({"command": "seed"})
                    with self.assertRaises(imaplib.IMAP4.abort):
                        mail.noop()
                finally:
                    mail.shutdown()
                self.assertEqual(call(inbox), before)
                self.assertEqual(call({"command": "server_sent_status"})["completed"], 0)
                self.assertEqual(
                    json.loads((directory / "mock-ready.json").read_text())["ports"], ready["ports"]
                )
                self.assertEqual(json.loads(credentials.read_text()), passwords)
                child.stdin.close()
                self.assertEqual(child.wait(timeout=20), 0)
                project = ready["project"]
                remaining = subprocess.check_output(
                    [
                        "docker",
                        "--host",
                        json.loads((directory / "project.json").read_text())["docker_endpoint"],
                        "ps",
                        "-a",
                        "--filter",
                        "label=com.docker.compose.project=" + project,
                        "-q",
                    ]
                )
                self.assertEqual(remaining.strip(), b"")
                for password in passwords.values():
                    self.assertNotIn(
                        password.encode(), (directory.parent / "stderr.log").read_bytes()
                    )
            finally:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=20)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5)
                        # A forced death cannot run finally; clean only this
                        # test's persisted, validated composition identity.
                        subprocess.run(
                            [
                                sys.executable,
                                str(here / "services.py"),
                                "stop",
                                "--directory",
                                str(directory),
                            ],
                            check=True,
                            timeout=45,
                        )
                child.stdout.close()
                if not child.stdin.closed:
                    child.stdin.close()


if __name__ == "__main__":
    unittest.main()
