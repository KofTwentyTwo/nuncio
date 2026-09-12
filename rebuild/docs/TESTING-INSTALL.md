# Download a testing build on Apple Silicon

Run this from any directory; no repository checkout or local build is needed:

```sh
curl -fsSL https://raw.githubusercontent.com/KofTwentyTwo/nuncio/feature/nuncio-google-first-rebuild/install-testing.sh | bash
```

Use macOS 15 or newer, native ARM64 Python 3.11+ available as `python3`, and
GitHub CLI (`gh`) 2.100+. Authenticate once with
`gh auth login --hostname github.com`; artifact downloads need Actions read
access. Missing prerequisites produce instructions without installing a package
manager or changing your environment. The default testing prefix is
`~/.local/opt/nuncio-testing`. To choose a different prefix:

```sh
curl -fsSL https://raw.githubusercontent.com/KofTwentyTwo/nuncio/feature/nuncio-google-first-rebuild/install-testing.sh | bash -s -- --prefix "$HOME/.local/opt/nuncio-testing"
```

The public Bash bootstrap downloads the reviewed Python installer from an
immutable source commit and checks its SHA-256 before executing it in isolated
Python mode. Downloads use HTTPS and a private temporary directory that is
removed on exit. The pinned installer still selects the latest eligible build;
its pin does not pin the application version. Maintainers changing the Python
installer must first checkpoint that source, then update the bootstrap commit
and checksum together; the offline pin test catches a stale checksum.

The bootstrap is currently on `feature/nuncio-google-first-rebuild`; it has not
been merged into `dev` or `main`. Its full invocation and verification receipts
are recorded in [VERIFICATION.md](VERIFICATION.md). The Python installer
downloads the newest retained, successful **push** build from
`KofTwentyTwo/nuncio`, branch `feature/nuncio-google-first-rebuild`, workflow
`rebuild-ci.yml`; newer unfinished or failed work is not installed. No Rust,
Cargo, compiler, or local build is needed. Each install gets its own version,
source, run, and attempt directory beneath the explicit testing prefix. Existing
directories are refused and preserved; PATH, shell profiles, production binaries,
services, accounts, and data profiles are unchanged. The script prints the exact
installed path, version, full source commit, CI link, package hash, and a safe
`nuncio-cli --help` command. Its `TESTING-INSTALL.json` retains that provenance.

If you already have this feature branch checked out, the equivalent command is:

```sh
python3 rebuild/scripts/install-testing.py --prefix "$HOME/.local/opt/nuncio-testing"
```

All CI jobs must finish successfully before a build is eligible. The installer
checks the GitHub ZIP digest, adjacent package checksum, complete file manifest,
clean-source production metadata, 22 extracted-binary check results, and both
ARM64 Mach-O executable headers before copying files. It refuses unsafe archive
paths, links, special files, unexpected contents, and changing CI attempts. This
establishes CI source and integrity evidence; it does **not** establish developer
code signing, notarization, live-provider acceptance, or native-keystore behavior.
It does not execute downloaded binaries or start a daemon. Use the exact installed
binary paths and the packaged [running guide](RUNNING.md) for separately chosen
testing-profile/account actions. To remove an unused build, delete only its printed
installation directory; retain profiles, backups, and Keychain entries. Choosing
older binaries is not a database-schema downgrade. Builds expire after 14 days:
if none is available, the installer reports that honestly and installs nothing.
The first actual delivery passed on September 12 at commit `164b021`, successful
[run34704460234](https://github.com/KofTwentyTwo/nuncio/actions/runs/34704460234),
attempt1. All eleven artifacts were retained and digest-checked. The unmodified
installer downloaded artifact10301603739 and installed its 467-entry package into
a fresh temporary prefix; normal-environment snapshots stayed unchanged. Separate
help/version/account-command checks passed27parser cases. Full system/E2E behavior
is recorded in [VERIFICATION.md](VERIFICATION.md); no daemon or live account was
started during installer verification. Exact package/ZIP hashes are in the
[implementation report](IMPLEMENTATION-REPORT.md). Older evidence-only runs cannot
supply binaries. No GitHub release or tag is created. See GitHub's
[artifact API](https://docs.github.com/en/rest/actions/artifacts) and
[workflow-run API](https://docs.github.com/en/rest/actions/workflow-runs) for the
download provenance fields.
