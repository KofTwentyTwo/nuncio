# Nuncio rebuild implementation report

The latest verified testing download is signed/pushed `779264348db7240bfc368a5a5e5650f57e9d0b80`.
It includes the complete engine/API/CLI, account management, readable output,
actionable help, detailed startup/activity logs, stable installer paths and the
Rustls patch. All ten hosted rebuild jobs, seven security jobs and actual public
fresh/update checks passed. The full 35-command local gate passed 348 tests in each
workspace. Two clean local production archives are byte-identical.

**The full goal remains incomplete until separately authorized Google, Synology
and native-keystore acceptance passes.** One-time Google app registration remains
pending. Native apps remain future work; passing mocks is not live compatibility.

Laptop feedback has since reopened initial IMAP-sync reliability and native startup
latency. Concurrent-arrival and flag-notification failures are reproduced and
corrected locally; full qualification is still in progress. The published source
above remains the last qualified download. [Current investigation](SESSION-STATE.md)
and [bounded implementation plan](PLAN-IMAP-CATCHUP.md) record this follow-up.

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
pass; detailed startup stages include schema/account counts and elapsed time.
The full 35-command gate passed 348 tests per workspace; actual hosted/public
qualification is recorded in
[VERIFICATION.md](VERIFICATION.md). The [Google setup walkthrough](GOOGLE-SETUP.md)
uses the already-shipped local client-file option without waiting for a shared
registration build.

Durable intent, explicit uncertainty, crash reconciliation, raw export, encrypted backup/restore and repair work through the engine/API/CLI. Tests inspect independent provider state, received bytes and send/copy/notification effects instead of inferring remote success from local records. The [R01–R16 and AM01–AM08 matrix](REQUIREMENTS.md) maps requirements to implementation and evidence.

## Verification by source

Repeated workspace and named executions are not additional unique tests. Exact commands, exit statuses and earlier failures remain in [VERIFICATION.md](VERIFICATION.md).

| Check | Observed result | Evidence under `test-results/` |
|---|---|---|
| Full current gate (7792643) | All 35 commands exit 0; 348 tests per workspace, zero failed/ignored | `startup-logging/patched-gate-final.json`; `startup-logging/patched-gate/` |
| Separate Google suites (7792643) | Mock 26; system 22; actual daemon/CLI E2E 41; operation system 8; all pass | `startup-logging/patched-gate/`, named logs |
| Separate IMAP/SMTP suites (7792643) | Contract 2; system 28; actual CLI E2E 16; all pass with independent effects | Same archived gate plus `all/independent-services/` |
| Recovery and isolation (7792643) | Recovery 5; repair 2/2; 46 migration crash cases; reconciliation 3; multi-engine 1 | Same archived gate and individual migration receipts |
| Security and resources (7792643) | Security 3/2, 196 invalid-auth cases across 49 RPCs; resources 4/3; release isolation 2; all pass | Same archived gate |
| Supporting checks (7792643) | Independent servers, 40 script tests, fmt, both Clippy, dependency policy and generated-client boundaries pass | Same archived gate |
| Rustls patch (7792643) | All three locks at 0.23.45; six refreshed advisory checks pass; original 790 tests/fmt/Clippy pass with egress denied | `startup-logging/rustls/` |
| Current local package pair (7792643) | Two clean identical archives; 22 extracted checks, 478 files and 241 notices each | `startup-logging/package-pair.json` |
| Scheduler correction | Controlled Linux latency RED 101 then GREEN 0; corrected complete Linux 22/macOS 22 and fmt/both Clippy pass | `account-setup-ux/linux/`; `scheduler-progress-regression.json` |
| Retained local package pair (82a92a0) | Two clean builds; identical archives/binaries;22 extracted checks,469 manifest entries and237 notices each | `account-setup-ux/{final-package-final,final-package-commands}.json` |
| Current hosted rebuild (7792643) | All ten jobs passed at the exact source | [Run 34880705357](https://github.com/KofTwentyTwo/nuncio/actions/runs/34880705357); `startup-logging/hosted-delivery-final.json` |
| Current hosted security (7792643) | All seven jobs passed; same 45 prior finding IDs/rules/paths/severities, all seen at this source | [Run 34880705426](https://github.com/KofTwentyTwo/nuncio/actions/runs/34880705426); `startup-logging/codeql-alert-comparison.json` |
| Account help (7508e25) | 14 CLI/15 harness tests, all 21 action help pages, seven examples and packaged checks passed | `account-help/` |
| Stable installer (f14b02a) | 20 installer cases and 40 total script tests; Ruff/format/Bash/ShellCheck passed. Both digest-verified hosted lint archives show 40 tests and nine commands passed per platform | `stable-installer/checks.json`; `stable-installer/hosted-scripts/verification.json` |
| Current public installer (7792643) | Fresh/update pass; 478 files, ARM64 binaries, startup preflight, 71 help pages/531 options/192 argument cases; prior build preserved | `startup-logging/installer/`; `startup-logging/update/public-verification.json` |

Automated provider tests stayed offline. Parent/child egress checks denied external traffic while allowing loopback; local independent servers used synthetic accounts. Resource tests retain their original 10,000-message, large-transfer, memory and concurrency assertions. Passing mocks is not live-provider compatibility; local checks do not prove remote CI ran.

## Current reproducible local candidate (7792643)

`dist/final-candidate/EVIDENCE.json` now selects this independently verified
local archive; the preceding selection is preserved in
`test-results/startup-logging/previous-final-candidate-evidence.json`.

```text
dist/startup-logging/a/nuncio-0.1.0-rc-aarch64-apple-darwin-779264348db7.tar.gz
```

| Item | SHA-256 |
|---|---|
| Archive | `29b9e6b27472253d44746d1c28f2ddceb98d51a61d8f5687bc8a4fc202cffcbc` |
| `bin/nunciod` | `5cd9e7663931d5509cd250e3f54567bd2e361133748ff5ac5486c4bc4fe08d23` |
| `bin/nuncio-cli` | `6dea2a85bce2e06e2fbfa3e055998a7ceb6148a5937754e2f129a3d0de4acaa5` |
| Frozen descriptor | `fa665cd63baac1ed7b5fbbfe561920231ce63c0785e5e98d960d0cc8e82a66f8` |

The `b/` sibling is byte-identical. Each clean package has 478 manifest entries,
241 notice entries and 22 passed extracted checks. Metadata records clean 7792643,
Rust 1.97.1, Apple clang 21, ARM64 and no test features or bundled Google registration.
`test-results/startup-logging/package-pair.json` records commands and exit statuses;
parent/child egress denial is verified for both builds. Equality with a hosted
archive is not assumed: its build environment and hash are verified separately.

## Retained reproducible local candidate (82a92a0)

The earlier local archive remains preserved under this isolated worktree's
`rebuild/` directory for its source-specific reproducibility evidence:

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

## Latest verified testing download (7792643)

The hosted archive SHA-256 is `b7af0ad42609fcfe7df18f0bb2e598c56987a0b248db4ebd6a838e1e2ff72d3f`.
The public installer selected artifact 10363403258, run 34880705357
attempt 1. Both a fresh installation and an update from f14b02a passed. The retained
update prefix is:

```text
/private/tmp/nuncio-curl-acceptance-01p6bwb2/testing prefix
```

Its `current/TESTING-INSTALL.json` identifies 7792643 and the selected artifact.
Permanent commands remain `bin/nunciod` and `bin/nuncio-cli`; their hashes are:

| Binary | SHA-256 |
|---|---|
| `bin/nunciod` | `8d0557e7d21d4b086f32791ee5252eb50e34513eb4a2e5c62ed12130a13e5262` |
| `bin/nuncio-cli` | `906a7ffa8c16f470e0cf65ba9427d5b6f9ac0e213873ab3d3420921ec362798a` |

All 478 manifest files, ARM64 headers, versions and the complete 71/531/192 CLI audit
passed in each installation. The fresh package also passed INFO/debug/off
production startup preflight without creating a profile or touching Keychain.
The previous f14b02a build remains byte-for-byte intact; its older 7508e25 predecessor
is retained too. Normal installation, shell files and accounts were unchanged.
Evidence: `test-results/startup-logging/{installer,update}/public-verification.json`,
`packaged-log-checks.json` and `help-audit/audit-after.json` in the relevant directory.
These production checks did not start a serving daemon or exercise live accounts;
the full lifecycle behavior is verified separately through offline subprocess tests.

The hosted archive has its own build environment and is not asserted byte-equal
to the local pair. Its metadata reports `google_oauth.configured=false`.
Artifacts expire after 14 days; the installer selects a successful retained eligible
build. Packaged documents are build-time snapshots; this checkout report records
subsequent verification. Historical f14b02a installer receipts remain in
`test-results/stable-installer/` and retain their original source/hashes.

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
