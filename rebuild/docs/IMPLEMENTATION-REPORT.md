# Nuncio rebuild implementation report

The engine, authenticated API, CLI, account management, guided setup and testing
installer are implemented at signed/pushed `b2b3ed5732f5884d1a1dc6b33daf18aa3cc3bc85`.
The full guided-setup offline gate passed, followed by the corrected Linux/macOS
scheduler regression. Two fresh production builds produced identical verified
Apple Silicon archives. Current hosted CI and latest-build installation are
pending. **The full goal remains incomplete until separately authorized Google,
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
| Full guided-setup offline gate | All35commands exited0;328tests per workspace configuration, zero failed/ignored | `all/results.json`; `account-setup-ux/full-second-egress.json` |
| Separate Google suites | Mock26; system22; actual CLI E2E35; operation system8; all passed | `all/`, named logs |
| Separate IMAP/SMTP suites | Contract2; system28; actual CLI E2E15; all passed with independent remote effects | `all/`, named logs |
| Recovery and isolation | Recovery5; repair2/2; all46 migration before/after-commit SIGKILL cases; reconciliation3; multi-engine1; all passed | `all/`, retained individual migration receipts |
| Security and resources | Security3/2,196invalid-auth cases across49RPCs; resources4/3; release isolation2; all passed | `all/`, auth/encryption/hostile-content/resource receipts |
| Supporting checks | Independent Python server checks,34current script regressions, formatting, bothClippy configurations, dependency and generated-client boundaries passed | `all/`; `account-setup-ux/post-ci-checks.json` |
| Scheduler correction | Controlled Linux latency RED101 then GREEN0; corrected complete Linux22/macOS22 and fmt/bothClippy pass | `account-setup-ux/linux/`; `scheduler-regression.json` |
| Fresh local packages | Two clean builds; identical archives/binaries;22extracted checks,469manifest entries and237notices each | `account-setup-ux/{package-final,package-commands}.json` |
| Current hosted rebuild | b2b3ed5 failed the earlier fixed-sleep beta-progress assertion; bounded-progress correction is under local verification | [Run34770885579](https://github.com/KofTwentyTwo/nuncio/actions/runs/34770885579) |
| Current hosted security | Pending exact-head run34770885540; preceding78d4d9e passed7/7 | [Run34770885540](https://github.com/KofTwentyTwo/nuncio/actions/runs/34770885540) |
| Current public installer | Pending successful hosted run; earlier versions retain separate passed installation receipts | `account-setup-ux/installer/` |

Automated provider tests stayed offline. Parent/child egress checks denied external traffic while allowing loopback; local independent servers used synthetic accounts. Resource tests retain their original 10,000-message, large-transfer, memory and concurrency assertions. Passing mocks is not live-provider compatibility; local checks do not prove remote CI ran.

## Verified local artifact

The selected archive is under this isolated worktree's `rebuild/` directory:

```text
dist/account-setup-ux/a/nuncio-0.1.0-rc-aarch64-apple-darwin-b2b3ed5732f5.tar.gz
```

| Item | SHA-256 |
|---|---|
| Archive | `8dc16d97c28bd88e6a55077f047996fba0e80b085a45b3ebb003bab7e06408ee` |
| `bin/nunciod` | `fd381a58f097e526c38f7516d94e816b7db6065031b0185b52220752745188a0` |
| `bin/nuncio-cli` | `9dbd9a67705d48b1c4a21a2c78dde78e6f3277cb5fc35cde216cc4b5da757904` |
| Frozen current descriptor | `fa665cd63baac1ed7b5fbbfe561920231ce63c0785e5e98d960d0cc8e82a66f8` |

Metadata records clean b2b3ed5, macOS ARM64, Rust1.97.1, Apple clang21 and source
epoch1789319544, no test features and `google_oauth.configured=false`. Adjacent
checksum/build/verification files and `test-results/account-setup-ux/package-final.json`
identify both artifacts and the retained extraction. Repeatability applies to
identical recorded source/platform/compiler/SDK/build paths; hosted archives
are verified separately. Developer ID signing/notarization and live/native
acceptance remain unestablished. Packaged reports are build-time snapshots.

The earlier164b021 archive4a9e33a9, hosted run34704460234 and actual temporary
installation remain preserved under `dist/final-candidate/` and
`test-results/testing-installer-current/`; they qualify that earlier software.
The public bootstrap was separately verified at57c619e selecting c662e48, with
all467manifest entries and both ARM64 binaries checked. Neither earlier result
is used as proof that the new guided setup download has qualified.

## Installation, operation and recovery

For a laptop download without compiling, follow [TESTING-INSTALL.md](TESTING-INSTALL.md).
The installer selects a fully successful retained feature-branch build, validates
GitHub provenance and package integrity, and copies it into a versioned prefix.
Current-source temporary installation is still pending. The installer does not
start a daemon or alter PATH, normal profiles or accounts. [PACKAGING.md](PACKAGING.md)
describes local archive checks and extraction; help checks establish command
availability, while separate system/E2E suites establish behavior.

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
