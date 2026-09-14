"""Testing downloads must never install unverified or unsafe archive content."""

import copy
import fcntl
import hashlib
import importlib.util
import io
import json
import os
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from contextlib import ExitStack, redirect_stdout
from pathlib import Path
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

SCRIPT = Path(__file__).with_name("install-testing.py")
COMMIT = "a" * 40
BRANCH = "feature/nuncio-google-first-rebuild"
LABEL = "nuncio-0.1.0-rc-aarch64-apple-darwin-aaaaaaaaaaaa"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def fixture_run(run_id=42):
    return {
        "id": run_id,
        "run_number": run_id,
        "run_attempt": 1,
        "head_sha": COMMIT,
        "head_branch": BRANCH,
        "repository": {"id": 7, "full_name": "KofTwentyTwo/nuncio"},
        "head_repository": {"id": 7, "full_name": "KofTwentyTwo/nuncio"},
        "event": "push",
        "path": ".github/workflows/rebuild-ci.yml",
        "status": "completed",
        "conclusion": "success",
    }


def fixture_package(metadata_change=None, payload_change=None, member=None, checksum_bad=False):
    lock = b"synthetic lockfile"
    metadata = {
        "version": "0.1.0",
        "host": "aarch64-apple-darwin",
        "commit": COMMIT,
        "dirty": False,
        "features": [],
        "local_candidate": True,
        "lock_sha256": sha(lock),
    }
    metadata.update(metadata_change or {})
    binary = struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 2, 0, 0, 0, 0)
    files = {
        "bin/nunciod": binary,
        "bin/nuncio-cli": binary,
        "BUILD-METADATA.json": json.dumps(metadata).encode(),
        "Cargo.lock": lock,
        "README.md": b"synthetic testing package",
        "api/nuncio.v2.bin": b"synthetic descriptor",
        "licenses/INDEX.json": b"[]",
    }
    files["SHA256.json"] = json.dumps({name: sha(data) for name, data in files.items()}).encode()
    files.update(payload_change or {})
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w:gz") as bundle:
        for name, data in files.items():
            info = tarfile.TarInfo(LABEL + "/" + name)
            info.size = len(data)
            info.mode = 0o755 if name.startswith("bin/") else 0o644
            bundle.addfile(info, io.BytesIO(data))
        if member is not None:
            bundle.addfile(member, io.BytesIO(b"x" * member.size))
    package = archive.getvalue()
    checks = []
    for name in ("nunciod", "nuncio-cli"):
        for flag in (
            "--help",
            "--version",
            "--test-secrets-file",
            "--test-config",
            "--test-google-base-url",
            "--test-clock-ms",
            "--test-barrier",
        ):
            checks.append(
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
            checks.append(
                {"binary": name, "variable": variable, "exit_status": 1 if name == "nunciod" else 2}
            )
    return {
        LABEL + ".tar.gz": package,
        LABEL + ".tar.gz.sha256": (
            ("0" * 64 if checksum_bad else sha(package)) + "  " + LABEL + ".tar.gz\n"
        ).encode(),
        LABEL + "-verification.json": json.dumps(checks).encode(),
    }


def fixture_zip(files):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as bundle:
        for name, data in files.items():
            bundle.writestr(name, data)
    return output.getvalue()


class GitHubProcess:
    """Only the external gh process is stubbed; archive/filesystem code is real."""

    def __init__(self, blob, run_id=42):
        self.blob = blob
        self.runs = [fixture_run(run_id)]
        self.refreshed = fixture_run(run_id)
        self.artifact = {
            "id": 9,
            "name": f"nuncio-testing-aarch64-apple-darwin-{COMMIT}-attempt-1",
            "expired": False,
            "size_in_bytes": len(blob),
            "digest": "sha256:" + sha(blob),
            "workflow_run": {
                "id": run_id,
                "repository_id": 7,
                "head_repository_id": 7,
                "head_sha": COMMIT,
                "head_branch": BRANCH,
            },
        }
        self.downloaded = False
        self.failed = False

    def __call__(self, command, **kwargs):
        if self.failed:
            return subprocess.CompletedProcess(command, 1, "private-token", "private-token")
        if command == ["gh", "--version"]:
            data = "gh version 2.100.0 (2026-09-03)\n"
        elif command == ["gh", "auth", "status", "--hostname", "github.com"]:
            data = ""
        else:
            if command[:2] != ["gh", "api"] or "--hostname" not in command:
                raise AssertionError(f"Unexpected external command: {command}")
            if command[command.index("--hostname") + 1] != "github.com":
                raise AssertionError("Installer must not use another GitHub host")
            endpoint = next(arg for arg in command if arg.startswith("repos/"))
            url = urlsplit(endpoint)
            if url.path.endswith("/workflows/rebuild-ci.yml/runs"):
                query = parse_qs(url.query)
                if query.get("branch") != [BRANCH] or query.get("event") != ["push"]:
                    raise AssertionError("Run query must select the testing branch push workflow")
                data = json.dumps([{"workflow_runs": self.runs}])
            elif url.path.endswith(f"/runs/{self.refreshed['id']}/artifacts"):
                data = json.dumps([{"artifacts": [self.artifact]}])
            elif url.path.endswith("/runs/43/artifacts"):
                data = json.dumps([{"artifacts": []}])
            elif url.path.endswith("/artifacts/9/zip"):
                kwargs["stdout"].write(self.blob)
                self.downloaded = True
                return subprocess.CompletedProcess(command, 0)
            elif url.path.endswith(f"/runs/{self.refreshed['id']}"):
                data = json.dumps(self.refreshed)
            else:
                raise AssertionError(f"Unexpected GitHub endpoint: {endpoint}")
        return subprocess.CompletedProcess(command, 0, data, "")


class InstallerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("install_testing", SCRIPT)
        cls.installer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.installer)

    def install(self, root, github):
        with ExitStack() as stack:
            stack.enter_context(patch.object(self.installer.subprocess, "run", github))
            stack.enter_context(
                patch.object(self.installer.platform, "system", return_value="Darwin")
            )
            stack.enter_context(
                patch.object(self.installer.platform, "machine", return_value="arm64")
            )
            stack.enter_context(
                patch.object(self.installer.platform, "mac_ver", return_value=("15.7", (), ""))
            )
            stack.enter_context(redirect_stdout(io.StringIO()))
            return self.installer.install(root / "testing")

    def test_help_works_without_network_or_build_tools(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--help"], capture_output=True, text=True, check=False
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--prefix", result.stdout)

    def test_verified_download_uses_stable_paths_and_repeat_install_is_safe(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            production = root / "bin" / "nunciod"
            production.parent.mkdir()
            production.write_bytes(b"preserve production")
            github = GitHubProcess(fixture_zip(fixture_package()))
            destination = self.install(root, github)
            self.assertEqual(destination, root / "testing")
            self.assertTrue(os.access(destination / "bin/nunciod", os.X_OK))
            self.assertEqual(production.read_bytes(), b"preserve production")
            receipt = json.loads((destination / "current/TESTING-INSTALL.json").read_text())
            self.assertEqual(receipt["commit"], COMMIT)
            self.assertEqual(receipt["run_id"], 42)
            self.assertEqual(receipt["artifact_id"], 9)
            self.assertEqual(receipt["artifact_sha256"], sha(github.blob))
            self.assertEqual(destination.stat().st_mode & 0o777, 0o700)
            selected = (destination / "current").resolve()
            self.assertEqual(os.readlink(destination / "bin"), "current/bin")
            self.assertEqual(selected.parent, destination.resolve())
            before = (selected / "TESTING-INSTALL.json").read_bytes()
            self.assertEqual(self.install(root, github), destination)
            self.assertEqual((destination / "current").resolve(), selected)
            self.assertEqual((selected / "TESTING-INSTALL.json").read_bytes(), before)
            self.assertEqual(
                {p.name for p in destination.iterdir()},
                {selected.name, "current", "bin", ".install.lock"},
            )

    def test_update_changes_stable_commands_and_preserves_older_builds_and_profiles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = GitHubProcess(fixture_zip(fixture_package()))
            prefix = self.install(root, first)
            previous = (prefix / "current").resolve()
            profile = root / "profile"
            profile.mkdir()
            (profile / "keep").write_bytes(b"account data")
            self.assertEqual(
                self.install(root, GitHubProcess(fixture_zip(fixture_package()), run_id=44)), prefix
            )
            current = (prefix / "current").resolve()
            self.assertNotEqual(current, previous)
            self.assertEqual((prefix / "bin/nuncio-cli").resolve(), current / "bin/nuncio-cli")
            self.assertEqual(
                json.loads((current / "TESTING-INSTALL.json").read_text())["run_id"], 44
            )
            self.assertEqual(
                json.loads((previous / "TESTING-INSTALL.json").read_text())["run_id"], 42
            )
            self.assertEqual((profile / "keep").read_bytes(), b"account data")

    def test_legacy_versioned_install_is_verified_before_becoming_current(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            github = GitHubProcess(fixture_zip(fixture_package()))
            prefix = self.install(root, github)
            selected = (prefix / "current").resolve()
            (prefix / "current").unlink()
            (prefix / "bin").unlink()
            self.assertEqual(self.install(root, github), prefix)
            self.assertEqual((prefix / "current").resolve(), selected)
            (selected / "README.md").write_text("local change")
            with self.assertRaisesRegex(ValueError, "existing|Existing"):
                self.install(root, github)
            self.assertEqual((selected / "README.md").read_text(), "local change")
            self.assertEqual((prefix / "current").resolve(), selected)

    def test_failed_activation_keeps_old_commands_and_retry_activates_complete_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prefix = self.install(root, GitHubProcess(fixture_zip(fixture_package())))
            selected = (prefix / "current").resolve()
            newer = GitHubProcess(fixture_zip(fixture_package()), run_id=44)
            with patch.object(
                self.installer.os, "replace", side_effect=OSError("activation failed")
            ):
                with self.assertRaisesRegex(OSError, "activation failed"):
                    self.install(root, newer)
            self.assertEqual((prefix / "current").resolve(), selected)
            self.assertEqual((prefix / "bin/nuncio-cli").resolve(), selected / "bin/nuncio-cli")
            self.assertEqual(self.install(root, newer), prefix)
            self.assertNotEqual((prefix / "current").resolve(), selected)
            self.assertTrue((selected / "bin/nuncio-cli").exists())

    def test_bad_update_preserves_the_active_installation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prefix = self.install(root, GitHubProcess(fixture_zip(fixture_package())))
            selected = (prefix / "current").resolve()
            with self.assertRaises(ValueError):
                self.install(
                    root, GitHubProcess(fixture_zip(fixture_package(checksum_bad=True)), 44)
                )
            self.assertEqual((prefix / "current").resolve(), selected)
            self.assertTrue((prefix / "bin/nunciod").is_file())

    def test_unmanaged_activation_paths_are_preserved(self):
        for name, target in [
            ("current", None),
            ("current", "../outside"),
            ("bin", None),
            ("bin", "../outside"),
            (".install.lock", "../outside"),
        ]:
            with self.subTest(name=name, target=target), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                prefix = root / "testing"
                prefix.mkdir()
                outside = root / "outside"
                outside.write_bytes(b"preserve")
                path = prefix / name
                if target is None:
                    path.mkdir()
                    (path / "keep").write_bytes(b"preserve")
                else:
                    path.symlink_to(target)
                with self.assertRaises((ValueError, OSError)):
                    self.install(root, GitHubProcess(fixture_zip(fixture_package())))
                self.assertEqual(outside.read_bytes(), b"preserve")
                if target is None:
                    self.assertEqual((path / "keep").read_bytes(), b"preserve")
                else:
                    self.assertEqual(os.readlink(path), target)

    def test_another_install_holds_the_lock_without_changing_current(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prefix = self.install(root, GitHubProcess(fixture_zip(fixture_package())))
            selected = (prefix / "current").resolve()
            with (prefix / ".install.lock").open("r+") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                with self.assertRaisesRegex(ValueError, "installer|installation"):
                    self.install(root, GitHubProcess(fixture_zip(fixture_package()), 44))
            self.assertEqual((prefix / "current").resolve(), selected)

    def test_refuses_unsupported_platforms_before_downloading(self):
        for system, machine, version in (
            ("Linux", "aarch64", ""),
            ("Darwin", "x86_64", "15.0"),
            ("Darwin", "arm64", "14.7"),
        ):
            with (
                self.subTest(system=system, machine=machine, version=version),
                ExitStack() as stack,
            ):
                stack.enter_context(
                    patch.object(self.installer.platform, "system", return_value=system)
                )
                stack.enter_context(
                    patch.object(self.installer.platform, "machine", return_value=machine)
                )
                stack.enter_context(
                    patch.object(self.installer.platform, "mac_ver", return_value=(version, (), ""))
                )
                with self.assertRaisesRegex(ValueError, "macOS 15.*Apple Silicon"):
                    self.installer.check_host()

    def test_bad_package_inputs_never_leave_an_install(self):
        cases = {
            "checksum": fixture_package(checksum_bad=True),
            "manifest": fixture_package(payload_change={"README.md": b"tampered"}),
            "wrong-host": fixture_package(metadata_change={"host": "x86_64-apple-darwin"}),
            "wrong-commit": fixture_package(metadata_change={"commit": "b" * 40}),
            "dirty": fixture_package(metadata_change={"dirty": True}),
            "test-features": fixture_package(metadata_change={"features": ["test-harness"]}),
            "invalid-tar": {**fixture_package(), LABEL + ".tar.gz": b"invalid"},
        }
        wrong_cpu = fixture_package()
        # Rehash the manifest too, so only the native binary architecture check catches this.
        native = struct.pack("<8I", 0xFEEDFACF, 0x01000007, 0, 2, 0, 0, 0, 0)
        with tarfile.open(fileobj=io.BytesIO(wrong_cpu[LABEL + ".tar.gz"])) as tar:
            manifest = json.load(tar.extractfile(LABEL + "/SHA256.json"))
        manifest["bin/nunciod"] = sha(native)
        cases["wrong-native-arch"] = fixture_package(
            payload_change={
                "bin/nunciod": native,
                "SHA256.json": json.dumps(manifest).encode(),
            }
        )
        for name, files in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with self.assertRaises((ValueError, tarfile.TarError)):
                    self.install(root, GitHubProcess(fixture_zip(files)))
                self.assertEqual(list((root / "testing").glob("*")), [])

    def test_tar_traversal_links_duplicates_and_special_files_are_rejected(self):
        for name, kind in (
            (LABEL + "/../../escaped", tarfile.REGTYPE),
            ("/tmp/escaped", tarfile.REGTYPE),
            (LABEL + "/link", tarfile.SYMTYPE),
            (LABEL + "/hard", tarfile.LNKTYPE),
            (LABEL + "/fifo", tarfile.FIFOTYPE),
            (LABEL + "/README.md", tarfile.REGTYPE),
            (LABEL + "/readme.md", tarfile.REGTYPE),
            (LABEL + "/bin/../escape", tarfile.REGTYPE),
        ):
            member = tarfile.TarInfo(name)
            member.type = kind
            member.linkname = "../../escaped" if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE) else ""
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with self.assertRaises(ValueError):
                    self.install(root, GitHubProcess(fixture_zip(fixture_package(member=member))))
                self.assertFalse((root / "escaped").exists())
                self.assertEqual(list((root / "testing").glob("*")), [])

    def test_rejects_unsafe_zip_and_digest_mismatch(self):
        valid = fixture_package()
        for blob, corrupt_digest in (
            (fixture_zip({**valid, "../escape": b"x"}), False),
            (b"not a zip archive", False),
            (fixture_zip(valid), True),
        ):
            with (
                self.subTest(corrupt_digest=corrupt_digest),
                tempfile.TemporaryDirectory() as directory,
            ):
                github = GitHubProcess(blob)
                if corrupt_digest:
                    github.artifact["digest"] = "sha256:" + "0" * 64
                with self.assertRaises((ValueError, zipfile.BadZipFile)):
                    self.install(Path(directory), github)
                self.assertFalse((Path(directory) / "escape").exists())

    def test_malformed_tar_and_failed_or_missing_package_checks_are_rejected(self):
        malformed = fixture_package()
        malformed[LABEL + ".tar.gz"] = b"not gzip or tar"
        malformed[LABEL + ".tar.gz.sha256"] = (
            sha(b"not gzip or tar") + "  " + LABEL + ".tar.gz\n"
        ).encode()
        failed = fixture_package()
        checks = json.loads(failed[LABEL + "-verification.json"])
        checks[0]["exit_status"] = 1
        failed[LABEL + "-verification.json"] = json.dumps(checks).encode()
        missing = fixture_package()
        del missing[LABEL + "-verification.json"]
        for files in (malformed, failed, missing):
            with self.subTest(files=list(files)), tempfile.TemporaryDirectory() as directory:
                with self.assertRaises((ValueError, tarfile.TarError)):
                    self.install(Path(directory), GitHubProcess(fixture_zip(files)))
                self.assertFalse((Path(directory) / "testing").exists())

    def test_symlink_or_shared_prefix_is_refused_and_existing_content_is_preserved(self):
        for symlink in (False, True):
            with self.subTest(symlink=symlink), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                prefix = root / "testing"
                if symlink:
                    shared = root / "production"
                    shared.mkdir()
                    prefix.symlink_to(shared, target_is_directory=True)
                else:
                    prefix.mkdir()
                    prefix.chmod(0o777)
                (prefix / "keep").write_bytes(b"preserved")
                with self.assertRaises(ValueError):
                    self.install(root, GitHubProcess(fixture_zip(fixture_package())))
                self.assertEqual(list(prefix.iterdir()), [prefix / "keep"])
                self.assertEqual((prefix / "keep").read_bytes(), b"preserved")

    def test_interrupted_copy_cleans_only_its_new_install(self):
        def fail_copy(_source, destination, **_kwargs):
            (destination / "partial").write_bytes(b"partial")
            raise OSError("synthetic disk failure")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prefix = root / "testing"
            prefix.mkdir()
            sentinel = prefix / "previous-build"
            sentinel.write_bytes(b"preserved")
            with patch.object(self.installer.shutil, "copytree", fail_copy):
                with self.assertRaisesRegex(OSError, "synthetic disk failure"):
                    self.install(root, GitHubProcess(fixture_zip(fixture_package())))
            self.assertEqual({p.name for p in prefix.iterdir()}, {sentinel.name, ".install.lock"})
            self.assertEqual(sentinel.read_bytes(), b"preserved")

    def test_refuses_unknown_failed_fork_or_rerun_provenance(self):
        changes = (
            {"conclusion": "failure"},
            {"status": "in_progress"},
            {"event": "pull_request"},
            {"head_branch": "dev"},
            {"path": ".github/workflows/other.yml"},
            {"head_repository": {"id": 8, "full_name": "other/nuncio"}},
        )
        for change in changes:
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                github = GitHubProcess(fixture_zip(fixture_package()))
                github.runs[0].update(change)
                with self.assertRaises(ValueError):
                    self.install(Path(directory), github)
                self.assertFalse(github.downloaded)
        for mutation in ("commit", "artifact-run", "rerun", "missing-digest"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                github = GitHubProcess(fixture_zip(fixture_package()))
                if mutation == "commit":
                    github.artifact["workflow_run"]["head_sha"] = "b" * 40
                elif mutation == "artifact-run":
                    github.artifact["workflow_run"]["id"] = 41
                elif mutation == "rerun":
                    github.refreshed["run_attempt"] = 2
                else:
                    del github.artifact["digest"]
                with self.assertRaises(ValueError):
                    self.install(Path(directory), github)
                self.assertEqual(list((Path(directory) / "testing").glob("*")), [])

    def test_no_retained_successful_artifact_reports_unavailable_without_installing(self):
        for expired in (False, True):
            with self.subTest(expired=expired), tempfile.TemporaryDirectory() as directory:
                github = GitHubProcess(fixture_zip(fixture_package()))
                if expired:
                    github.artifact["expired"] = True
                else:
                    github.runs = []
                with self.assertRaisesRegex(ValueError, "No.*testing build"):
                    self.install(Path(directory), github)
                self.assertFalse(github.downloaded)

    def test_latest_successful_source_is_selected_even_if_response_is_unordered(self):
        with tempfile.TemporaryDirectory() as directory:
            github = GitHubProcess(fixture_zip(fixture_package()))
            older = copy.deepcopy(github.runs[0])
            older["id"] = older["run_number"] = 41
            github.runs.insert(0, older)
            destination = self.install(Path(directory), github)
            self.assertEqual(
                json.loads((destination / "current/TESTING-INSTALL.json").read_text())["run_id"], 42
            )

    def test_newer_evidence_only_run_does_not_hide_last_retained_package(self):
        with tempfile.TemporaryDirectory() as directory:
            github = GitHubProcess(fixture_zip(fixture_package()))
            github.runs.insert(0, fixture_run(43))
            destination = self.install(Path(directory), github)
            self.assertEqual(
                json.loads((destination / "current/TESTING-INSTALL.json").read_text())["run_id"], 42
            )

    def test_gh_failures_do_not_echo_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            github = GitHubProcess(b"")
            github.failed = True
            with self.assertRaises(ValueError) as caught:
                self.install(Path(directory), github)
            self.assertNotIn("private-token", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
