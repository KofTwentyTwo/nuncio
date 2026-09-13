#!/usr/bin/env python3
"""Build a synthetic registered CLI for offline browser-consent subprocess tests."""

import os
import shutil
import subprocess
from pathlib import Path

rebuild = Path(__file__).resolve().parents[1]
target = rebuild / "target/test-harness"
env = dict(os.environ, CARGO_NET_OFFLINE="true", NUNCIO_GOOGLE_CLIENT_ID="nuncio-test-client")
env.pop("NUNCIO_GOOGLE_CLIENT_SECRET", None)
subprocess.run(
    [
        "cargo",
        "build",
        "--locked",
        "--offline",
        "--target-dir",
        str(target),
        "-p",
        "nuncio-cli",
        "--features",
        "nuncio-cli/test-harness",
    ],
    cwd=rebuild,
    env=env,
    check=True,
)
shutil.copyfile(target / "debug/nuncio-cli", target / "debug/nuncio-cli-managed")
(target / "debug/nuncio-cli-managed").chmod(0o700)
