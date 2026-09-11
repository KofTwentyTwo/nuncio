#!/usr/bin/env python3
"""Offline rebuild verification. Required suites fail closed when absent."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
REBUILD = ROOT / "rebuild"
MANIFEST = "rebuild/Cargo.toml"
HARNESS = REBUILD / "target/test-harness"
PRODUCTION = REBUILD / "target/production"
REQUIRED_SUITES = (
    "google_mock_contract", "google_system", "google_e2e", "operation_system",
    "imap_contract", "imap_system", "imap_e2e", "recovery_e2e", "repair_system",
    "repair_e2e", "migration_e2e", "reconciliation_system", "multi_engine_system",
    "security_system", "security_e2e", "resource_system", "resource_e2e", "release_isolation",
)
FEATURES = "nunciod/test-harness,nuncio-cli/test-harness"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    choice = parser.add_mutually_exclusive_group(required=True)
    choice.add_argument("--suite")
    choice.add_argument("--all", action="store_true")
    args = parser.parse_args()
    names = REQUIRED_SUITES if args.all else (args.suite,)
    for name in names:
        if not name or not all(c.isascii() and (c.isalnum() or c == "_") for c in name):
            parser.error("suite must be a plain test target name")
        path = REBUILD / "crates/nuncio-test-support/tests" / (name + ".rs")
        if not path.is_file():
            print(f"Required suite is missing: {path}", file=sys.stderr)
            return 2

    label = "all" if args.all else args.suite
    logs = REBUILD / "test-results" / label
    logs.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    for name in list(env):
        if name.startswith(("NUNCIO_", "GOOGLE_", "GMAIL_", "SMTP_", "IMAP_")):
            env.pop(name)
    env.update({
        "RUST_TEST_THREADS": "2",
        "NUNCIO_E2E_DAEMON": str(HARNESS / "debug/nunciod"),
        "NUNCIO_E2E_CLI": str(HARNESS / "debug/nuncio-cli"),
        "NUNCIO_RELEASE_DAEMON": str(PRODUCTION / "release/nunciod"),
        "NUNCIO_RELEASE_CLI": str(PRODUCTION / "release/nuncio-cli"),
        "NUNCIO_TEST_ARTIFACTS": str(logs / "runs"),
    })
    results = []

    def run(name: str, argv: list[str]) -> bool:
        path = logs / (name + ".log")
        with path.open("w") as output:
            output.write("COMMAND: " + repr(argv) + "\n")
            output.flush()
            result = subprocess.run(argv, cwd=ROOT, env=env, stdout=output,
                                    stderr=subprocess.STDOUT, check=False)
            output.write("\nEXIT_STATUS: " + str(result.returncode) + "\n")
        results.append({"command": argv, "exit_status": result.returncode,
                        "log": str(path)})
        (logs / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(f"{name}: exit {result.returncode}; {path}", flush=True)
        if result.returncode:
            print(path.read_text()[-10000:], file=sys.stderr)
            return False
        return True

    build = ["cargo", "build", "--locked", "--manifest-path", MANIFEST,
             "-p", "nunciod", "-p", "nuncio-cli"]
    if not run("build-test-harness", build + ["--features", FEATURES,
                                             "--target-dir", str(HARNESS)]):
        return 1
    if args.all or args.suite == "release_isolation":
        if not run("build-production", build + ["--release", "--target-dir", str(PRODUCTION)]):
            return 1
    if args.all:
        checks = [
            ("fmt", ["cargo", "fmt", "--manifest-path", MANIFEST, "--all", "--", "--check"]),
            ("clippy", ["cargo", "clippy", "--locked", "--manifest-path", MANIFEST,
                        "--workspace", "--all-targets", "--", "-D", "warnings"]),
            ("clippy-test-harness", ["cargo", "clippy", "--locked", "--manifest-path", MANIFEST,
                                     "--workspace", "--all-targets", "--features", FEATURES,
                                     "--", "-D", "warnings"]),
            ("workspace", ["cargo", "test", "--locked", "--manifest-path", MANIFEST, "--workspace"]),
            ("workspace-test-harness", ["cargo", "test", "--locked", "--manifest-path", MANIFEST,
                                        "--workspace", "--features", FEATURES,
                                        "--target-dir", str(HARNESS)]),
        ]
        for name, argv in checks:
            if not run(name, argv):
                return 1
    for name in names:
        if not run(name, ["cargo", "test", "--locked", "--manifest-path", MANIFEST,
                          "-p", "nuncio-test-support", "--test", name,
                          "--target-dir", str(HARNESS)]):
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
