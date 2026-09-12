# Nuncio implementation report

Status at September 12, 2026, 12:45 a.m. Central: **implemented and verified against local independent providers; hosted CI and live acceptance pending.** The full R01–R16 goal is incomplete. Google/Synology live checks remain deferred at James's direction. This report covers the approved separate `rebuild/` workspace; native apps, merge, release and normal-environment installation remain outside the work performed.

## Delivered software

The Rust daemon owns encrypted SQLCipher storage, credentials, account isolation, scheduling and durable mail/calendar operations. The authenticated `nuncio.v2` local gRPC API exposes the engine to an independent CLI. A frozen descriptor and separately generated status/watch client test the client boundary.

Google support includes OAuth/refresh, Gmail full/history synchronization, local search and MIME/attachment fidelity, drafts, send and mail mutations, Calendar catalog/events/occurrences, writes, RSVP and free/busy. Synology mail support uses strict IMAP/SMTP, folder/UID/flag synchronization, exact copy/move/trash/restore behavior and separate SMTP-acceptance/Sent-copy receipts. All provider automation is tested against stateful local services. Calendar support uses Google; Synology Calendar/CalDAV is excluded.

Durable operation intent, crash reconciliation, encrypted backups/new-profile restore, migrations and projection repair are implemented through engine/API/CLI. Unknown send/copy/notification outcomes remain explicit. Independent remote state and effect counts validate recovery; a local success record alone is insufficient. Resource admission, bounded payloads/concurrency and numeric status are exposed and tested.

The [R01–R16 matrix](REQUIREMENTS.md) links implementation and executable evidence. [Compatibility limits](COMPATIBILITY.md) distinguish implemented behavior from live-provider claims.

## Actual verification

| Check | Observed result | Local evidence under `test-results/` |
|---|---|---|
| Full integrated egress-denied gate, shell 58495 | 34 commands passed; 305 Rust tests in each workspace configuration; zero failed/ignored | `task15-all-green/{results,counts,source-verification}.json` and command logs |
| Separate named suites in that gate | Google mock 26 / system 22 / E2E 27; operations 6; IMAP contract 2 / system 27 / E2E 13; recovery 5; repair 2 / 2; migration 1; reconciliation 3; three-engine 1; security 3 / 2; resources 4 / 3; production isolation 2 | `task15-all-green/` |
| Independent services, dependency and client checks | Six Python mail-service scripts passed; dependency advisories/licenses/sources passed; generated-client E2E 1 passed; CLI/client dependency boundaries passed | `task15-all-green/`; `task15-dependencies/`; `task15-contract/` |
| Packaging and final guide correction | Eight script regressions passed; final two fresh packages each passed 22 extracted-binary checks and produced identical archive/binary bytes; guide identifies its own metadata/checksum | `task16-package-final/`; prior `task15-repeat-green/`; adjacent archive evidence |
| Hosted resource-fixture correction, shell 91079 | Six outer / 17 nested commands passed; 56 subprocess tests passed; both Clippy modes and formatting passed; parent/child egress checks passed | `ci-resource-fix/{results,counts,production-source-check}.json` and copied logs |
| Hosted run 34671490682 on 7f81b73 | macOS lint/mock/release passed; macOS E2E failed its synthetic one-second large-payload deadline; six Ubuntu jobs cancelled when superseded | `remote-ci/34671490682/` |
| Hosted rerun 34672641999 on 2e4d30e | all four macOS jobs passed, including the package command; all six Ubuntu jobs failed with runner communication loss; no overall pass claimed | [Actual workflow](https://github.com/KofTwentyTwo/nuncio/actions/runs/34672641999) |

Repeated workspace and named executions are not distinct unique tests. All automated provider tests use synthetic accounts with external egress denied. The migration test includes 44 actual SIGKILL cases; restore coverage includes five process-death boundaries. Security checks include 168 invalid-auth cases across 42 RPCs, encrypted-store/log canaries, TLS rejection and hostile content. Resource workloads include 10,000 messages and eight 16 MiB attachment cycles with the original 128 MiB post-warm-up growth bound.

The hosted failure was independently reproduced with a 1.25-second response delay. Only the large-resource fixture now uses the existing 30-second production HTTP deadline; other short fault-test deadlines, mock validation and remote-effect assertions are unchanged. The correction passed locally and in the hosted macOS E2E job (40 tests, seven commands, parent/child egress checks). It was signed/pushed as 2e4d30ef9434638db4a5b824b490cdb4278bf13c. All 244 production source files still match the verified packaged candidate. Original failed runs remain recorded in [VERIFICATION.md](VERIFICATION.md).

The later Linux failures exposed a separate CI guard defect: filtering the whole runner UID also cut off GitHub control traffic. An independent same-UID controller regression reproduced that cutoff. A dedicated cgroup now limits filtering to test descendants; local IPv4/IPv6 controller continuity, test denial, exit0/17 propagation, detached-child termination and owned-resource cleanup pass. All eight existing script regressions pass under the macOS guard. This CI-only correction still needs its own hosted run; it changes no production source or selected package.

## Verified local artifact

From the isolated worktree's `rebuild/` directory:

```text
dist/final-candidate/verified/nuncio-0.1.0-rc-aarch64-apple-darwin-2e4d30ef9434.tar.gz
```

| Item | SHA-256 |
|---|---|
| Archive | `8996085e4e698383e11625d848b287616b165c0a9b33fb4430c8ab872cedc1be` |
| `bin/nunciod` | `f64aad27cb3c75eebd05822f73f8439f4ae9fc0e52e9b8cc6e0bb0642c47b7de` |
| `bin/nuncio-cli` | `1495aa640c31a3b45159005af8f040292af3d03ffacb41448d2b33e22163cac5` |

The archive includes 237 third-party notices, protobuf sources/descriptor, operating documentation and a manifest. Adjacent `EVIDENCE.json`, checksum, build JSON and extraction-check JSON retain proof. Metadata honestly identifies the tested dirty source snapshot based on 2e4d30e with uncommitted documentation; it is not relabeled as a clean build. Repeatability applies to identical recorded source, platform, compiler, SDK and checkout/build paths. Linux/Windows artifacts and cross-platform byte equality are not inferred.

## Operation and recovery

Follow [PACKAGING.md](PACKAGING.md) to verify the checksum, extract into a new temporary directory, inspect help/version and understand optional later install/uninstall. [RUNNING.md](RUNNING.md) contains mock-only startup, account configuration, mail/calendar action files, JSON and exit semantics. The default rebuild profile and port are separate from the original application. No original data import or normal-environment installation was performed.

Use [RECOVERY.md](RECOVERY.md) for encrypted backup, passphrase input through a protected pipe, inspection and restoration into a new profile. Preserve original profiles and keys. Restored pending operations stay held until explicit reconciliation; ambiguous SMTP delivery must not be repeated to repair a Sent copy. Keep the original request UUID after lost acknowledgements, inspect durable receipts and independent provider evidence, and use explicit duplicate-risk resolution only when intended.

## Remaining completion conditions and risks

1. Checkpoint the locally verified Linux cgroup correction, observe all required jobs on the new hosted head, preserve their command evidence and fix demonstrated failures. Local checks do not prove hosted CI ran or passed.
2. When James resumes live scope, complete the [manual worksheet](MANUAL-ACCEPTANCE.md): G01–G10, S01–S05 and X01. All are currently unapproved/unverified. Named accounts, recipients, calendars, actions and secure credential entry require explicit authorization before access.
3. Record actual Google OAuth/provider behavior, Synology DSM/MailPlus versions/capabilities and Sent policy, and native keystore operation. Passing independent mocks cannot establish those facts.

Provider acknowledgement loss can leave delivery or notification outcomes unknown by design. Resource results are machine-specific, and archived documentation is a build-time snapshot. The current source report, matrix and [session state](SESSION-STATE.md) retain later evidence. No merge, public release, installation or live-compatibility claim is made.
