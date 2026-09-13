#!/usr/bin/env python3
"""Package a CI candidate with an optional maintainer Desktop OAuth registration."""

import os
import subprocess
import sys
import tempfile
from pathlib import Path


def main() -> int:
    env = dict(os.environ)
    registration = env.pop("NUNCIO_DESKTOP_OAUTH_JSON", "")
    if registration and (
        env.get("GITHUB_EVENT_NAME") != "push"
        or env.get("GITHUB_REF") != "refs/heads/feature/nuncio-google-first-rebuild"
    ):
        raise ValueError("Google registration is limited to the trusted testing push")
    if len(registration.encode()) > 65536:
        raise ValueError("Google registration exceeds 64 KiB")
    command = [
        sys.executable,
        "scripts/egress.py",
        "--evidence",
        "test-results/package-egress.json",
        "--",
        sys.executable,
        "scripts/package.py",
        "--output",
        "dist",
    ]
    # The downloaded registration is a build input, never a workspace artifact.
    with tempfile.TemporaryDirectory(prefix="nuncio-oauth-build-") as directory:
        if registration:
            path = Path(directory) / "desktop.json"
            with os.fdopen(
                os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w"
            ) as output:
                output.write(registration)
            command += ["--google-client-config", str(path)]
        return subprocess.call(command, env=env)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(f"Testing package failed: {error}", file=sys.stderr)
        sys.exit(1)
