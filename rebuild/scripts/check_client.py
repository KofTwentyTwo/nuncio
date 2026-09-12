#!/usr/bin/env python3
"""Verify production CLI, standalone client and mock dependency boundaries."""

import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    checks = [
        ("cli", "Cargo.toml", "nuncio-cli", {"nuncio-engine", "nuncio-test-support", "nunciod"}),
        (
            "external",
            "clients/smoke/Cargo.toml",
            "nuncio-api-smoke",
            {"nuncio-engine", "nuncio-test-support", "nunciod", "nuncio-cli", "nuncio-proto"},
        ),
        (
            "mock",
            "Cargo.toml",
            "nuncio-test-support",
            {"nuncio-engine", "nuncio-proto", "nunciod", "nuncio-cli"},
        ),
    ]
    evidence = []
    for label, manifest, package, forbidden in checks:
        result = subprocess.run(
            [
                "cargo",
                "tree",
                "--locked",
                "--offline",
                "--manifest-path",
                manifest,
                "-p",
                package,
                "--edges",
                "normal,build",
                "--prefix",
                "none",
                "--format",
                "{p}",
            ],
            cwd=ROOT,
            check=True,
            text=True,
            capture_output=True,
        )
        names = {line.split()[0] for line in result.stdout.splitlines() if line.strip()}
        if names & forbidden:
            raise RuntimeError(f"{label} dependency boundary violated: {sorted(names & forbidden)}")
        evidence.append(
            {
                "boundary": label,
                "packages": len(names),
                "forbidden": sorted(forbidden),
                "passed": True,
            }
        )
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
