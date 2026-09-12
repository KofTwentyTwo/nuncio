# Nuncio rebuild implementation report

The engine, authenticated API, CLI, account management, and testing installer are implemented at signed/pushed checkpoint `164b021750a70b49a1e060602f653f22cd14a46c`. The full offline gate passed, and two fresh production builds produced identical verified Apple Silicon archives. All ten hosted rebuild jobs and all seven security jobs passed on that exact source; the actual CI download/temporary installation also passed. **The full goal remains incomplete until separately authorized Google, Synology and native-keystore acceptance passes.** Native apps remain future work.

## Delivered behavior

The Rust daemon owns SQLCipher storage, credentials, account isolation, synchronization and durable operations. The authenticated `nuncio.v2` loopback gRPC API has 49 RPCs and an independent CLI. Google support includes OAuth/refresh, Gmail initial/history synchronization, offline search and MIME/attachment reads, durable drafts, sending and mail mutations, Calendar discovery/synchronization, recurring-event occurrences, writes, RSVP and free/busy. MailPlus support uses IMAP/SMTP for folder/UID/flag synchronization, copy/move/trash/restore, submission and separate delivery/Sent-copy receipts. Live MailPlus compatibility is not established by the independent servers.

[Account management](ACCOUNT-MANAGEMENT.md) adds saved details and versioned edits, Google add/reauthentication and consent wait/cancel, IMAP settings/password updates, pause/resume, archive-by-default removal, restore and separately confirmed local purge. Schema 23 stores the new lifecycle metadata. Purge refuses unresolved remote-effect evidence, preserves other accounts and replay continuity, and retains failed credential cleanup for retry. Archive/restore, migration, backup and failed-deletion recovery are covered through storage, engine, API and actual CLI tests.

Durable intent, explicit uncertainty, crash reconciliation, raw export, encrypted backup/restore and repair work through the engine/API/CLI. Tests inspect independent provider state, received bytes and send/copy/notification effects instead of inferring remote success from local records. The [R01–R16 and AM01–AM08 matrix](REQUIREMENTS.md) maps requirements to implementation and evidence.

## Actual verification

Repeated workspace and named executions are not additional unique tests. Exact commands, exit statuses and earlier failures remain in [VERIFICATION.md](VERIFICATION.md).

| Check | Observed result | Evidence under `test-results/` |
|---|---|---|
| Full offline gate | All 34 commands exited 0; 322 tests in each workspace configuration, zero failed/ignored; 411 captured source hashes unchanged | `account-management-final-all/{final-summary,exit,source-verification,egress}.json` and `current-all/` |
| Separate Google suites | Mock 26; system 22; actual CLI E2E 30; operation system 8; all passed | Same directory, named logs |
| Separate IMAP/SMTP suites | Contract 2; system 28; actual CLI E2E 14; all passed with independent remote effects | Same directory, `imap_*.log` |
| Recovery and isolation | Recovery E2E 5; repair 2/2; migration 1 covering all 46 before/after commit SIGKILL cases; reconciliation 3; multi-engine 1 | Same directory; all 46 individual migration receipts independently inspected |
| Security and resource checks | Security 3/2, including 196 invalid-auth cases across 49 RPCs; resources 4/3; release isolation 2; all passed | Same directory; retained auth, encryption, hostile-content and resource evidence |
| Supporting checks | Six independent Python service scripts; 22 script regressions; formatting; both Clippy modes; dependency and client boundaries; generated-client E2E 1; all passed | Same directory |
| Fresh local production packages | Two clean builds; identical archives/binaries; 22 extracted checks, 467 verified manifest entries and 237 third-party notices each; unchanged inputs and valid README links | `account-management-package/{results,comparison,receipt-validation,verified-candidate}.json` |
| Current hosted rebuild and installer | All 10 jobs passed: core 162/platform, mock 26/platform, E2E 43/platform, system 39, IMAP/resource 48, release/client 3/platform. All 11 artifacts retained/digest-checked; actual temporary installation and 27 help/version/account checks passed | [Run 34704460234](https://github.com/KofTwentyTwo/nuncio/actions/runs/34704460234), `testing-installer-current/` |
| Current hosted security | All seven jobs passed; all four analyses inspected; nine action alerts fixed, no new findings; 44 previously classified alerts remain open | [Run 34704460237](https://github.com/KofTwentyTwo/nuncio/actions/runs/34704460237), `security-ci/hosted-34704460237/` |

Automated provider tests stayed offline. Parent/child egress checks denied external traffic while allowing loopback; local independent servers used synthetic accounts. Resource tests retain their original 10,000-message, large-transfer, memory and concurrency assertions. Passing mocks is not live-provider compatibility; local checks do not prove remote CI ran.

## Verified local artifact

The selected archive is under this isolated worktree's `rebuild/` directory:

```text
dist/final-candidate/verified/nuncio-0.1.0-rc-aarch64-apple-darwin-164b021750a7.tar.gz
```

| Item | SHA-256 |
|---|---|
| Archive | `4a9e33a91c11d3dbdf376ebd01827f9bd645f4fbbce03517fa8fad04f13af7cc` |
| `bin/nunciod` | `402d81018ee2a98ea43e928711844aac3045f1df92a0d84c994c42669ab288e5` |
| `bin/nuncio-cli` | `68462388004977f37c24ddde391be118991c5220d4ba869631815acb3673478d` |
| Frozen current descriptor | `fa665cd63baac1ed7b5fbbfe561920231ce63c0785e5e98d960d0cc8e82a66f8` |

Metadata records clean 164b021, macOS ARM64, Rust 1.97.1, Apple clang 21 and source epoch `1789229447`, with no test features. Adjacent checksum/build/verification files and `dist/final-candidate/EVIDENCE.json` identify the exact artifact and remaining acceptance. Repeatability applies to identical recorded source/platform/compiler/SDK/build paths; hosted builds are verified against their own metadata, with no assumed byte equality to this machine. Developer ID signing and notarization are not established. Archived reports are build-time snapshots, so later documentation does not relabel their source.

The actual hosted download is artifact `10301603739` from run `34704460234`,
attempt 1: 9,602,280-byte ZIP SHA256
`2dc2926f53a6c92b256125fb1a5b41fdaba8f83b8a1132eb1e105c26249ffbe0`;
contained archive SHA256
`a9e9a1f7a9ee9672591252580ee1f47c453bd1627c043b7d2b0eba5692b3fa76`.
Its own metadata/manifest were verified; differing bytes from the local archive
are not a reproducibility failure across different recorded build environments.
`test-results/testing-installer-current/verification-summary.json` records the
retained temporary installation path and exact commands. Artifacts expire after
14 days; the installer selects the newest successful retained eligible run.

## Installation, operation and recovery

For a laptop download without local compilation, follow [TESTING-INSTALL.md](TESTING-INSTALL.md). The installer selects a fully successful retained feature-branch build, validates GitHub provenance and package integrity, and copies it into a separate versioned prefix. Actual run34704460234/attempt1 installation passed into a fresh `/private/tmp` prefix. Its 467-entry manifest and both ARM64 headers matched; 27 parser-only checks passed and normal-environment snapshots stayed unchanged. These help checks establish command availability, while the separate system/E2E suites establish behavior. It does not start a daemon or alter PATH, normal profiles or accounts. [PACKAGING.md](PACKAGING.md) describes local archive checks and extraction.

[RUNNING.md](RUNNING.md) gives first-run setup, secure credential entry, mock-only usage, action-file examples, JSON and exit semantics. [ACCOUNT-MANAGEMENT.md](ACCOUNT-MANAGEMENT.md) gives add/edit/reauth/pause/archive/restore/purge commands. The [API guide](API.md) describes the current contract. Original source/data and the rebuild's separate profile/port remain preserved.

Follow [RECOVERY.md](RECOVERY.md): preserve the original profile and keys, create an encrypted consistent backup, inspect it and restore into a new profile. Restored pending operations remain held until explicit reconciliation. After a lost acknowledgement, retain the request UUID and inspect durable receipts plus independent provider evidence. Do not repeat ambiguous SMTP delivery to repair a missing Sent copy; explicit duplicate-risk resolution is a separate decision. Account purge is logical local deletion, not secure erasure or deletion of remote data/backups.

## Dependencies, PRs and future work

The original workspace dependency/security remediation passed formatting, strict Clippy, all 790 tests, old AES-GCM/age ciphertext compatibility and a full-lock audit with fresh registry/advisory indexes. All 11 open PRs were reviewed and incorporated or superseded on the feature branch; none were merged or closed. [OPEN-PR-REPORT.md](OPEN-PR-REPORT.md) gives exact heads and proposed later dispositions. [DEPENDENCY-SECURITY-REPORT.md](DEPENDENCY-SECURITY-REPORT.md) records versions and evidence.

[SECURITY-CI.md](SECURITY-CI.md) records development triggers, full-lock scanning and actual findings. The first expanded run detected a yanked optional lock entry and nine mutable action references; both were corrected. The original 53 code alerts were individually classified; reported Rust/Python credential flows were synthetic test fixtures or protected production paths, with no demonstrated production exposure in those traces. Archived reference files have documented semantic-extraction limits. No alerts were dismissed or suppressed and no remote protection settings were changed. Workflow presence does not establish mandatory merge enforcement.

[API-PUBLICATION-PLAN.md](API-PUBLICATION-PLAN.md) proposes independent SemVer contract artifacts, generated documentation and compatibility tooling; that publication pipeline is not claimed implemented. The requested [post-engine PO/PM roadmap](POST-ENGINE-ROADMAP.md) proposes Mac alpha → daily mail → daily calendar → dependable personal release after engine/CLI acceptance. Its staffing/effort assumptions are planning estimates. Native development and wider publication remain outside this goal.

## Compatibility and remaining completion condition

[COMPATIBILITY.md](COMPATIBILITY.md) records the exact protocol limits. Google Calendar supplies calendaring; Synology Calendar/CalDAV, permanent provider-message purge, reminder delivery and native apps are excluded. Independent Dovecot 2.4.5 and Mailpit 1.31.1 checks do not establish compatibility with an unknown DSM/MailPlus installation. Actual TLS, capabilities, mailbox encoding and server/client Sent policy must be recorded during acceptance. An earlier hosted Linux transfer exceeded a CLI deadline; later runs passed unchanged bounds, but that earlier timeout's cause remains unproven and its evidence is retained.

The [manual worksheet](MANUAL-ACCEPTANCE.md) still requires explicit authorization for named disposable accounts, recipients, calendars, actions and cleanup. G01–G10, S01–S05 and X01 remain unapproved/unverified: live OAuth/refresh, native keystore, remote byte fidelity, real mail/calendar writes and notifications, and MailPlus Sent behavior. James deferred those checks in favor of full mocks. No live account was accessed for rebuild acceptance; separately authorized status emails provide no provider-acceptance evidence. Merges, releases/tags, normal-environment installation and remote settings changes were not performed.

Earlier baseline checkpoints 6ff9bb9, 7390e77 and 549a598 passed their recorded hosted rebuild runs. Their schema22/42-RPC reports and b26d2c93 archive remain preserved in Git, verification history and `dist/final-candidate/EVIDENCE-before-account-management-164b021.json`; they are not relabeled as current-source evidence.
