# Nuncio implementation report

Status at September 12, 2026, 3:44 a.m. Central: **engine/API/CLI implemented; IMAP cursor correction passed the complete offline gate; fresh artifacts and hosted verification are next.** The full R01–R16 goal is incomplete. Google/Synology live and native-keystore acceptance remain deferred/unverified. Native apps, merge, release and normal-environment installation are outside the work performed.

The cursor correction passed all34offline commands and308tests in each workspace configuration, with zero failures or ignored tests and399source hashes unchanged. All separate mock/system/actualCLI/independent-server/security/recovery/resource/client/dependency checks passed. The correction retains provider mailbox hints while excluding optional Marked/Unmarked from the coverage fingerprint. Its regression failed before the fix and passes afterward; real mailbox changes still alter coverage. Existing profiles receive a one-time local coverage-token change on next promotion; no schema/API or remote-cursor changes are needed.

Actual prior hosted run [34681263133](https://github.com/KofTwentyTwo/nuncio/actions/runs/34681263133) ended9jobs passed/1IMAP failure and supplied the cause. The earlier Linux transfer timeout did not recur, but its cause remains unexplained. A signed correction checkpoint, fresh clean package pair and actual hosted replacement are next. The preserved clean6a324f9 package below passed22extracted checks per build and repeatability; it predates this cursor correction. Prior failures/artifacts remain retained.

## Delivered software

The Rust daemon owns encrypted SQLCipher storage, credentials, account isolation, scheduling and durable mail/calendar operations. The authenticated `nuncio.v2` local gRPC API exposes the engine to an independent CLI. A frozen descriptor and separately generated status/watch client test the client boundary.

Google support includes OAuth/refresh, Gmail full/history synchronization, local search and MIME/attachment fidelity, drafts, send and mail mutations, Calendar catalog/events/occurrences, writes, RSVP and free/busy. Synology mail support uses strict IMAP/SMTP, folder/UID/flag synchronization, exact copy/move/trash/restore behavior and separate SMTP-acceptance/Sent-copy receipts. All provider automation is tested against stateful local services. Calendar support uses Google; Synology Calendar/CalDAV is excluded.

Durable operation intent, crash reconciliation, encrypted backups/new-profile restore, migrations and projection repair are implemented through engine/API/CLI. Unknown send/copy/notification outcomes remain explicit. Independent remote state and effect counts validate recovery; a local success record alone is insufficient. Resource admission, bounded payloads/concurrency and numeric status are exposed and tested.

The [R01–R16 matrix](REQUIREMENTS.md) links implementation and executable evidence. [Compatibility limits](COMPATIBILITY.md) distinguish implemented behavior from live-provider claims.

## Actual verification

| Check | Observed result | Local evidence under `test-results/` |
|---|---|---|
| Cursor correction full gate, shell88082 | 34commands passed;308tests per workspace, zero failed/ignored;399source files unchanged | `ci-imap-interest-all/{final-summary,exit,source-verification}.json` and `current-all/` |
| Prior promotion full gate, shell91408 | 34commands passed;307tests per workspace, zero failed/ignored;399source files unchanged | `ci-promotion-all/{final-summary,exit,source-verification}.json` and `current-all/` |
| Full integrated egress-denied gate, shell 58495 (prior snapshot) | 34 commands passed; 305 Rust tests in each workspace configuration; zero failed/ignored | `task15-all-green/{results,counts,source-verification}.json` and command logs |
| Separate named suites in that gate | Google mock 26 / system 22 / E2E 27; operations 6; IMAP contract 2 / system 27 / E2E 13; recovery 5; repair 2 / 2; migration 1; reconciliation 3; three-engine 1; security 3 / 2; resources 4 / 3; production isolation 2 | `task15-all-green/` |
| Independent services, dependency and client checks | Six Python mail-service scripts passed; dependency advisories/licenses/sources passed; generated-client E2E 1 passed; CLI/client dependency boundaries passed | `task15-all-green/`; `task15-dependencies/`; `task15-contract/` |
| Prior clean6a324f9 package pair, shell14220 | Two fresh builds,22extracted checks each, identical archives/binaries,237notices; unchanged inputs | `task16-promotion-package/` and selected `dist/final-candidate/EVIDENCE.json` |
| Packaging and final guide correction (prior snapshot) | Eight script regressions passed; final two fresh packages each passed 22 extracted-binary checks and produced identical archive/binary bytes; guide identifies its own metadata/checksum | `task16-package-final/`; prior `task15-repeat-green/`; adjacent archive evidence |
| Hosted resource-fixture correction, shell 91079 | Six outer / 17 nested commands passed; 56 subprocess tests passed; both Clippy modes and formatting passed; parent/child egress checks passed | `ci-resource-fix/{results,counts,production-source-check}.json` and copied logs |
| Hosted run 34671490682 on 7f81b73 | macOS lint/mock/release passed; macOS E2E failed its synthetic one-second large-payload deadline; six Ubuntu jobs cancelled when superseded | `remote-ci/34671490682/` |
| Hosted rerun 34672641999 on 2e4d30e | all four macOS jobs passed, including the package command; all six Ubuntu jobs failed with runner communication loss; no overall pass claimed | [Actual workflow](https://github.com/KofTwentyTwo/nuncio/actions/runs/34672641999) |

Repeated workspace and named executions are not distinct unique tests. All automated provider tests use synthetic accounts with external egress denied. The migration test includes 44 actual SIGKILL cases; restore coverage includes five process-death boundaries. Security checks include 168 invalid-auth cases across 42 RPCs, encrypted-store/log canaries, TLS rejection and hostile content. Resource workloads include 10,000 messages and eight 16 MiB attachment cycles with the original 128 MiB post-warm-up growth bound.

The hosted failure was independently reproduced with a 1.25-second response delay. Only the large-resource fixture now uses the existing 30-second production HTTP deadline; other short fault-test deadlines, mock validation and remote-effect assertions are unchanged. The correction passed locally and in the hosted macOS E2E job (40 tests, seven commands, parent/child egress checks). It was signed/pushed as 2e4d30ef9434638db4a5b824b490cdb4278bf13c. That earlier source snapshot matched the prior packaged candidate; the current production correction requires a fresh package. Original failed runs remain recorded in [VERIFICATION.md](VERIFICATION.md).

The later Linux failures exposed a separate CI guard defect: filtering the whole runner UID also cut off GitHub control traffic. An independent same-UID controller regression reproduced that cutoff. A dedicated cgroup now limits filtering to test descendants; local IPv4/IPv6 controller continuity, test denial, exit0/17 propagation, detached-child termination and owned-resource cleanup pass. All eight existing script regressions pass under the macOS guard. All six hosted Linux controller checks subsequently passed in34676397291; nine jobs passed and the separate large-mailbox production resource failure is addressed by the current correction.

## Preserved verified local artifact

This archive passed its recorded checks but predates the current uncommitted IMAP cursor correction. A fresh verified package is required after full regression. From the isolated worktree's `rebuild/` directory:

```text
dist/final-candidate/verified/nuncio-0.1.0-rc-aarch64-apple-darwin-6a324f9d1cb1.tar.gz
```

| Item | SHA-256 |
|---|---|
| Archive | `bfc7c8e70d8a0e68bd843a811e7dc9a49018f052c204fa4128991ab695ff1784` |
| `bin/nunciod` | `7fd338ff1ccd3e5b0b8b1184a0dec1da23070b59f2822aad41498c9ad1e64476` |
| `bin/nuncio-cli` | `1495aa640c31a3b45159005af8f040292af3d03ffacb41448d2b33e22163cac5` |

The archive includes 237 third-party notices, protobuf sources/descriptor, operating documentation and a manifest. Adjacent `EVIDENCE.json`, checksum, build JSON and extraction-check JSON retain proof. Metadata identifies clean checkpoint6a324f9 with dirty:false; included reports retain their build-time timestamps. Current hosted results are recorded separately in this checkout. Repeatability applies to identical recorded source, platform, compiler, SDK and checkout/build paths. Linux/Windows artifacts and cross-platform byte equality are not inferred.

## Operation and recovery

Follow [PACKAGING.md](PACKAGING.md) to verify the checksum, extract into a new temporary directory, inspect help/version and understand optional later install/uninstall. [RUNNING.md](RUNNING.md) contains mock-only startup, account configuration, mail/calendar action files, JSON and exit semantics. The default rebuild profile and port are separate from the original application. No original data import or normal-environment installation was performed.

Use [RECOVERY.md](RECOVERY.md) for encrypted backup, passphrase input through a protected pipe, inspection and restoration into a new profile. Preserve original profiles and keys. Restored pending operations stay held until explicit reconciliation; ambiguous SMTP delivery must not be repeated to repair a Sent copy. Keep the original request UUID after lost acknowledgements, inspect durable receipts and independent provider evidence, and use explicit duplicate-risk resolution only when intended.

## Remaining completion conditions and risks

1. Complete the current cursor correction's full offline gate, signed checkpoint/push, fresh clean package pair and actual hosted verification. Preserve original assertions and earlier failures. Local checks do not prove hosted CI ran or passed.
2. When James resumes live scope, complete the [manual worksheet](MANUAL-ACCEPTANCE.md): G01–G10, S01–S05 and X01. All are currently unapproved/unverified. Named accounts, recipients, calendars, actions and secure credential entry require explicit authorization before access.
3. Record actual Google OAuth/provider behavior, Synology DSM/MailPlus versions/capabilities and Sent policy, and native keystore operation. Passing independent mocks cannot establish those facts.

Provider acknowledgement loss can leave delivery or notification outcomes unknown by design. Resource results are machine-specific, and archived documentation is a build-time snapshot. The current source report, matrix and [session state](SESSION-STATE.md) retain later evidence. No merge, public release, installation or live-compatibility claim is made.
