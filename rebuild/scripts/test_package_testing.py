"""CI registration is restricted to trusted testing pushes and transient files."""

import importlib.util
import os
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "package_testing", Path(__file__).with_name("package-testing.py")
)
module = importlib.util.module_from_spec(spec)


class TestingPackageTests(unittest.TestCase):
    def test_optional_registration_is_private_and_removed_on_success_or_failure(self):
        spec.loader.exec_module(module)
        for outcome in (0, 7):
            for registration in ("", '{"installed":{"client_id":"synthetic"}}'):
                paths = []

                def run(argv, **kwargs):
                    self.assertNotIn("NUNCIO_DESKTOP_OAUTH_JSON", kwargs["env"])
                    if registration:
                        path = Path(argv[argv.index("--google-client-config") + 1])
                        paths.append(path)
                        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                        self.assertEqual(path.read_text(), registration)
                    else:
                        self.assertNotIn("--google-client-config", argv)
                    return outcome

                env = {
                    "GITHUB_EVENT_NAME": "push",
                    "GITHUB_REF": "refs/heads/feature/nuncio-google-first-rebuild",
                    "NUNCIO_DESKTOP_OAUTH_JSON": registration,
                }
                with (
                    patch.dict(os.environ, env, clear=True),
                    patch.object(module.subprocess, "call", side_effect=run),
                ):
                    self.assertEqual(module.main(), outcome)
                self.assertTrue(all(not p.exists() for p in paths))

    def test_registration_is_refused_for_untrusted_refs_or_events(self):
        spec.loader.exec_module(module)
        for event, ref in [
            ("pull_request", "refs/heads/feature/nuncio-google-first-rebuild"),
            ("push", "refs/heads/untrusted"),
        ]:
            env = {
                "GITHUB_EVENT_NAME": event,
                "GITHUB_REF": ref,
                "NUNCIO_DESKTOP_OAUTH_JSON": "synthetic",
            }
            with (
                patch.dict(os.environ, env, clear=True),
                patch.object(module.subprocess, "call") as run,
            ):
                with self.assertRaises(ValueError):
                    module.main()
                run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
