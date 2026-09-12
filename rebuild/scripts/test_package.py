"""Release archive integrity and build-isolation regressions."""

import importlib.util
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "release_package", Path(__file__).with_name("package.py")
)
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class PackageTests(unittest.TestCase):
    def test_archive_ignores_output_filename_and_filesystem_timestamps(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            archives = []
            for index in (0, 1):
                root = base / str(index) / "nuncio"
                root.mkdir(parents=True)
                item = root / "README.md"
                item.write_text("identical content\n")
                item.chmod(0o600 if index == 0 else 0o644)
                os.utime(item, (100 + index, 100 + index))
                archive = base / f"output-{index}.tar.gz"
                package.write_archive(root, archive, 1234567890)
                archives.append(archive.read_bytes())
            self.assertEqual(archives[0], archives[1])

    def test_fixed_build_path_is_fresh_and_preserves_preexisting_data(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "production"
            with package.fresh_target(target) as owned:
                self.assertEqual(owned, target)
                (owned / "compiled").write_bytes(b"artifact")
            self.assertFalse(target.exists())
            target.mkdir()
            sentinel = target / "existing-data"
            sentinel.write_bytes(b"preserve")
            with self.assertRaises(FileExistsError), package.fresh_target(target):
                self.fail("must refuse existing build input")
            self.assertEqual(sentinel.read_bytes(), b"preserve")

    def test_manifest_rejects_modified_missing_and_extra_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "bin" / "nunciod"
            binary.parent.mkdir()
            binary.write_bytes(b"release-binary")
            package.write_manifest(root)
            package.verify_manifest(root)
            binary.write_bytes(b"different-binary")
            with self.assertRaises(ValueError):
                package.verify_manifest(root)
            binary.unlink()
            with self.assertRaises(ValueError):
                package.verify_manifest(root)
            binary.write_bytes(b"release-binary")
            (root / "mock-google").write_bytes(b"must-not-ship")
            with self.assertRaises(ValueError):
                package.verify_manifest(root)

    def test_release_rejects_test_and_compiler_override_environment(self):
        for name in ["NUNCIO_TEST_CONFIG", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER"]:
            with self.subTest(name=name), patch.dict(os.environ, {name: "override"}, clear=True):
                with self.assertRaises(ValueError):
                    package.release_environment()
        with patch.dict(os.environ, {"PATH": "/usr/bin"}, clear=True):
            env = package.release_environment()
            self.assertEqual(env["CARGO_NET_OFFLINE"], "true")


if __name__ == "__main__":
    unittest.main()
