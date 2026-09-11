#!/usr/bin/env python3
"""Run independent mail-service contracts and retain exact local evidence."""

import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent


def run_command(command, output, *, timeout=120, env=None):
    output.write("COMMAND: " + repr(command) + "\n")
    output.flush()
    child = subprocess.Popen(
        command,
        stdout=output,
        stderr=subprocess.STDOUT,
        stdin=subprocess.DEVNULL,
        env=env,
        start_new_session=True,
    )
    try:
        status = child.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        # The process group contains only this invocation and its children.
        # Stop Docker CLI descendants too, before cleaning persisted projects.
        os.killpg(child.pid, signal.SIGTERM)
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            pass
        # A parent may exit while a descendant ignores TERM.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait(timeout=5)
        output.write("DEADLINE_EXCEEDED\n")
        status = 124
    output.write("EXIT_STATUS: " + str(status) + "\n")
    output.flush()
    return status


def cleanup(run, output):
    results = []
    for identity in sorted(run.rglob("project.json")):
        command = [
            sys.executable,
            str(HERE / "services.py"),
            "stop",
            "--directory",
            str(identity.parent),
        ]
        status = run_command(command, output, timeout=60)
        results.append({"directory": str(identity.parent), "exit_status": status})
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output", type=Path, default=HERE.parents[1] / "test-results/imap-services"
    )
    args = parser.parse_args()
    logs = args.output.resolve()
    logs.mkdir(parents=True, exist_ok=True)
    results = []
    for test in [
        "test_runner.py",
        "test_services.py",
        "test_proxy.py",
        "test_control.py",
        "test_auto_sent.py",
        "test_standalone.py",
    ]:
        name = test.removesuffix(".py")
        run = Path(tempfile.mkdtemp(prefix=name + "-", dir=logs))
        command = [sys.executable, str(HERE / test)]
        log = run / "test.log"
        env = dict(os.environ, NUNCIO_MAIL_TEST_RUNS=str(run / "services"))
        with log.open("w") as output:
            status = run_command(command, output, env=env)
            cleanups = cleanup(run, output)
        results.append(
            {"command": command, "exit_status": status, "cleanup": cleanups, "log": str(log)}
        )
        (logs / "results.json").write_text(json.dumps(results, indent=2))
        print(test + ": exit " + str(status), flush=True)
        if status or any(v["exit_status"] for v in cleanups):
            print(log.read_text()[-10000:])
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
