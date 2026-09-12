# Nuncio rebuild implementation report

The engine, authenticated API, and CLI are implemented and offline verified at signed checkpoint `6ff9bb9e7be23249c9f448276e08c3062efc44a7`. The full local gate passed, all ten actual hosted CI jobs passed, and two fresh production builds produced identical verified archives. **The full goal remains incomplete: live Google/Synology and native-keystore acceptance are deferred, unapproved, and unverified.** No native application, merge, release, or normal-environment installation was performed.

The Rust daemon owns SQLCipher storage, credentials, account isolation, synchronization, and durable operations. The versioned, authenticated `nuncio.v2` loopback gRPC API exposes that engine to an independent CLI. Google support includes OAuth/refresh, Gmail initial/history sync, offline search and MIME/attachment reads, drafts, sending and mail mutations, Calendar discovery/sync, recurring-event occurrences, writes, RSVP, and free/busy. MailPlus support uses IMAP/SMTP for folder/UID/flag synchronization, copy/move/trash/restore, submission, and separate Sent-copy receipts. It was tested against independent local servers; live MailPlus compatibility is not established.

Durable intent, explicit uncertainty, crash reconciliation, raw export, encrypted backup/restore, migrations, and repair work through the engine, API, and CLI. Tests inspect independent provider state, received bytes, and send/copy/notification effects rather than relying only on local success records. The [R01–R16 matrix](REQUIREMENTS.md) links each requirement to implementation, executable evidence, and its remaining acceptance condition.

The following results were observed on the correction checkpoint. Repeated workspace and named executions are not additional unique tests. Exact commands, exit statuses, and logs are retained in [VERIFICATION.md](VERIFICATION.md) and the evidence directories below.

| Check | Actual result | Evidence under `test-results/` |
|---|---|---|
| Complete offline gate | All 34 commands exited 0; both workspace configurations passed 308 tests each, with zero failures or ignored tests; all 399 captured source hashes remained unchanged | `ci-imap-interest-all/{final-summary,exit,source-verification}.json`, `current-all/` |
| Separate Google suites | Mock 26; system 22; actual CLI E2E 27; operation system 6; all passed | `ci-imap-interest-all/current-all/` |
| Separate IMAP/SMTP suites | Contract 2; system 27; actual CLI E2E 13; all passed, including independently observed remote effects | Same directory, `imap_*.log` |
| Recovery and isolation | Recovery E2E 5; repair 2/2; migration 1, including 44 SIGKILL cases; reconciliation 3; three-engine convergence 1; all passed | Same directory and retained numeric run artifacts |
| Security and resource checks | Security 3/2; resources 4/3; release isolation 2; all passed. Includes 168 invalid-auth cases across 42 RPCs, encrypted-store/log canaries, TLS rejection, hostile content, and production test-hook exclusion | Same directory, named suite logs |
| Supporting gates | Six independent Python mail-service scripts, eight script regressions, formatting, both Clippy configurations, dependency checks, and client-boundary checks passed; external generated-client E2E passed 1 test | Same directory; `dependency-review.log` reports advisories/licenses/sources OK |
| Actual hosted CI | All 10 jobs passed on Linux/macOS: core 155 tests per platform; mock 26 per platform; Google system 37; E2E 40 per platform; IMAP/resource 46; release isolation/external client 3 per platform; both package commands exited 0 | [Run 34684158773](https://github.com/KofTwentyTwo/nuncio/actions/runs/34684158773), `remote-ci/34684158773/{run-final,downloaded-summary}.json` |
| Fresh local production packages | Two clean builds; identical archives/binaries; 22 extracted-binary checks and 458 manifest entries verified per archive; 237 third-party notices; no input differences or missing README links | `task16-interest-package/{results,comparison,receipt-validation,verified-candidate}.json` |

Automated provider tests remained offline. Host and child egress checks denied external traffic while allowing loopback access; independent mail services used the isolated local topology. Hosted package command success is recorded, but hosted archive bytes and hashes were not uploaded and are not inferred from those exits.

The original 10,000-message workload retained all messages and traversed 100 API pages and 100 remote pages. Synchronization took 12.675 seconds in the normal workspace, 12.950 seconds in the second configuration, and 12.526 seconds in the separate system run; listing took 1.509, 1.508, and 1.497 seconds respectively. Repeated 16 MiB transfers and the 64 MiB input refusal passed the original memory and concurrency bounds. These are measurements on this machine, not universal latency promises.

The final correction excludes optional IMAP `Marked`/`Unmarked` interest hints from the coverage fingerprint while preserving the full provider state. Its SQLCipher regression failed before the fix and passed afterward; actual mailbox changes still alter coverage. Existing profiles receive a one-time opaque local coverage-token change on their next promotion. There is no schema, public API, or remote-cursor change. Earlier production and CI failures, their diagnoses, and original assertions remain recorded in the verification history.

The verified local artifact is under this isolated worktree's `rebuild/` directory:

```text
dist/final-candidate/verified/nuncio-0.1.0-rc-aarch64-apple-darwin-6ff9bb9e7be2.tar.gz
```

| Item | SHA-256 |
|---|---|
| Archive | `b26d2c93b57fbe31123ddaaa31a25fadb520e5d7fd78429ec603ee07078ed105` |
| `bin/nunciod` | `0dcdc7223d32a5b3b0f0dc94bf92bf8041796ab69131211094fc0ce2889ca9fa` |
| `bin/nuncio-cli` | `1495aa640c31a3b45159005af8f040292af3d03ffacb41448d2b33e22163cac5` |
| Frozen `nuncio.v2` descriptor | `21ab39c24af1fd2029001195f3016e03f3da20b1e7ead0462751320012576bf5` |

Metadata records clean checkpoint `6ff9bb9`, macOS ARM64, Rust 1.97.1, Apple clang 21, and source epoch `1789202823`. Adjacent checksum, build, and extracted-check files plus `dist/final-candidate/EVIDENCE.json` retain the receipts. Prior candidates are preserved. Byte repeatability applies to the same recorded source, platform, compiler, SDK, and build paths; no cross-platform byte-equivalence claim is made. Archived reports are build-time snapshots; later documentation does not relabel the archive's source.

Follow [PACKAGING.md](PACKAGING.md) to verify the checksum and extract into a new temporary directory. The guide also describes installation/uninstallation for a later authorized decision. [RUNNING.md](RUNNING.md) gives first-run setup, secure credential entry, mock-only development usage, action-file examples, JSON results, and exit semantics. [API.md](API.md) describes the frozen contract. The profile/data directory and default port are separate from the original application; original source, data, and unrelated work were preserved.

For recovery, follow [RECOVERY.md](RECOVERY.md): preserve the original profile and keys, create an encrypted consistent backup, inspect it, and restore into a new profile. Restored pending operations remain held until explicit reconciliation. Retain the original request UUID after an acknowledgement is lost, inspect durable receipts and independent provider evidence, and do not repeat an ambiguous SMTP delivery to repair a missing Sent copy. Explicit duplicate-risk resolution is a separate user decision.

[COMPATIBILITY.md](COMPATIBILITY.md) records protocol and provider limits. Google Calendar supplies calendaring; Synology Calendar/CalDAV, permanent purge, reminder delivery, and native apps are excluded. Independent Dovecot 2.4.5 and Mailpit 1.31.1 results do not establish compatibility with an unknown DSM/MailPlus installation. Actual TLS configuration, negotiated capabilities, mailbox encoding, and server/client Sent policy must be recorded during acceptance. One earlier hosted Linux transfer exceeded a CLI deadline; later runs passed unchanged bounds, but that earlier timeout's cause remains unproven and its diagnostic evidence is retained.

To complete the full goal, the [manual acceptance worksheet](MANUAL-ACCEPTANCE.md) still requires explicit approval for named disposable accounts, recipients, calendars, actions, and cleanup. Rows G01–G10, S01–S05, and X01 are unapproved and unverified. Live OAuth/refresh, native-keystore behavior, remote byte fidelity, actual mail/calendar writes, notifications, and MailPlus Sent behavior must pass before completion can be claimed. James has deferred that work in favor of full mocks for now; no live accounts were accessed for rebuild acceptance. Separately authorized status emails are not provider-acceptance evidence.
