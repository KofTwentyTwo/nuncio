"""Drive the actual CLI through a PTY; only synthetic test inputs are allowed."""

import errno
import json
import os
import pty
import re
import select
import signal
import sys
import termios
import time
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def main():
    config = json.load(sys.stdin)
    pid, master = pty.fork()
    if pid == 0:
        os.execv(config["command"][0], config["command"])
    transcript = bytearray()
    deadline = time.monotonic() + 35
    status = None
    cursor = 0
    secrets_hidden = 0

    def receive():
        nonlocal status
        if time.monotonic() > deadline:
            raise RuntimeError("CLI terminal deadline exceeded")
        if select.select([master], [], [], 0.05)[0]:
            try:
                chunk = os.read(master, 4096)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                chunk = b""
            transcript.extend(chunk)
            if len(transcript) > 1024 * 1024:
                raise RuntimeError("CLI terminal output exceeded bound")
        if status is None:
            child, value = os.waitpid(pid, os.WNOHANG)
            if child:
                status = value

    def until(marker):
        nonlocal cursor
        encoded = marker.encode()
        while encoded not in transcript[cursor:]:
            receive()
            if status is not None and encoded not in transcript[cursor:]:
                raise RuntimeError("CLI exited before expected prompt: " + marker)
        cursor = transcript.index(encoded, cursor) + len(encoded)

    try:
        for step in config["steps"]:
            until(step["prompt"])
            if step.get("secret"):
                if termios.tcgetattr(master)[3] & termios.ECHO:
                    raise RuntimeError("Password prompt left terminal echo enabled")
                secrets_hidden += 1
            if step.get("interrupt"):
                os.kill(pid, signal.SIGINT)
            else:
                os.write(master, (step["answer"] + "\n").encode())
        if config.get("google"):
            until("Complete Google consent in your browser:")
            while b"\n" not in transcript[cursor:]:
                receive()
            url = re.search(rb"http://[^\s]+", transcript[cursor:]).group().decode()
            expected = urllib.parse.urlsplit(config["google"])
            actual = urllib.parse.urlsplit(url)
            if (actual.scheme, actual.netloc) != (expected.scheme, expected.netloc):
                raise RuntimeError("Consent URL escaped independent mock")
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
            try:
                opener.open(url, timeout=3)
                raise RuntimeError("Mock consent did not redirect")
            except urllib.error.HTTPError as error:
                if error.code != 302:
                    raise
                callback = error.headers["Location"]
            parsed = urllib.parse.urlsplit(callback)
            if parsed.scheme != "http" or parsed.hostname != "127.0.0.1":
                raise RuntimeError("OAuth callback escaped local daemon")
            with opener.open(callback, timeout=3) as response:
                if response.status != 200:
                    raise RuntimeError("OAuth callback failed")
        while status is None:
            receive()
        # Drain bytes written immediately before process exit.
        for _ in range(3):
            receive()
        echo_restored = bool(termios.tcgetattr(master)[3] & termios.ECHO)
        text = transcript.decode(errors="replace")
        for step in config["steps"]:
            if step.get("secret") and step.get("answer") and step["answer"] in text:
                raise RuntimeError("Password appeared in terminal transcript")
        print(
            json.dumps(
                {
                    "exit_status": os.waitstatus_to_exitcode(status),
                    "transcript": text,
                    "secrets_hidden": secrets_hidden,
                    "echo_restored": echo_restored,
                }
            )
        )
    finally:
        if status is None:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        os.close(master)


if __name__ == "__main__":
    main()
