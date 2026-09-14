# Install and update the testing build

Run this from any directory; no checkout or local build is needed:

```sh
curl -fsSL https://raw.githubusercontent.com/KofTwentyTwo/nuncio/feature/nuncio-google-first-rebuild/install-testing.sh | bash
```

The engine does not update itself. The command above is the permanent testing
installer URL for both first installation and later updates. Keep this branch
and bootstrap file available as the testing-channel entry point when development
moves elsewhere; changing the implementation behind it must preserve this URL.

The installation always uses **`~/.local/opt/nuncio-testing`** by default.
Rerun the same command to update it. Installing the same version again succeeds.
The permanent startup commands are:

```sh
~/.local/opt/nuncio-testing/bin/nunciod --profile laptop-qa
```

The pending logging/usability build adds running activity at **info** level
and `--log-level debug`, plus readable CLI output with explicit `--json` for
scripts. The installer currently selects the earlier passing f14b02a build;
it will select a newer build only after its required CI succeeds.
[Log levels and saving output](RUNNING.md#running-logs).

In a second terminal:

```sh
~/.local/opt/nuncio-testing/bin/nuncio-cli --profile laptop-qa account add
~/.local/opt/nuncio-testing/bin/nuncio-cli --profile laptop-qa account list
```

See [guided account setup](ACCOUNT-SETUP.md) and
[individual-account commands](ACCOUNT-MANAGEMENT.md#work-on-one-account).
Google still needs Nuncio's one-time app registration before browser sign-in can
connect a real account. Named live-provider testing remains separately controlled.

## Prerequisites and custom location

Use macOS 15+, native ARM64 Python 3.11+ available as `python3`, and GitHub CLI
2.100+. Authenticate once with `gh auth login --hostname github.com`; downloads
need Actions read access. The installer reports missing prerequisites and does
not install a package manager or modify your shell configuration.

To use a different permanent location, supply the same prefix for each update:

```sh
curl -fsSL https://raw.githubusercontent.com/KofTwentyTwo/nuncio/feature/nuncio-google-first-rebuild/install-testing.sh | bash -s -- --prefix "$HOME/Applications/Nuncio-Testing"
```

Use that directory's `bin/nunciod` and `bin/nuncio-cli`. From a checked-out repo,
the equivalent is `python3 rebuild/scripts/install-testing.py --prefix PATH`.

## Updates and recovery

The installer retains each verified build in a version/source/run directory.
The `current` link selects a complete build; the permanent `bin` link points to
`current/bin`. Activation is an atomic replacement of `current`, so an update
cannot expose a partially copied build. A lock prevents concurrent installers
from activating different builds in the same prefix.

Failed downloads and copy failures preserve the active build. If activation is
interrupted, rerun the installer; it verifies an existing completed build before
reusing it. A repeat install also verifies all installed files, their permissions,
and the receipt. Unexpected files, modified builds, and unrelated objects at
`bin` or `current` are refused and preserved. Existing installations in the older
version-directory layout are reused after verification; no manual move is needed.

Stop a running daemon with Ctrl-C or authenticated `system shutdown`, then restart
it through the permanent `bin/nunciod` path. An already running process continues
using its old binary until restart. Use the permanent command paths after updates;
a shell already inside an older version directory stays in that directory.

The selected build's provenance is in `current/TESTING-INSTALL.json`. Older builds
are retained for recovery; stop the daemon before explicitly running an older
build's binaries, and check schema compatibility in [RECOVERY.md](RECOVERY.md).
Older binaries cannot downgrade a data schema. For uninstall, stop the daemon
and remove the testing application prefix only. Preserve data profiles, backups,
and Keychain records unless their deletion is separately intended.

## What the installer verifies

The Bash bootstrap fetches the Python installer from an immutable commit and
checks its SHA-256 before executing it in isolated Python mode. The installer
selects the newest retained successful push build on
`KofTwentyTwo/nuncio`'s `feature/nuncio-google-first-rebuild` branch and
`rebuild-ci.yml` workflow. Failed or unfinished builds are ineligible.

It checks the GitHub artifact digest, package checksum, complete manifest,
clean-source production metadata, 22 extracted-binary check results, and both
ARM64 executable headers. Unsafe archive paths, links, special files, changing
CI attempts and mismatched provenance are refused. Builds expire after 14 days;
if no eligible build remains, installation fails without activating anything.
The installer does not run downloaded executables, start services or access mail.

This verifies source and integrity, not developer code signing, notarization,
live-provider compatibility or native Keychain acceptance. Actual delivery receipts
and historical checks are in [VERIFICATION.md](VERIFICATION.md) and the latest
[session state](SESSION-STATE.md). No GitHub release or tag is created.

Maintainers first checkpoint a changed Python installer locally, then update the
bootstrap's immutable commit and checksum together. Run installer, bootstrap and
script checks before pushing the integrated result; verify the actual public
bootstrap and installation afterward. The bootstrap source pin is independent of
the application version it selects.
