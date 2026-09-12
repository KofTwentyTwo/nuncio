# Local packages and operation

For a laptop download without compiling, use [TESTING-INSTALL.md](TESTING-INSTALL.md).
The current local schema-23 account/CLI package is verified at signed/pushed
`164b021`. The full 34-command gate passed; two fresh clean builds produced the
same `4a9e33a9` archive, with 22 extracted checks and 467 manifest entries each.
The selected path and full hashes are in [IMPLEMENTATION-REPORT.md](IMPLEMENTATION-REPORT.md).
Its current hosted CI/testing download also passed. Older schema-22 packages
and their receipts remain preserved; use each archive's own metadata.

Candidates are versioned local archives, not installed or public releases. Use the
archive's adjacent SHA-256 sidecar to verify it before extraction. The adjacent
`-build.jsonl` and `-verification.json` files record the build and extracted-binary
checks. For the selected local candidate, `dist/final-candidate/EVIDENCE.json`
records its exact path, archive/binary hashes, repeatability result and remaining
acceptance conditions. Preserve those files with the archive.

Inside the archive, `BUILD-METADATA.json` identifies its source state, platform and
build inputs; `SHA256.json` hashes its contents. Developer reports are dated
build-time snapshots and may describe earlier verification artifacts. Use the
archive's own metadata/manifest and adjacent evidence to identify the binaries you
are inspecting. Hosted CI and live/native-keystore acceptance are separate facts;
no additional platform or provider compatibility is inferred.

## Build prerequisites and command

Use Rust1.97.1 with Cargo, rustfmt and Clippy (rust-toolchain.toml), Python3.14,
Ruff0.16.7 and cargo-deny0.19.8. The lockfiles pin Rust dependencies and vendored
Protobuf tooling. macOS needs Xcode Command Line Tools; Linux needs a C compiler,
make, Perl and standard development headers. SQLite/SQLCipher and OpenSSL are
built from the locked vendored sources; this build does not link a separately
installed system SQLCipher. Native runtime credentials use macOS Keychain or an
available, unlocked Secret Service session on Linux. Tests use synthetic stores.

Fetch dependencies for both workspaces and the advisory database before enabling
egress denial. Pre-pull the exact Dovecot, Mailpit and HAProxy images listed in
`tests/imap/compose.yaml`; all test startup uses pull-never. Run the complete
[offline gate](TESTING.md), then from the isolated worktree root:

```sh
python3 rebuild/scripts/package.py --output rebuild/dist/final-candidate
```

Packaging reserves a fresh, exclusive `target/package-production` directory, builds only the production
daemon/CLI, rejects test features/mock artifacts, verifies the frozen v2 descriptor,
collects complete dependency/native notices, and emits a versioned tar.gz, checksum,
build JSON and extraction-test evidence. It rejects compiler/test overrides and
refuses to replace an existing archive. Downloads, installation and publication
are absent from this command. No build cache is accepted as a substitute for its
fresh production build.

For repeatable local artifacts, the build uses the current Git commit timestamp
as SOURCE_DATE_EPOCH for OpenSSL and normalized tar/gzip metadata. Actual check
times remain in the external command result; they are not mislabeled inside the
archive as source timestamps. The stable build path is recreated empty for every
invocation, then removed. An existing path is refused and preserved, preventing
cache reuse or interference with another build. After a killed packaging process,
inspect that generated path and confirm no build is active before removing its
leftover build output; profiles and credentials are unrelated to that directory.

Repeatability applies to identical source/docs and the recorded platform, toolchain,
C compiler, SDK and checkout/build path. Different operating systems, architectures
or SDKs are different build inputs; cross-platform byte equality is not claimed.
Use two fresh output directories to compare archive SHA-256 values. Controlled
full-build verification is recorded in VERIFICATION.md and adjacent build evidence.

## Inspect and run a candidate

Verify the adjacent checksum before extraction, then use a new empty destination.
On macOS, `shasum -a 256 -c ARCHIVE.tar.gz.sha256` checks the archive. On Linux,
use `sha256sum -c`. Extract with `tar -xzf ARCHIVE.tar.gz -C NEW_EMPTY_DIRECTORY`.
The package contains `bin/nunciod`, `bin/nuncio-cli`, the API descriptor/protobufs,
operating docs, `licenses/INDEX.json`, `BUILD-METADATA.json` and `SHA256.json`.
The latter hashes every packaged file except itself. Review the reported source
state, platform and archive hash before using any account credentials.

`bin/nunciod --help` and `bin/nuncio-cli --help` are safe inspection commands.
They do not initialize a profile or contact providers. Production binaries reject
test-only flags and environment controls before profile access. Passing these
checks does not prove native keystore or provider acceptance; the automated
functional tests use separate test-feature binaries against local services.

After the exact profile/account actions are authorized, start the daemon in the
foreground with `--data-dir /APPROVED/NEW/PROFILE --bind 127.0.0.1:9421`.
The data directory must be owned by the current user and private. A new profile
creates fresh SQLCipher/API keys in the OS keystore. Use the same data directory
and `--endpoint http://127.0.0.1:9421` on the CLI; `system status` is local.
Connect providers only through the named-account worksheet. The default rebuild
port9421 and profile root `~/.nuncio-rebuild` are separate from the original tool.
There is no automatic import of original application data.

Stop with authenticated `system shutdown`, Ctrl-C or SIGTERM. Keep the profile
and keystore intact across upgrades. See [RUNNING.md](RUNNING.md) for account,
mail/calendar and JSON/action-file commands, and [RECOVERY.md](RECOVERY.md) for
backup, interrupted work and new-profile restore. A transient error after send,
copy or notification dispatch is not permission to issue a new request UUID.
Inspect the durable operation and independent provider evidence first.

## Testing installation and removal

The repository [testing installer](TESTING-INSTALL.md) uses a version/source/run
directory beneath the explicit `~/.local/opt/nuncio-testing` prefix and refuses
existing destinations. It verifies CI provenance, digests, package contents and
architecture without compiling or executing the downloaded binaries. Use the
printed binary paths; it does not modify PATH, profiles, production executables
or login services. Download and temporary installation of the `164b021` CI archive
have passed the checks in [TESTING-INSTALL.md](TESTING-INSTALL.md). For a manually
extracted local candidate, likewise choose a new version-specific directory and
preserve older candidates. No normal-environment installation has been performed
here. Native application packaging is outside this goal.

For rollback, stop the daemon and choose a binary compatible with the profile's
schema, or restore a verified backup into a new directory. An older binary must
refuse a newer schema; replacing the executable is not a database downgrade.
For uninstall, stop the process and remove only the chosen installation directory
and any links deliberately created for it. Preserve profile directories, backups
and OS-keychain records until their deletion is separately authorized. Deleting
keys or losing the recovery passphrase can make retained data unreadable.
