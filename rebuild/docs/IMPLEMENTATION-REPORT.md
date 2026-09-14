# Nuncio rebuild implementation report

The latest verified testing download is signed/pushed
`f14b02a223ba95d95a0ce3f3e8f680910189869e`. It includes the engine/API/CLI,
account management, guided setup, individual-account help and permanent installer
paths. All ten hosted rebuild jobs, seven security jobs and actual public
installation/update checks passed at this source.

The retained reproducible local candidate is a separate build at `82a92a0`:
two fresh builds produced identical archives after the full guided-setup gate
and scheduler corrections passed. Its hashes remain identified below; they do
not describe the newer download. **The full goal remains incomplete until
separately authorized Google, Synology and native-keystore acceptance passes.**
Native apps remain future work.

## Delivered behavior

The Rust daemon owns SQLCipher storage, credentials, account isolation, synchronization and durable operations. The authenticated `nuncio.v2` loopback gRPC API has 49 RPCs and an independent CLI. Google support includes OAuth/refresh, Gmail initial/history synchronization, offline search and MIME/attachment reads, durable drafts, sending and mail mutations, Calendar discovery/synchronization, recurring-event occurrences, writes, RSVP and free/busy. MailPlus support uses IMAP/SMTP for folder/UID/flag synchronization, copy/move/trash/restore, submission and separate delivery/Sent-copy receipts. Live MailPlus compatibility is not established by the independent servers.

[Account management](ACCOUNT-MANAGEMENT.md) adds saved details and versioned edits, Google add/reauthentication and consent wait/cancel, IMAP settings/password updates, pause/resume, archive-by-default removal, restore and separately confirmed local purge. Schema 23 stores the new lifecycle metadata. Purge refuses unresolved remote-effect evidence, preserves other accounts and replay continuity, and retains failed credential cleanup for retry. Archive/restore, migration, backup and failed-deletion recovery are covered through storage, engine, API and actual CLI tests.

[Guided setup](ACCOUNT-SETUP.md) adds one `account add` command with provider choice,
email, server, hidden password, optional advanced TLS/mailbox settings and explicit
consent. Google browser sign-in can use a build-bundled Desktop registration, with
safe cancellation and rejected late callbacks. Nuncio's one-time Google app
registration has not been created; the current package reports Google sign-in
unavailable before asking for account details. No per-account credential JSON is
needed in the normal guided flow.

The daemon now has timestamped stderr [activity logs](RUNNING.md#running-logs),
defaulting to info, with `--log-level` controls. It logs safe local IDs, progress
counts and durable outcomes; even trace excludes provider wire traffic and
private content. Focused actual Google and independent IMAP/SMTP logging tests
pass; the full35-command offline gate also passed, with current-source delivery
qualification next as recorded in
[VERIFICATION.md](VERIFICATION.md). The [Google setup walkthrough](GOOGLE-SETUP.md)
uses the already-shipped local client-file option without waiting for a shared
registration build.

Durable intent, explicit uncertainty, crash reconciliation, raw export, encrypted backup/restore and repair work through the engine/API/CLI. Tests inspect independent provider state, received bytes and send/copy/notification effects instead of inferring remote success from local records. The [R01–R16 and AM01–AM08 matrix](REQUIREMENTS.md) maps requirements to implementation and evidence.

## Verification by source

Repeated workspace and named executions are not additional unique tests. Exact commands, exit statuses and earlier failures remain in [VERIFICATION.md](VERIFICATION.md).

| Check | Observed result | Evidence under `test-results/` |
|---|---|---|
| Guided-setup baseline gate (78d4d9e) | All 35 commands exited0;328 tests per workspace configuration, zero failed/ignored | `all/results.json`; `account-setup-ux/full-second-egress.json` |
| Separate Google suites | Mock 26; system 22; actual CLI E2E 35; operation system 8; all passed | `all/`, named logs |
| Separate IMAP/SMTP suites | Contract 2; system 28; actual CLI E2E 15; all passed with independent remote effects | `all/`, named logs |
| Recovery and isolation | Recovery 5; repair 2/2; all 46 migration before/after-commit SIGKILL cases; reconciliation 3; multi-engine 1; all passed | `all/`, retained individual migration receipts |
| Security and resources | Security 3/2, 196 invalid-auth cases across 49 RPCs; resources 4/3; release isolation 2; all passed | `all/`, auth/encryption/hostile-content/resource receipts |
| Supporting checks | Baseline independent Python server checks,34 script regressions, formatting, both Clippy configurations, dependency and generated-client boundaries passed | `all/`; `account-setup-ux/post-ci-checks.json` |
| Scheduler correction | Controlled Linux latency RED 101 then GREEN 0; corrected complete Linux 22/macOS 22 and fmt/both Clippy pass | `account-setup-ux/linux/`; `scheduler-progress-regression.json` |
| Retained local package pair (82a92a0) | Two clean builds; identical archives/binaries;22 extracted checks,469 manifest entries and237 notices each | `account-setup-ux/{final-package-final,final-package-commands}.json` |
| Current hosted rebuild (f14b02a) | All ten jobs passed; exact run/job results retained | [Run 34793311166](https://github.com/KofTwentyTwo/nuncio/actions/runs/34793311166); `stable-installer/hosted-delivery-final.json` |
| Current hosted security (f14b02a) | All seven jobs passed; earlier detailed finding classification remains separately recorded | [Run 34793311124](https://github.com/KofTwentyTwo/nuncio/actions/runs/34793311124) |
| Account help (7508e25) | 14 CLI/15 harness tests, all 21 action help pages, seven examples and packaged checks passed | `account-help/` |
| Stable installer (f14b02a) | 20 installer cases and 40 total script tests; Ruff/format/Bash/ShellCheck passed. Both digest-verified hosted lint archives show 40 tests and nine commands passed per platform | `stable-installer/checks.json`; `stable-installer/hosted-scripts/verification.json` |
| Current public installer (f14b02a) | Fresh/repeat/legacy-migration checks passed, followed by update in the same prefix;470 files, both ARM64 binaries/versions and eight CLI cases passed; previous build preserved | `stable-installer/installer/`; `stable-installer/update/public-verification.json` |

Automated provider tests stayed offline. Parent/child egress checks denied external traffic while allowing loopback; local independent servers used synthetic accounts. Resource tests retain their original 10,000-message, large-transfer, memory and concurrency assertions. Passing mocks is not live-provider compatibility; local checks do not prove remote CI ran.

## Retained reproducible local candidate (82a92a0)

The locally built candidate selected by `dist/final-candidate/EVIDENCE.json` is
under this isolated worktree's `rebuild/` directory. It remains preserved for its
reproducibility evidence; it is not the latest testing download:

```text
dist/account-setup-ux-final/a/nuncio-0.1.0-rc-aarch64-apple-darwin-82a92a06eec5.tar.gz
```

| Item | SHA-256 |
|---|---|
| Archive | `5dbed9e97e5ebfe694be11ba97e95b13e0076e46b1e09a04fdf71b6a8c3480da` |
| `bin/nunciod` | `e5d992a530287e18d69b42fd6cd1efe74cb39ff4222d7aa45b083fe4c0cbab00` |
| `bin/nuncio-cli` | `9dbd9a67705d48b1c4a21a2c78dde78e6f3277cb5fc35cde216cc4b5da757904` |
| Frozen current descriptor | `fa665cd63baac1ed7b5fbbfe561920231ce63c0785e5e98d960d0cc8e82a66f8` |

Metadata records clean 82a92a0, macOS ARM64, Rust 1.97.1, Apple clang 21 and source
epoch 1789320309, no test features and `google_oauth.configured=false`. Adjacent
checksum/build/verification files and `test-results/account-setup-ux/final-package-final.json`
identify both artifacts and the retained extraction. Repeatability applies to
identical recorded source/platform/compiler/SDK/build paths; hosted archives
are verified separately. Developer ID signing/notarization and live/native
acceptance remain unestablished. Packaged reports are build-time snapshots.

The earlier 164b021 archive 4a9e33a9, hosted run 34704460234 and actual temporary
installation remain preserved under `dist/final-candidate/` and
`test-results/testing-installer-current/`; they qualify that earlier software.
The public bootstrap was separately verified at 57c619e selecting c662e48, with
all 467 manifest entries and both ARM64 binaries checked. Neither earlier result
is used as proof that the new guided setup download has qualified.

## Latest verified testing download (f14b02a)

The hosted archive SHA-256 is
`1c6283dfdf16b37c8ebcf359c65387cffd1de985211fd0b645c4d645ff834f5d`.
The public installer selected artifact 10328798490, run
34793311166 attempt 1. The retained temporary prefix is:

```text
/private/tmp/nuncio-curl-acceptance-01p6bwb2/testing prefix
```

Its `current/TESTING-INSTALL.json` identifies f14b02a and the exact selected
artifact. Permanent commands are `bin/nunciod` and `bin/nuncio-cli`; their hashes
were independently verified:

| Binary | SHA-256 |
|---|---|
| `bin/nunciod` | `35ca95b1b42b7afb33ab84b6adc06c53af00566a615a57280866b4c64e9ac08b` |
| `bin/nuncio-cli` | `b5653ed76bbcfb4a55ac3dba8d06867c443b1090d9fba3e0f3bbb8b0bad96649` |

Exact public-pipeline commands, provenance, manifest/header checks and unchanged
normal-environment snapshots are in
`test-results/stable-installer/update/public-verification.json`. The same prefix
was first tested with 7508e25 for fresh/repeat/legacy-layout installation. The
subsequent update retained that previous build byte-for-byte. No daemon or live
account action ran during these installer checks.

The hosted archive has its own recorded build environment; equality to the
older local candidate is not assumed. Its metadata reports
`google_oauth.configured=false`. Artifacts expire after 14 days; the installer
selects the newest successful retained eligible build. Packaged documents are
build-time snapshots; the checkout's current report records later verification.

## Installation, operation and recovery

For a laptop download without compiling, follow [TESTING-INSTALL.md](TESTING-INSTALL.md).
The installer selects a fully successful retained feature-branch build, verifies
GitHub provenance and package integrity, then activates it through permanent
`~/.local/opt/nuncio-testing/bin/` paths. Repeat installation verifies and reuses
the existing build; earlier builds remain available for recovery. Stop and restart
the daemon through the permanent path to use an update. The installer does not
start a daemon or alter PATH, profiles or accounts. [PACKAGING.md](PACKAGING.md)
describes local archive checks and extraction; help checks establish command
availability, while separate system/E2E suites establish behavior.

[RUNNING.md](RUNNING.md) gives first-run setup, secure credential entry, mock-only usage, action-file examples, JSON and exit semantics. [ACCOUNT-MANAGEMENT.md](ACCOUNT-MANAGEMENT.md) gives add/edit/reauth/pause/archive/restore/purge commands. The [API guide](API.md) describes the current contract. Original source/data and the rebuild's separate profile/port remain preserved.

Follow [RECOVERY.md](RECOVERY.md): preserve the original profile and keys, create an encrypted consistent backup, inspect it and restore into a new profile. Restored pending operations remain held until explicit reconciliation. After a lost acknowledgement, retain the request UUID and inspect durable receipts plus independent provider evidence. Do not repeat ambiguous SMTP delivery to repair a missing Sent copy; explicit duplicate-risk resolution is a separate decision. Account purge is logical local deletion, not secure erasure or deletion of remote data/backups.

## Dependencies, PRs and future work

The original workspace dependency/security remediation passed formatting, strict Clippy, all 790 tests, old AES-GCM/age ciphertext compatibility and a full-lock audit with fresh registry/advisory indexes. All 11 open PRs were reviewed and incorporated or superseded on the feature branch; none were merged or closed. [OPEN-PR-REPORT.md](OPEN-PR-REPORT.md) gives exact heads and proposed later dispositions. [DEPENDENCY-SECURITY-REPORT.md](DEPENDENCY-SECURITY-REPORT.md) records versions and evidence.

[SECURITY-CI.md](SECURITY-CI.md) records development triggers, full-lock scanning and actual findings. The first expanded run detected a yanked optional lock entry and nine mutable action references; both were corrected. The original 53 code alerts were individually classified; the guided setup scan adds one reviewed synthetic PTY-output alert, leaving45 open at that reviewed baseline after the earlier nine fixes; reported Rust/Python credential flows were synthetic test fixtures or protected production paths, with no demonstrated production exposure in those traces. Archived reference files have documented semantic-extraction limits. No alerts were dismissed or suppressed and no remote protection settings were changed. Workflow presence does not establish mandatory merge enforcement.

[API-PUBLICATION-PLAN.md](API-PUBLICATION-PLAN.md) proposes independent SemVer contract artifacts, generated documentation and compatibility tooling; that publication pipeline is not claimed implemented. The requested [post-engine PO/PM roadmap](POST-ENGINE-ROADMAP.md) proposes Mac alpha → daily mail → daily calendar → dependable personal release after engine/CLI acceptance. Its staffing/effort assumptions are planning estimates. Native development and wider publication remain outside this goal.

## Compatibility and remaining completion condition

[COMPATIBILITY.md](COMPATIBILITY.md) records the exact protocol limits. Google Calendar supplies calendaring; Synology Calendar/CalDAV, permanent provider-message purge, reminder delivery and native apps are excluded. Independent Dovecot 2.4.5 and Mailpit 1.31.1 checks do not establish compatibility with an unknown DSM/MailPlus installation. Actual TLS, capabilities, mailbox encoding and server/client Sent policy must be recorded during acceptance. An earlier hosted Linux transfer exceeded a CLI deadline; later runs passed unchanged bounds, but that earlier timeout's cause remains unproven and its evidence is retained.

The [manual worksheet](MANUAL-ACCEPTANCE.md) still requires explicit authorization for named disposable accounts, recipients, calendars, actions and cleanup. G01–G10, S01–S05 and X01 remain unapproved/unverified: live OAuth/refresh, native keystore, remote byte fidelity, real mail/calendar writes and notifications, and MailPlus Sent behavior. James deferred those checks in favor of full mocks. No live account was accessed for rebuild acceptance; separately authorized status emails provide no provider-acceptance evidence. Merges, releases/tags, normal-environment installation and remote settings changes were not performed.

Earlier baseline checkpoints 6ff9bb9, 7390e77 and 549a598 passed their recorded hosted rebuild runs. Their schema22/42-RPC reports and b26d2c93 archive remain preserved in Git, verification history and `dist/final-candidate/EVIDENCE-before-account-management-164b021.json`; they are not relabeled as current-source evidence.
