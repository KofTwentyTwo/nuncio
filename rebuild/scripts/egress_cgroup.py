#!/usr/bin/env python3
"""Privileged cgroup operations for disposable Linux test runners."""

import argparse
import json
import os
import pwd
import re
import sys
import time
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("create", "run", "kill", "remove"))
    parser.add_argument("path", type=Path)
    args = parser.parse_args()
    payload = json.load(sys.stdin)
    uid = payload["uid"]
    environment = payload["environment"]
    if (
        sys.platform != "linux"
        or os.geteuid() != 0
        or not isinstance(uid, int)
        or uid <= 0
        or str(uid) != os.environ.get("SUDO_UID")
        or environment.get("GITHUB_ACTIONS") != "true"
        or environment.get("RUNNER_ENVIRONMENT") != "github-hosted"
    ):
        raise RuntimeError("Cgroup helper requires the unprivileged disposable-runner caller")
    if not re.fullmatch(r"/sys/fs/cgroup/NUNCIO-[1-9][0-9]*", str(args.path)):
        raise RuntimeError("Invalid owned cgroup path")
    if args.path.resolve() != args.path or not Path("/sys/fs/cgroup/cgroup.controllers").is_file():
        raise RuntimeError("A real cgroup-v2 hierarchy is required")
    if args.action == "create":
        args.path.mkdir()
    elif args.action == "run":
        (args.path / "cgroup.procs").write_text(str(os.getpid()))
        account = pwd.getpwuid(uid)
        os.initgroups(account.pw_name, account.pw_gid)
        os.setgid(account.pw_gid)
        os.setuid(uid)
        os.chdir(payload["cwd"])
        argv = payload["argv"]
        os.execvpe(argv[0], argv, environment)
    elif args.action == "kill":
        (args.path / "cgroup.kill").write_text("1")
        deadline = time.monotonic() + 10
        while "populated 1" in (args.path / "cgroup.events").read_text():
            if time.monotonic() >= deadline:
                raise RuntimeError("Owned test cgroup did not become empty")
            time.sleep(0.02)
    else:
        args.path.rmdir()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, KeyError, TypeError) as error:
        print(f"Cgroup operation failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
