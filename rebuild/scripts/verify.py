#!/usr/bin/env python3
"""Offline rebuild verification. Required suites fail closed when absent."""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REBUILD = ROOT / "rebuild"
MANIFEST = "rebuild/Cargo.toml"
HARNESS = REBUILD / "target/test-harness"
PRODUCTION = REBUILD / "target/production"
REQUIRED_SUITES = (
    "google_mock_contract",
    "google_system",
    "google_e2e",
    "operation_system",
    "imap_contract",
    "imap_system",
    "imap_e2e",
    "recovery_e2e",
    "repair_system",
    "repair_e2e",
    "migration_e2e",
    "reconciliation_system",
    "multi_engine_system",
    "security_system",
    "security_e2e",
    "resource_system",
    "resource_e2e",
    "release_isolation",
)
FEATURES = "nunciod/test-harness,nuncio-cli/test-harness"
JOBS = {
    "rebuild-lint": (),
    "rebuild-mock-contract": ("google_mock_contract",),
    "rebuild-system": (
        "google_system",
        "operation_system",
        "repair_system",
        "reconciliation_system",
        "multi_engine_system",
        "security_system",
    ),
    "rebuild-e2e": (
        "google_e2e",
        "recovery_e2e",
        "repair_e2e",
        "migration_e2e",
        "security_e2e",
        "resource_e2e",
    ),
    "rebuild-imap": ("imap_contract", "imap_system", "imap_e2e", "resource_system"),
    "rebuild-release-check": ("release_isolation",),
}


def run_checks(checks, cwd, env, logs) -> int:
    logs.mkdir(parents=True, exist_ok=True)
    results = []
    for name, argv in checks:
        path = logs / (name + ".log")
        with path.open("w") as output:
            output.write("COMMAND: " + repr(argv) + "\n")
            output.flush()
            result = subprocess.run(
                argv, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT, check=False
            )
            output.write("\nEXIT_STATUS: " + str(result.returncode) + "\n")
        results.append({"command": argv, "exit_status": result.returncode, "log": str(path)})
        (logs / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(f"{name}: exit {result.returncode}; {path}", flush=True)
        if result.returncode:
            print(path.read_text()[-10000:], file=sys.stderr)
            return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    choice = parser.add_mutually_exclusive_group(required=True)
    choice.add_argument("--suite")
    choice.add_argument("--all", action="store_true")
    choice.add_argument("--job", choices=tuple(JOBS))
    args = parser.parse_args()
    names = REQUIRED_SUITES if args.all else JOBS[args.job] if args.job else (args.suite,)
    for name in names:
        if not name or not all(c.isascii() and (c.isalnum() or c == "_") for c in name):
            parser.error("suite must be a plain test target name")
        path = REBUILD / "crates/nuncio-test-support/tests" / (name + ".rs")
        if not path.is_file():
            print(f"Required suite is missing: {path}", file=sys.stderr)
            return 2

    label = "all" if args.all else args.job or args.suite
    logs = REBUILD / "test-results" / label
    logs.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    for name in list(env):
        if name != "NUNCIO_ADVISORY_DB" and name.startswith(
            ("NUNCIO_", "GOOGLE_", "GMAIL_", "SMTP_", "IMAP_")
        ):
            env.pop(name)
    env.update(
        {
            "RUST_TEST_THREADS": "2",
            "NUNCIO_E2E_DAEMON": str(HARNESS / "debug/nunciod"),
            "NUNCIO_E2E_CLI": str(HARNESS / "debug/nuncio-cli"),
            "NUNCIO_RELEASE_DAEMON": str(PRODUCTION / "release/nunciod"),
            "NUNCIO_RELEASE_CLI": str(PRODUCTION / "release/nuncio-cli"),
            "NUNCIO_TEST_ARTIFACTS": str(logs / "runs"),
            "CARGO_NET_OFFLINE": "true",
        }
    )
    env.setdefault("NUNCIO_ADVISORY_DB", str(REBUILD / "test-results/advisory-db"))
    checks = []
    build = [
        "cargo",
        "build",
        "--locked",
        "--manifest-path",
        MANIFEST,
        "-p",
        "nunciod",
        "-p",
        "nuncio-cli",
    ]
    checks.append(
        ("build-test-harness", build + ["--features", FEATURES, "--target-dir", str(HARNESS)])
    )
    if args.all or "release_isolation" in names:
        checks.append(("build-production", build + ["--release", "--target-dir", str(PRODUCTION)]))
    if args.all or args.job == "rebuild-lint":
        ruff = os.environ.get("REBUILD_RUFF", "ruff")
        python_paths = ["rebuild/scripts", "rebuild/tests/imap", "rebuild/tests/egress"]
        checks += [
            (
                "python-lint",
                [ruff, "check", "--config", "rebuild/tests/imap/ruff.toml", *python_paths],
            ),
            (
                "python-fmt",
                [
                    ruff,
                    "format",
                    "--check",
                    "--config",
                    "rebuild/tests/imap/ruff.toml",
                    *python_paths,
                ],
            ),
        ]
        checks += [
            ("fmt", ["cargo", "fmt", "--manifest-path", MANIFEST, "--all", "--", "--check"]),
            (
                "clippy",
                [
                    "cargo",
                    "clippy",
                    "--locked",
                    "--manifest-path",
                    MANIFEST,
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            (
                "clippy-test-harness",
                [
                    "cargo",
                    "clippy",
                    "--locked",
                    "--manifest-path",
                    MANIFEST,
                    "--workspace",
                    "--all-targets",
                    "--features",
                    FEATURES,
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
        ]
        checks += [
            (
                "script-regressions",
                [
                    "python3",
                    "-m",
                    "unittest",
                    "discover",
                    "-s",
                    "rebuild/scripts",
                    "-p",
                    "test_*.py",
                ],
            ),
        ]
        if not args.all:
            checks.append(
                (
                    "core-tests",
                    [
                        "cargo",
                        "test",
                        "--locked",
                        "--manifest-path",
                        MANIFEST,
                        "-p",
                        "nuncio-engine",
                        "-p",
                        "nuncio-proto",
                        "-p",
                        "nunciod",
                        "-p",
                        "nuncio-cli",
                    ],
                )
            )
    if args.all:
        checks += [
            (
                "workspace",
                ["cargo", "test", "--locked", "--manifest-path", MANIFEST, "--workspace"],
            ),
            (
                "workspace-test-harness",
                [
                    "cargo",
                    "test",
                    "--locked",
                    "--manifest-path",
                    MANIFEST,
                    "--workspace",
                    "--features",
                    FEATURES,
                    "--target-dir",
                    str(HARNESS),
                ],
            ),
        ]
    if args.all or args.job == "rebuild-imap":
        checks.append(
            (
                "imap-independent-services",
                [
                    "python3",
                    "rebuild/tests/imap/run-tests.py",
                    "--output",
                    str(logs / "independent-services"),
                ],
            )
        )
    for name in names:
        checks.append(
            (
                name,
                [
                    "cargo",
                    "test",
                    "--locked",
                    "--manifest-path",
                    MANIFEST,
                    "-p",
                    "nuncio-test-support",
                    "--test",
                    name,
                    "--target-dir",
                    str(HARNESS),
                ],
            )
        )
    if args.all or args.job == "rebuild-release-check":
        external = [
            "--locked",
            "--manifest-path",
            "rebuild/clients/smoke/Cargo.toml",
            "--target-dir",
            "rebuild/target/external-client",
        ]
        checks += [
            (
                "dependency-review",
                [
                    "cargo",
                    "deny",
                    "--manifest-path",
                    MANIFEST,
                    "--locked",
                    "--offline",
                    "check",
                    "-c",
                    "rebuild/deny.toml",
                    "advisories",
                    "licenses",
                    "sources",
                ],
            ),
            (
                "external-fmt",
                [
                    "cargo",
                    "fmt",
                    "--manifest-path",
                    "rebuild/clients/smoke/Cargo.toml",
                    "--",
                    "--check",
                ],
            ),
            (
                "external-clippy",
                ["cargo", "clippy", *external, "--all-targets", "--", "-D", "warnings"],
            ),
            ("external-client", ["cargo", "test", *external, "--test", "daemon"]),
            ("client-boundary", ["python3", "rebuild/scripts/check_client.py"]),
        ]
    return run_checks(checks, ROOT, env, logs)


if __name__ == "__main__":
    sys.exit(main())
