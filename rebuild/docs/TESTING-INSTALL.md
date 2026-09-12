# Download a testing build on Apple Silicon

Use macOS 15 or newer, native ARM64 Python 3.11+, and GitHub CLI (`gh`) 2.100+.
Authenticate with `gh auth login --hostname github.com`; artifact downloads need
Actions read access. From this repository's root, run the command below. It
downloads the newest retained, successful **push** build from
`KofTwentyTwo/nuncio`, branch `feature/nuncio-google-first-rebuild`, workflow
`rebuild-ci.yml`; newer unfinished or failed work is not installed. No Rust,
Cargo, compiler, or local build is needed. Each install gets its own version,
source, run, and attempt directory beneath the explicit testing prefix. Existing
directories are refused and preserved; PATH, shell profiles, production binaries,
services, accounts, and data profiles are unchanged. The script prints the exact
installed path, version, full source commit, CI link, package hash, and a safe
`nuncio-cli --help` command. Its `TESTING-INSTALL.json` retains that provenance.

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
The first download requires a checkpoint push containing the archive-upload step
and a fully successful rebuild workflow; older evidence-only runs cannot supply
binaries. No GitHub release or tag is created. See GitHub's
[artifact API](https://docs.github.com/en/rest/actions/artifacts) and
[workflow-run API](https://docs.github.com/en/rest/actions/workflow-runs) for the
download provenance fields.
