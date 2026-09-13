# Nuncio rebuild implementation report

The engine, authenticated API, CLI, account management, guided setup and testing
installer are implemented at signed/pushed `82a92a06eec5c6a674dde25981293404fd074b59`.
The full guided-setup offline gate passed, followed by the corrected Linux/macOS
scheduler regression. Two fresh production builds produced identical verified
Apple Silicon archives. All ten hosted rebuild jobs, seven security jobs and actual public testing
installation passed. **The full goal remains incomplete until separately authorized Google,
Synology and native-keystore acceptance passes.** Native apps remain future work.

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

Durable intent, explicit uncertainty, crash reconciliation, raw export, encrypted backup/restore and repair work through the engine/API/CLI. Tests inspect independent provider state, received bytes and send/copy/notification effects instead of inferring remote success from local records. The [R01–R16 and AM01–AM08 matrix](REQUIREMENTS.md) maps requirements to implementation and evidence.

## Actual verification

Repeated workspace and named executions are not additional unique tests. Exact commands, exit statuses and earlier failures remain in [VERIFICATION.md](VERIFICATION.md).

| Check | Observed result | Evidence under `test-results/` |
|---|---|---|
| Full guided-setup offline gate | All 35 commands exited0;328 tests per workspace configuration, zero failed/ignored | `all/results.json`; `account-setup-ux/full-second-egress.json` |
| Separate Google suites | Mock 26; system 22; actual CLI E2E 35; operation system 8; all passed | `all/`, named logs |
| Separate IMAP/SMTP suites | Contract 2; system 28; actual CLI E2E 15; all passed with independent remote effects | `all/`, named logs |
| Recovery and isolation | Recovery 5; repair 2/2; all 46 migration before/after-commit SIGKILL cases; reconciliation 3; multi-engine 1; all passed | `all/`, retained individual migration receipts |
| Security and resources | Security 3/2, 196 invalid-auth cases across 49 RPCs; resources 4/3; release isolation 2; all passed | `all/`, auth/encryption/hostile-content/resource receipts |
| Supporting checks | Independent Python server checks,34 current script regressions, formatting, both Clippy configurations, dependency and generated-client boundaries passed | `all/`; `account-setup-ux/post-ci-checks.json` |
| Scheduler correction | Controlled Linux latency RED 101 then GREEN 0; corrected complete Linux 22/macOS 22 and fmt/both Clippy pass | `account-setup-ux/linux/`; `scheduler-progress-regression.json` |
| Fresh local packages | Two clean builds; identical archives/binaries;22 extracted checks,469 manifest entries and237 notices each | `account-setup-ux/{final-package-final,final-package-commands}.json` |
| Current hosted rebuild | All ten jobs passed at 82a92a0; all ten evidence ZIP digests and command exits verified | [Run 34771537477](https://github.com/KofTwentyTwo/nuncio/actions/runs/34771537477) |
| Current hosted security | All seven jobs passed; 45 open alerts reviewed, including one new synthetic terminal-harness finding; none dismissed | [Run 34771537540](https://github.com/KofTwentyTwo/nuncio/actions/runs/34771537540) |
| Current public installer | Public curl installer selected82a92a0/run34771537477/attempt1/artifact10322850026;469 files, ARM64 headers and guided command checks passed | `account-setup-ux/installer/` |

Automated provider tests stayed offline. Parent/child egress checks denied external traffic while allowing loopback; local independent servers used synthetic accounts. Resource tests retain their original 10,000-message, large-transfer, memory and concurrency assertions. Passing mocks is not live-provider compatibility; local checks do not prove remote CI ran.

## Verified local artifact

The selected archive is under this isolated worktree's `rebuild/` directory:

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

The current hosted archive SHA256 is `9dc2d94e25ae7abc7316c453a95a6fbf5c8e9a3034e885385d587f51e9b448ef`.
It was installed from artifact10322850026, run34771537477 attempt1, into:

```text
/private/tmp/nuncio-curl-acceptance-ndt9p63p/testing prefix/nuncio-0.1.0-rc-aarch64-apple-darwin-82a92a06eec5-run-34771537477-attempt-1
```

Exact public-pipeline argv, GitHub provenance, independent manifest/header checks
and normal-environment snapshots are in
`test-results/account-setup-ux/installer/public-verification.json`. The hosted
archive uses its own recorded build environment; equality to the local archive
is not assumed. Artifacts expire after14days; the installer selects the newest
successful retained eligible build. No daemon or live/native account action ran.

## Installation, operation and recovery

For a laptop download without compiling, follow [TESTING-INSTALL.md](TESTING-INSTALL.md).
The installer selects a fully successful retained feature-branch build, validates
GitHub provenance and package integrity, and copies it into a versioned prefix.
Current-source installation passed into a fresh private temporary prefix, with
469 manifest entries and normal-environment preservation independently checked. The installer does not
start a daemon or alter PATH, normal profiles or accounts. [PACKAGING.md](PACKAGING.md)
describes local archive checks and extraction; help checks establish command
availability, while separate system/E2E suites establish behavior.

[RUNNING.md](RUNNING.md) gives first-run setup, secure credential entry, mock-only usage, action-file examples, JSON and exit semantics. [ACCOUNT-MANAGEMENT.md](ACCOUNT-MANAGEMENT.md) gives add/edit/reauth/pause/archive/restore/purge commands. The [API guide](API.md) describes the current contract. Original source/data and the rebuild's separate profile/port remain preserved.

Follow [RECOVERY.md](RECOVERY.md): preserve the original profile and keys, create an encrypted consistent backup, inspect it and restore into a new profile. Restored pending operations remain held until explicit reconciliation. After a lost acknowledgement, retain the request UUID and inspect durable receipts plus independent provider evidence. Do not repeat ambiguous SMTP delivery to repair a missing Sent copy; explicit duplicate-risk resolution is a separate decision. Account purge is logical local deletion, not secure erasure or deletion of remote data/backups.

## Dependencies, PRs and future work

The original workspace dependency/security remediation passed formatting, strict Clippy, all 790 tests, old AES-GCM/age ciphertext compatibility and a full-lock audit with fresh registry/advisory indexes. All 11 open PRs were reviewed and incorporated or superseded on the feature branch; none were merged or closed. [OPEN-PR-REPORT.md](OPEN-PR-REPORT.md) gives exact heads and proposed later dispositions. [DEPENDENCY-SECURITY-REPORT.md](DEPENDENCY-SECURITY-REPORT.md) records versions and evidence.

[SECURITY-CI.md](SECURITY-CI.md) records development triggers, full-lock scanning and actual findings. The first expanded run detected a yanked optional lock entry and nine mutable action references; both were corrected. The original 53 code alerts were individually classified; the guided setup scan adds one reviewed synthetic PTY-output alert, leaving45 open after the earlier nine fixes; reported Rust/Python credential flows were synthetic test fixtures or protected production paths, with no demonstrated production exposure in those traces. Archived reference files have documented semantic-extraction limits. No alerts were dismissed or suppressed and no remote protection settings were changed. Workflow presence does not establish mandatory merge enforcement.

[API-PUBLICATION-PLAN.md](API-PUBLICATION-PLAN.md) proposes independent SemVer contract artifacts, generated documentation and compatibility tooling; that publication pipeline is not claimed implemented. The requested [post-engine PO/PM roadmap](POST-ENGINE-ROADMAP.md) proposes Mac alpha → daily mail → daily calendar → dependable personal release after engine/CLI acceptance. Its staffing/effort assumptions are planning estimates. Native development and wider publication remain outside this goal.

## Compatibility and remaining completion condition

[COMPATIBILITY.md](COMPATIBILITY.md) records the exact protocol limits. Google Calendar supplies calendaring; Synology Calendar/CalDAV, permanent provider-message purge, reminder delivery and native apps are excluded. Independent Dovecot 2.4.5 and Mailpit 1.31.1 checks do not establish compatibility with an unknown DSM/MailPlus installation. Actual TLS, capabilities, mailbox encoding and server/client Sent policy must be recorded during acceptance. An earlier hosted Linux transfer exceeded a CLI deadline; later runs passed unchanged bounds, but that earlier timeout's cause remains unproven and its evidence is retained.

The [manual worksheet](MANUAL-ACCEPTANCE.md) still requires explicit authorization for named disposable accounts, recipients, calendars, actions and cleanup. G01–G10, S01–S05 and X01 remain unapproved/unverified: live OAuth/refresh, native keystore, remote byte fidelity, real mail/calendar writes and notifications, and MailPlus Sent behavior. James deferred those checks in favor of full mocks. No live account was accessed for rebuild acceptance; separately authorized status emails provide no provider-acceptance evidence. Merges, releases/tags, normal-environment installation and remote settings changes were not performed.

Earlier baseline checkpoints 6ff9bb9, 7390e77 and 549a598 passed their recorded hosted rebuild runs. Their schema22/42-RPC reports and b26d2c93 archive remain preserved in Git, verification history and `dist/final-candidate/EVIDENCE-before-account-management-164b021.json`; they are not relabeled as current-source evidence.
