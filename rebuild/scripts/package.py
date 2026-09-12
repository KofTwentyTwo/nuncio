#!/usr/bin/env python3
"""Build and verify a local production archive; never install or publish it."""

import argparse
import gzip
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path

REBUILD = Path(__file__).resolve().parents[1]
BINARIES = ("nunciod", "nuncio-cli")
TEST_FLAGS = (
    "--test-secrets-file",
    "--test-config",
    "--test-google-base-url",
    "--test-clock-ms",
    "--test-barrier",
)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


@contextmanager
def fresh_target(target: Path):
    # A stable path avoids embedded random protobuf/OpenSSL build paths. Refuse
    # existing data rather than accepting a cache or removing another build.
    target.mkdir(mode=0o700)
    try:
        yield target
    finally:
        shutil.rmtree(target)


def write_archive(root: Path, archive: Path, epoch: int):
    def normalized(info):
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        info.mtime = epoch
        info.mode = 0o755 if info.isdir() or info.name.startswith(root.name + "/bin/") else 0o644
        info.pax_headers = {}
        return info

    with archive.open("xb") as output:
        with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as bundle:
                bundle.add(root, arcname=root.name, filter=normalized)


def write_manifest(root: Path):
    entries = {
        str(path.relative_to(root)): digest(path)
        for path in sorted(root.rglob("*"))
        if path.is_file() and path != root / "SHA256.json"
    }
    (root / "SHA256.json").write_text(json.dumps(entries, indent=2) + "\n")


def verify_manifest(root: Path):
    if any(path.is_symlink() for path in root.rglob("*")):
        raise ValueError("Archive must not contain symlinks")
    entries = json.loads((root / "SHA256.json").read_text())
    actual = {
        str(path.relative_to(root)): digest(path)
        for path in root.rglob("*")
        if path.is_file() and path != root / "SHA256.json"
    }
    if entries != actual:
        raise ValueError("Archive content does not match its manifest")


def release_environment() -> dict:
    forbidden = {
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_BOOTSTRAP",
        "CARGO_BUILD_RUSTFLAGS",
    }
    if any(
        name in forbidden or name.startswith(("NUNCIO_TEST_", "CARGO_FEATURE_"))
        for name in os.environ
    ):
        raise ValueError("Remove test controls and compiler overrides before packaging")
    return dict(os.environ, CARGO_NET_OFFLINE="true")


def run(argv: list[str], env: dict) -> str:
    return subprocess.run(
        argv, cwd=REBUILD, env=env, check=True, text=True, stdout=subprocess.PIPE
    ).stdout


def check_binaries(root: Path, env: dict) -> list[dict]:
    observed = []
    with tempfile.TemporaryDirectory(prefix="nuncio-package-smoke-") as directory:
        profile = Path(directory) / "must-not-create"
        for name in BINARIES:
            binary = root / "bin" / name
            for flag in ["--help", "--version", *TEST_FLAGS]:
                command = [str(binary), flag]
                if flag in TEST_FLAGS:
                    command.append("unused")
                result = subprocess.run(
                    command,
                    cwd=directory,
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=10,
                    check=False,
                )
                expected = 2 if flag in TEST_FLAGS else 0
                if result.returncode != expected:
                    raise ValueError(
                        f"Extracted {name} {flag}: exit {result.returncode}, expected {expected}"
                    )
                observed.append(
                    {"binary": name, "argument": flag, "exit_status": result.returncode}
                )
            for variable in (
                "NUNCIO_TEST_SECRETS_FILE",
                "NUNCIO_TEST_GOOGLE_BASE_URL",
                "NUNCIO_TEST_CLOCK_MS",
                "NUNCIO_TEST_BARRIER",
            ):
                command = [str(binary), "--data-dir", str(profile)]
                if name == "nuncio-cli":
                    command += ["--json", "--endpoint", "http://127.0.0.1:1", "system", "status"]
                result = subprocess.run(
                    command,
                    cwd=directory,
                    env=dict(env, **{variable: "synthetic"}),
                    capture_output=True,
                    timeout=10,
                    check=False,
                )
                if result.returncode != (1 if name == "nunciod" else 2) or profile.exists():
                    raise ValueError(
                        f"Extracted {name} accepted test environment or accessed profile"
                    )
                observed.append(
                    {"binary": name, "variable": variable, "exit_status": result.returncode}
                )
    return observed


def notices(root: Path, metadata: dict, used: set[str]) -> list[dict]:
    inventory = []
    upstream = json.loads((REBUILD / "licenses/upstream/index.json").read_text())
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] not in used or package["source"] is None:
            continue
        source = Path(package["manifest_path"]).parent
        destination = root / "licenses" / (package["name"] + "-" + package["version"])
        found = [
            p
            for p in source.rglob("*")
            if p.is_file()
            and re.match(r"^(licen[cs]e|copying|notice)([.\-_]|$)", p.name, re.IGNORECASE)
        ]
        if package.get("license_file"):
            found.append(source / package["license_file"])
        supplied = upstream.get(package["name"] + "@" + package["version"], []) if not found else []
        if not (found or supplied) or not (package.get("license") or package.get("license_file")):
            raise ValueError(f"Missing license declaration/notice for {package['name']}")
        paths = []
        for path in sorted(set(found)):
            relative = path.relative_to(source)
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
            paths.append(str(target.relative_to(root)))
        for notice in supplied:
            path = (REBUILD / notice["path"]).resolve()
            if not path.is_relative_to(REBUILD / "licenses") or digest(path) != notice["sha256"]:
                raise ValueError("Pinned upstream license notice does not match its provenance")
            target = destination / "upstream" / path.name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
            paths.append(str(target.relative_to(root)))
        inventory.append(
            {
                "name": package["name"],
                "version": package["version"],
                "license": package.get("license"),
                "notices": paths,
                "upstream_provenance": supplied,
            }
        )
    for name, suffix in [
        ("libsqlite3-sys", "sqlcipher/LICENSE"),
        ("openssl-src", "openssl/LICENSE.txt"),
        ("ring", "LICENSE-BoringSSL"),
        ("webpki-roots", "LICENSE"),
    ]:
        if not any(
            item["name"] == name and any(p.endswith("/" + suffix) for p in item["notices"])
            for item in inventory
        ):
            raise ValueError(f"Required native/trust-data notice missing: {name}/{suffix}")
    (root / "licenses" / "INDEX.json").write_text(json.dumps(inventory, indent=2) + "\n")
    return inventory


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    env = release_environment()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    rustc = run(["rustc", "-vV"], env)
    host = next(line.split(": ", 1)[1] for line in rustc.splitlines() if line.startswith("host: "))
    version = tomllib.loads((REBUILD / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    commit = run(["git", "rev-parse", "HEAD"], env).strip()
    epoch = int(run(["git", "show", "-s", "--format=%ct", "HEAD"], env).strip())
    if not 0 <= epoch < 2**32:
        raise ValueError("Source timestamp is outside the archive format's range")
    env["SOURCE_DATE_EPOCH"] = str(epoch)
    label = f"nuncio-{version}-rc-{host}-{commit[:12]}"
    archive = output / (label + ".tar.gz")
    if archive.exists():
        raise ValueError(f"Archive already exists: {archive}; use a fresh output directory")
    metadata = json.loads(
        run(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], env)
    )
    (REBUILD / "target").mkdir(exist_ok=True)
    with (
        fresh_target(REBUILD / "target/package-production") as target,
        tempfile.TemporaryDirectory(prefix="nuncio-package-", dir=REBUILD / "target") as directory,
    ):
        temporary = Path(directory)
        root = temporary / label
        (root / "bin").mkdir(parents=True)
        build = [
            "cargo",
            "build",
            "--locked",
            "--offline",
            "--release",
            "--no-default-features",
            "-p",
            "nunciod",
            "-p",
            "nuncio-cli",
            "--target-dir",
            str(target),
            "--message-format=json",
        ]
        messages = run(build, env)
        (output / (label + "-build.jsonl")).write_text(messages)
        used = set()
        for line in messages.splitlines():
            message = json.loads(line)
            if message.get("reason") != "compiler-artifact":
                continue
            used.add(message["package_id"])
            if "test-harness" in message.get("features", []) or message["target"]["name"] in (
                "mock-google",
                "nuncio_test_support",
            ):
                raise ValueError("Test code appeared in the production build graph")
        for name in BINARIES:
            shutil.copy2(target / "release" / name, root / "bin" / name)
        descriptors = list((target / "release/build").glob("nuncio-proto-*/out/nuncio_v2.bin"))
        frozen = REBUILD / "crates/nuncio-proto/proto/nuncio.v2.bin"
        if not descriptors or any(path.read_bytes() != frozen.read_bytes() for path in descriptors):
            raise ValueError("Built API descriptor differs from the reviewed release freeze")
        shutil.copytree(REBUILD / "crates/nuncio-proto/proto", root / "api")
        shutil.copytree(
            REBUILD / "docs",
            root / "docs",
            ignore=shutil.ignore_patterns("SESSION-STATE.md", "TODO.md"),
        )
        (root / "README.md").write_text(
            "# Nuncio local candidate\n\n"
            "This archive contains the foreground mail/calendar engine and CLI. "
            "It does not install or start a service. Platform, source revision and "
            "build inputs are in `BUILD-METADATA.json`; file hashes are in `SHA256.json`.\n\n"
            "- [Package verification and setup](docs/PACKAGING.md)\n"
            "- [Operation and CLI commands](docs/RUNNING.md)\n"
            "- [Backup and recovery](docs/RECOVERY.md)\n"
            "- [API contract](docs/API.md)\n"
            "- [Provider and platform limits](docs/COMPATIBILITY.md)\n"
            "- [Separate live acceptance worksheet](docs/MANUAL-ACCEPTANCE.md)\n\n"
            "Developer test/evidence documents refer to the "
            "[source checkout](https://github.com/KofTwentyTwo/nuncio/tree/"
            "feature/nuncio-google-first-rebuild/rebuild). "
            "Live provider and native-keystore acceptance remain unverified.\n"
        )
        for name, source in [
            ("LICENSE", REBUILD.parent / "LICENSE"),
            ("Cargo.lock", REBUILD / "Cargo.lock"),
        ]:
            shutil.copy2(source, root / name)
        inventory = notices(root, metadata, used)
        build_metadata = {
            "version": version,
            "local_candidate": True,
            "host": host,
            "rustc": rustc,
            "commit": commit,
            "dirty": bool(run(["git", "status", "--porcelain"], env).strip()),
            "source_date_epoch": epoch,
            "build_directory": str(target),
            "c_compiler": run(["cc", "--version"], env),
            "features": [],
            "lock_sha256": digest(REBUILD / "Cargo.lock"),
            "third_party_packages": len(inventory),
            "live_acceptance": "unverified",
            "remote_ci": "not established by packaging",
        }
        build_metadata["source_sha256"] = {
            str(path.relative_to(REBUILD)): digest(path)
            for path in sorted((REBUILD / "crates").rglob("*"))
            if path.is_file()
        }
        (root / "BUILD-METADATA.json").write_text(json.dumps(build_metadata, indent=2) + "\n")
        write_manifest(root)
        pending_archive = temporary / archive.name
        write_archive(root, pending_archive, epoch)
        extracted = temporary / "extracted"
        with tarfile.open(pending_archive) as bundle:
            bundle.extractall(extracted, filter="data")
        extracted_root = extracted / label
        verify_manifest(extracted_root)
        observed = check_binaries(extracted_root, env)
        (output / (label + "-verification.json")).write_text(json.dumps(observed, indent=2) + "\n")
        with archive.open("xb") as destination, pending_archive.open("rb") as source:
            shutil.copyfileobj(source, destination)
        checksum = digest(archive)
        (output / (archive.name + ".sha256")).write_text(f"{checksum}  {archive.name}\n")
        print(
            json.dumps(
                {
                    "archive": str(archive),
                    "sha256": checksum,
                    "extracted_checks": len(observed),
                    "verified_at": datetime.now(timezone.utc).isoformat(),
                }
            )
        )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Packaging failed: {error}", file=sys.stderr)
        sys.exit(1)
