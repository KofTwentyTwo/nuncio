#!/usr/bin/env python3
"""Download a verified macOS Apple Silicon testing build; never compile or start it."""

import argparse
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath
from urllib.parse import urlencode

REPOSITORY = "KofTwentyTwo/nuncio"
BRANCH = "feature/nuncio-google-first-rebuild"
WORKFLOW = ".github/workflows/rebuild-ci.yml"
HOST = "aarch64-apple-darwin"
API = f"repos/{REPOSITORY}/actions"
MAX_BYTES = 512 * 1024 * 1024
BINARIES = ("nunciod", "nuncio-cli")


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def check_host() -> None:
    version = platform.mac_ver()[0].split(".")[0]
    if (
        platform.system() != "Darwin"
        or platform.machine() != "arm64"
        or not version.isdigit()
        or int(version) < 15
    ):
        raise ValueError(
            "This installer requires macOS 15 or newer on Apple Silicon (native Python)."
        )


def gh(arguments: list[str], output=None) -> str:
    # Authentication stays in gh's credential store/environment. Suppress raw
    # subprocess diagnostics because debug output can contain signed URLs/tokens.
    environment = dict(os.environ, GH_PROMPT_DISABLED="1", GH_HOST="github.com")
    environment.pop("GH_DEBUG", None)
    try:
        result = subprocess.run(
            ["gh", *arguments],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=output if output is not None else subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=output is None,
            timeout=180,
            check=False,
        )
    except FileNotFoundError as error:
        raise ValueError(
            "Install GitHub CLI (gh) 2.100 or newer, then run gh auth login."
        ) from error
    except subprocess.TimeoutExpired as error:
        raise ValueError(
            "GitHub request timed out; no build was installed. Retry later."
        ) from error
    if result.returncode:
        raise ValueError(
            "GitHub CLI failed; check gh auth status --hostname github.com, "
            "network access and Actions read permission. No build was installed."
        )
    return result.stdout if output is None else ""


def github_json(endpoint: str, paginate: bool = False):
    arguments = [
        "api",
        "--hostname",
        "github.com",
        endpoint,
        "-H",
        "X-GitHub-Api-Version: 2026-03-10",
    ]
    if paginate:
        arguments += ["--paginate", "--slurp"]
    return json.loads(gh(arguments))


def valid_run(run: dict) -> bool:
    repository = run.get("repository") or {}
    head = run.get("head_repository") or {}
    return (
        run.get("status") == "completed"
        and run.get("conclusion") == "success"
        and run.get("event") == "push"
        and run.get("head_branch") == BRANCH
        and run.get("path") == WORKFLOW
        and repository.get("full_name") == REPOSITORY
        and head.get("full_name") == REPOSITORY
        and head.get("id") == repository.get("id")
        and type(repository.get("id")) is int
        and repository["id"] > 0
        and all(
            type(run.get(key)) is int and run[key] > 0
            for key in ("id", "run_number", "run_attempt")
        )
        and re.fullmatch(r"[0-9a-f]{40}", run.get("head_sha", "")) is not None
    )


def select_build() -> tuple[dict, dict]:
    query = urlencode({"branch": BRANCH, "status": "success", "event": "push", "per_page": 100})
    pages = github_json(f"{API}/workflows/rebuild-ci.yml/runs?{query}", paginate=True)
    runs = [run for page in pages for run in page["workflow_runs"] if valid_run(run)]
    for run in sorted(runs, key=lambda item: item["run_number"], reverse=True):
        name = f"nuncio-testing-{HOST}-{run['head_sha']}-attempt-{run['run_attempt']}"
        pages = github_json(f"{API}/runs/{run['id']}/artifacts?per_page=100", paginate=True)
        artifacts = [
            item
            for page in pages
            for item in page["artifacts"]
            if item.get("name") == name and item.get("expired") is False
        ]
        if not artifacts:
            continue
        if len(artifacts) != 1:
            raise ValueError("Ambiguous testing artifact; refusing to choose unverified bytes.")
        artifact = artifacts[0]
        origin = artifact.get("workflow_run") or {}
        if (
            origin.get("id") != run["id"]
            or origin.get("head_sha") != run["head_sha"]
            or origin.get("head_branch") != BRANCH
            or origin.get("repository_id") != run["repository"]["id"]
            or origin.get("head_repository_id") != run["repository"]["id"]
            or type(artifact.get("id")) is not int
            or artifact["id"] <= 0
            or type(artifact.get("size_in_bytes")) is not int
            or not 0 < artifact["size_in_bytes"] <= MAX_BYTES
            or not re.fullmatch(r"sha256:[0-9a-f]{64}", artifact.get("digest", ""))
        ):
            raise ValueError("Testing artifact has missing or mismatched source/digest provenance.")
        return run, artifact
    raise ValueError(
        "No retained successful macOS testing build is available. "
        "Wait for a testing-branch push with the archive-upload step and all CI jobs passing."
    )


def safe_path(name: str) -> PurePosixPath:
    path = PurePosixPath(name)
    if (
        not name
        or path.is_absolute()
        or "\\" in name
        or any(part in ("", ".", "..") for part in name.split("/"))
        or any(ord(character) < 32 or ord(character) == 127 for character in name)
    ):
        raise ValueError("Archive contains an unsafe path.")
    return path


def unpack_download(download: Path, directory: Path, artifact: dict, commit: str) -> Path:
    if (
        download.stat().st_size != artifact["size_in_bytes"]
        or "sha256:" + digest(download) != artifact["digest"]
    ):
        raise ValueError("Downloaded artifact SHA-256 or size does not match GitHub.")
    with zipfile.ZipFile(download) as bundle:
        members = bundle.infolist()
        names = [member.filename for member in members]
        archives = [name for name in names if name.endswith(".tar.gz")]
        if len(archives) != 1:
            raise ValueError("Testing artifact must contain exactly one package archive.")
        archive_name = archives[0]
        label = archive_name.removesuffix(".tar.gz")
        if not re.fullmatch(
            rf"nuncio-[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?"
            rf"-rc-{HOST}-{commit[:12]}",
            label,
        ):
            raise ValueError("Testing package filename has an unsupported version, host or source.")
        expected = {archive_name, archive_name + ".sha256", label + "-verification.json"}
        if len(names) != 3 or set(names) != expected:
            raise ValueError("Testing artifact has missing, duplicate or unexpected files.")
        total = 0
        for member in members:
            safe_path(member.filename)
            mode = member.external_attr >> 16
            total += member.file_size
            if (
                member.is_dir()
                or stat.S_IFMT(mode) not in (0, stat.S_IFREG)
                or total > MAX_BYTES
                or member.flag_bits & 1
            ):
                raise ValueError("Testing artifact contains an unsupported ZIP member.")
            with bundle.open(member) as source, (directory / member.filename).open("xb") as target:
                shutil.copyfileobj(source, target)
    archive = directory / archive_name
    sidecar = (directory / (archive_name + ".sha256")).read_text()
    if sidecar != f"{digest(archive)}  {archive_name}\n":
        raise ValueError("Package SHA-256 does not match its checksum sidecar.")
    check_receipt(json.loads((directory / (label + "-verification.json")).read_text()))
    extracted = directory / "extracted"
    extracted.mkdir(mode=0o700)
    seen = set()
    total = 0
    with tarfile.open(archive, "r:gz") as bundle:
        for member in bundle:
            path = safe_path(member.name)
            key = str(path).casefold()
            total += member.size
            if (
                path.parts[0] != label
                or key in seen
                or len(seen) >= 10000
                or not (member.isfile() or member.isdir())
                or member.size < 0
                or total > MAX_BYTES
            ):
                raise ValueError("Package contains duplicate, unsafe or unsupported TAR members.")
            seen.add(key)
            target = extracted.joinpath(*path.parts)
            if member.isdir():
                target.mkdir(mode=0o700, parents=True, exist_ok=True)
            else:
                target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                with bundle.extractfile(member) as source, target.open("xb") as output:
                    shutil.copyfileobj(source, output)
                target.chmod(
                    0o755
                    if path.parts[1:] in (("bin", "nunciod"), ("bin", "nuncio-cli"))
                    else 0o644
                )
    root = extracted / label
    verify_package(root, commit)
    return root


def check_receipt(observed: list) -> None:
    expected = []
    for name in BINARIES:
        for flag in (
            "--help",
            "--version",
            "--test-secrets-file",
            "--test-config",
            "--test-google-base-url",
            "--test-clock-ms",
            "--test-barrier",
        ):
            expected.append(
                {
                    "binary": name,
                    "argument": flag,
                    "exit_status": 0 if flag in ("--help", "--version") else 2,
                }
            )
        for variable in (
            "NUNCIO_TEST_SECRETS_FILE",
            "NUNCIO_TEST_GOOGLE_BASE_URL",
            "NUNCIO_TEST_CLOCK_MS",
            "NUNCIO_TEST_BARRIER",
        ):
            expected.append(
                {"binary": name, "variable": variable, "exit_status": 1 if name == "nunciod" else 2}
            )
    if observed != expected:
        raise ValueError("Package extraction checks are missing or did not pass.")


def verify_package(root: Path, commit: str) -> dict:
    manifest = json.loads((root / "SHA256.json").read_text())
    actual = {
        str(path.relative_to(root)): digest(path)
        for path in root.rglob("*")
        if path.is_file() and path != root / "SHA256.json"
    }
    if manifest != actual:
        raise ValueError("Package content does not match its SHA-256 manifest.")
    metadata = json.loads((root / "BUILD-METADATA.json").read_text())
    version = metadata.get("version", "")
    if (
        metadata.get("commit") != commit
        or metadata.get("host") != HOST
        or metadata.get("dirty") is not False
        or metadata.get("features") != []
        or metadata.get("local_candidate") is not True
        or root.name != f"nuncio-{version}-rc-{HOST}-{commit[:12]}"
        or metadata.get("lock_sha256") != digest(root / "Cargo.lock")
        or not (root / "api/nuncio.v2.bin").is_file()
        or not (root / "licenses/INDEX.json").is_file()
    ):
        raise ValueError("Package metadata does not identify the expected clean production build.")
    for name in BINARIES:
        with (root / "bin" / name).open("rb") as binary:
            header = binary.read(32)
        if len(header) != 32:
            raise ValueError("Package binary is not a macOS Apple Silicon executable.")
        magic, cpu, _, kind, *_ = struct.unpack("<8I", header)
        if (magic, cpu, kind) != (0xFEEDFACF, 0x0100000C, 2):
            raise ValueError("Package binary is not a macOS Apple Silicon executable.")
    return metadata


def install(prefix: Path) -> Path:
    check_host()
    version = re.search(r"gh version (\d+)\.(\d+)\.(\d+)", gh(["--version"]))
    if not version or tuple(map(int, version.groups())) < (2, 100, 0):
        raise ValueError("GitHub CLI (gh) 2.100 or newer is required.")
    gh(["auth", "status", "--hostname", "github.com"])
    run, artifact = select_build()
    commit = run["head_sha"]
    run_url = f"https://github.com/{REPOSITORY}/actions/runs/{run['id']}"
    print(f"Downloading testing source {commit}, CI run {run['id']} attempt {run['run_attempt']}.")
    with tempfile.TemporaryDirectory(prefix="nuncio-testing-download-") as directory:
        temporary = Path(directory)
        download = temporary / "artifact.zip"
        with download.open("xb") as output:
            gh(["api", "--hostname", "github.com", f"{API}/artifacts/{artifact['id']}/zip"], output)
        root = unpack_download(download, temporary, artifact, commit)
        fresh = github_json(f"{API}/runs/{run['id']}")
        if (
            not valid_run(fresh)
            or fresh["head_sha"] != commit
            or fresh["run_attempt"] != run["run_attempt"]
            or fresh["id"] != run["id"]
        ):
            raise ValueError("CI run changed during download; retry after its checks finish.")
        prefix = prefix.expanduser().absolute()
        if prefix.is_symlink():
            raise ValueError("Testing prefix must be a real directory, not a symlink.")
        prefix.mkdir(mode=0o700, parents=True, exist_ok=True)
        info = prefix.stat()
        if info.st_uid != os.getuid() or info.st_mode & 0o022:
            raise ValueError("Testing prefix must be owned by you and not writable by others.")
        destination = prefix / f"{root.name}-run-{run['id']}-attempt-{run['run_attempt']}"
        try:
            destination.mkdir(mode=0o700)
        except FileExistsError as error:
            raise ValueError(f"Testing installation already exists: {destination}") from error
        try:
            shutil.copytree(root, destination, dirs_exist_ok=True)
            destination.chmod(0o700)
            receipt = {
                "repository": REPOSITORY,
                "branch": BRANCH,
                "commit": commit,
                "version": json.loads((root / "BUILD-METADATA.json").read_text())["version"],
                "host": HOST,
                "run_id": run["id"],
                "run_attempt": run["run_attempt"],
                "run_url": run_url,
                "artifact_id": artifact["id"],
                "artifact_sha256": artifact["digest"].removeprefix("sha256:"),
                "package_sha256": digest(temporary / (root.name + ".tar.gz")),
                "live_acceptance": "unverified",
                "notarization": "not established",
            }
            (destination / "TESTING-INSTALL.json").write_text(json.dumps(receipt, indent=2) + "\n")
        except (OSError, ValueError):
            shutil.rmtree(destination)
            raise
    print(f"Installed testing version {receipt['version']} ({HOST}) from {commit}.")
    print(f"Successful CI: {run_url}\nPackage SHA-256: {receipt['package_sha256']}")
    print(
        f"Installation: {destination}\nInspect: {shlex.quote(str(destination / 'bin/nuncio-cli'))} --help"
    )
    return destination


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--prefix",
        type=Path,
        default=Path.home() / ".local/opt/nuncio-testing",
        help="testing-only parent directory (default: ~/.local/opt/nuncio-testing)",
    )
    args = parser.parse_args()
    install(args.prefix)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        tarfile.TarError,
        zipfile.BadZipFile,
    ) as error:
        print(f"Testing install failed: {error}", file=sys.stderr)
        sys.exit(1)
