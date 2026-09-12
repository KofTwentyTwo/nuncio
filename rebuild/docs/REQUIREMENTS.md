# Requirement-to-evidence matrix

Current schema-23 account/installer source passed the complete offline gate:
all 34 commands exited 0, both workspace configurations passed 322 tests with zero
failed/ignored, all 22 script regressions passed, and 411 captured source hashes
stayed unchanged. Receipts are in
`test-results/account-management-final-all/{final-summary.json,exit.json,source-verification.json,current-all/}`.
Signed/pushed checkpoint `164b021` also passed two fresh local production builds:
identical `4a9e33a9` archives and binaries, 22 extracted checks, 467 manifest entries
and 237 notices each. All ten current-source hosted rebuild jobs, seven security jobs and the actual
testing download/temporary installation passed. Live Google/Synology and native-keystore acceptance are deferred and
unverified. Exact evidence is in [VERIFICATION.md](VERIFICATION.md) and the
[implementation report](IMPLEMENTATION-REPORT.md). Historical schema22 results,
including hosted 6ff9bb9/7390e77/549a598 and the old b26d2c93 archive, remain
preserved separately and are not used to qualify these additions.

## Account-management follow-up

Current source adds saved account details/versioned edits, Google add/reauth and
consent wait/cancel, IMAP configuration/password updates, pause/resume,
archive-by-default removal, restore, and confirmed local purge. Its seven new
authenticated methods bring the API to 49 RPCs. The updated full offline gate
passes, including storage/system/actual CLI checks, all 46 migration SIGKILL
boundaries, and the 196-case auth matrix. The fresh local artifact and signed/pushed checkpoint are verified;
current-source hosted and installer verification also passed. The
[account guide](ACCOUNT-MANAGEMENT.md), [approved follow-up plan](ACCOUNT-MANAGEMENT-PLAN.md),
and dated [verification evidence](VERIFICATION.md) track that additional scope.
The [testing installer](TESTING-INSTALL.md) has offline regression evidence, but
its actual first eligible run and temporary installation are now verified. The [API publication proposal](API-PUBLICATION-PLAN.md) is not a delivered
public contract archive or documentation site.

| Account requirement | Implementation | Independent or storage evidence | Delivery status |
|---|---|---|---|
| AM01 Inspect and versioned edit | [engine management](../crates/nuncio-engine/src/accounts/management.rs), [CLI](../crates/nuncio-cli/src/accounts.rs) | `account_details_edit_pause_archive_restore_and_purge_work_through_cli`; IMAP failed-probe and version-conflict checks | Full current-source offline gate passed |
| AM02 Add, reauthenticate, wait and cancel | [Google CLI flow](../crates/nuncio-cli/src/accounts/google.rs), [IMAP management](../crates/nuncio-engine/src/accounts/imap.rs) | `google_auth_wait_cancel_and_archive_cannot_revive_saved_accounts`; `imap_account_edit_archive_restore_reauth_and_purge_preserve_remote_mail` | Offline flows passed; real OAuth/native keystore deferred |
| AM03 Pause and resume | [lifecycle storage](../crates/nuncio-engine/src/store/account_lifecycle.rs), [operation worker](../crates/nuncio-engine/src/operations.rs) | `paused_or_purged_admitted_work_does_not_dispatch_or_stop_other_accounts`; restart and queued-operation checks | Full current-source offline gate passed |
| AM04 Archive and restore | [lifecycle storage](../crates/nuncio-engine/src/store/account_lifecycle.rs), [engine management](../crates/nuncio-engine/src/accounts/management.rs) | Google/IMAP actual CLI lifecycle tests; preserved drafts and independent remote-state snapshots | Full current-source offline gate passed |
| AM05 Confirmed local purge | [preview/purge transaction](../crates/nuncio-engine/src/store/account_lifecycle.rs) | `account_purge_preserves_lost_ack_send_evidence_until_explicit_abandonment`; fresh-preview refusal, rollback, other-account isolation and replay tests | Logical local deletion passed; no remote or secure-erasure claim |
| AM06 Concurrency and recovery | [engine coordination](../crates/nuncio-engine/src/engine.rs), [schema 23](../crates/nuncio-engine/src/store/account_lifecycle_schema.sql) | Four actual archive/purge process-death boundaries; late OAuth/upload rejection; `imap_account_edits_failed_probe_and_purge_cleanup_are_durable`; encrypted backup/restore | Full gate and all 46 migration SIGKILL cases passed |
| AM07 API and CLI parity | [account contract](../crates/nuncio-proto/proto/nuncio/v2/accounts.proto), [daemon API](../crates/nunciod/src/accounts.rs), [wire compatibility](../crates/nuncio-proto/tests/contract.rs) | Seven additive RPCs; retained old descriptor comparison; all 49 RPCs included in the 196-case auth matrix; actual text/JSON CLI checks | Old-wire compatibility and all 196 auth cases passed |
| AM08 Verification and alpha delivery | [verification](VERIFICATION.md), [installer](TESTING-INSTALL.md), [packaging](PACKAGING.md) | Full offline gate, clean production package pair, exact hosted CI artifact and temporary-prefix installation receipt | Full offline gate, local package pair, actual10-job CI and temporary installer passed |

## R01–R16 at current checkpoint 164b021

The matrix maps the approved September 10 requirements to current implementation
and the new full offline regression. "Passed offline" is not live-provider
sign-off. Hosted delivery at164b021 is verified; the full goal remains incomplete
for the named live/native acceptance conditions. Task estimates are separate in [PROGRESS.md](PROGRESS.md).

All test paths below are under `crates/nuncio-test-support/tests/` unless stated
otherwise. [VERIFICATION.md](VERIFICATION.md) records dated command/exit evidence;
[SESSION-STATE.md](SESSION-STATE.md) identifies the active run. Independent provider
state and effect observations are part of the named tests, not inferred from local
operation status. Source links point to entry points; related modules carry the
storage, parsing and provider implementations.

| Requirement | Status | Implementation | Executable evidence | Remaining acceptance condition |
|---|---|---|---|---|
| R01 Daemon lifecycle, profiles and authenticated local API | Passed offline | [engine](../crates/nuncio-engine/src/engine.rs), [daemon](../crates/nunciod/src/server.rs), [System contract](../crates/nuncio-proto/proto/nuncio/v2/system.proto) | `google_e2e::real_cli_daemon_restart_health_json_and_missing_daemon_exit`; engine profile/lifecycle tests; `security_system` checks all 49 RPCs | Integrated egress and final macOS archive checks passed |
| R02 OAuth, secure credentials and account isolation | Externally pending | [accounts](../crates/nuncio-engine/src/accounts.rs), [IMAP account lifecycle](../crates/nuncio-engine/src/accounts/imap.rs), [OS keystore](../crates/nuncio-engine/src/secrets/keyring.rs) | `google_system::oauth_accounts_are_authenticated_isolated_and_refreshes_are_serialized`; actual CLI connect/disconnect/reconnect; strict IMAP trust/credential refusal | Keyring dependency remediation passed; native-keystore/live OAuth acceptance pending |
| R03 Gmail sync, MIME, attachments, offline queries and search | Externally pending | [Gmail adapter](../crates/nuncio-engine/src/providers/google/gmail.rs), [mail synchronization](../crates/nuncio-engine/src/mail.rs), [Mail API](../crates/nuncio-proto/proto/nuncio/v2/mail.proto) | `google_e2e::gmail_initial_sync_queries_and_original_downloads_work_through_cli`; `gmail_projection_and_cursor_survive_daemon_crashes_at_each_commit_boundary`; 10,000-message resource workload | Integrated regression passed; live byte/cursor compatibility pending |
| R04 Calendar catalog, canonical events and occurrences | Externally pending | [Calendar sync](../crates/nuncio-engine/src/calendar.rs), [Calendar API](../crates/nuncio-proto/proto/nuncio/v2/calendar.proto) | `google_e2e::calendar_canonical_and_expanded_agenda_are_available_through_actual_cli`; `support/calendar_cases` covers deltas, recurrence, tombstones and expired tokens | Integrated regression passed; live Calendar compatibility pending |
| R05 Drafts, composition, attachments and explicit send history | Externally pending | [draft domain](../crates/nuncio-engine/src/domain/drafts.rs), [draft store](../crates/nuncio-engine/src/store/drafts.rs), [operation worker](../crates/nuncio-engine/src/operations.rs) | `support/draft_e2e`, `support/send_e2e`, `support/send_crash_e2e`; SMTP E2E verifies independent delivery, MIME, private Bcc and separate Sent receipts | Integrated regression passed; named live recipient checks pending |
| R06 Mail flags, labels, folder copy/move, trash and restore | Externally pending | [mail changes](../crates/nuncio-engine/src/operations/mail_change.rs), [IMAP transfers](../crates/nuncio-engine/src/providers/imap/transfers.rs) | `support/mail_change_system`, `support/mail_change_e2e`; `imap_write_system` and `imap_folder_e2e`/`imap_trash_e2e` verify exact remote placements and unrelated-message preservation | Integrated regression passed; live mailbox compatibility pending; permanent purge excluded |
| R07 Calendar writes, scopes, RSVP, notifications and free/busy | Externally pending | [change validation](../crates/nuncio-engine/src/domain/calendar_change.rs), [Google writes](../crates/nuncio-engine/src/providers/google/calendar_write.rs) | `support/calendar_write_system`, `support/calendar_write_e2e`; new limited-writer tests check private denial, valid edits, restart and independent notification counts | Limited-writer full regression passed; integrated egress passed; named live event/notification checks pending |
| R08 Durable intent, idempotency, safe retry and uncertainty | Passed offline | [worker](../crates/nuncio-engine/src/operations.rs), [operation store](../crates/nuncio-engine/src/store/operations.rs), [Operations API](../crates/nuncio-proto/proto/nuncio/v2/operations.proto) | `operation_system`, `reconciliation_system`, Google/SMTP crash cases, restored send reconciliation; no blind retry after ambiguous sends/copies/notifications | Full integrated regression passed; provider uncertainty remains explicit by design |
| R09 Synology IMAP/SMTP mail parity | Externally pending | [IMAP transport](../crates/nuncio-engine/src/providers/imap/mod.rs), [SMTP](../crates/nuncio-engine/src/providers/imap/smtp.rs), [Sent handling](../crates/nuncio-engine/src/providers/imap/sent.rs) | `imap_contract`, `imap_system`, `imap_e2e`; independent Dovecot/Mailpit receipts, UID epochs, folder encoding, TLS, exact copy/flag/delivery observations | Optional LIST-hint cursor correction:7projection tests and full34-command regression pass. Live MailPlus version/capabilities and actual Sent policy unverified |
| R10 Scheduled sync, isolation, bounded work and observable progress | Passed offline | [scheduler](../crates/nuncio-engine/src/scheduler.rs), [resource budgets](../crates/nuncio-engine/src/resources.rs), [System status](../crates/nuncio-proto/proto/nuncio/v2/system.proto) | `google_e2e::daemon_polling_refreshes_mail_and_calendar_without_manual_sync`; scheduling/cancellation suites; `resource_system` admission recovery and `resource_e2e` status/restart checks | Full integrated regression passed; counters reset on process restart |
| R11 Export, encrypted backup/restore, migration and repair | Passed offline | [backup](../crates/nuncio-engine/src/store/backup.rs), [recovery](../crates/nuncio-engine/src/engine/recovery.rs), [Maintenance API](../crates/nuncio-proto/proto/nuncio/v2/maintenance.proto) | `recovery_e2e`, `migration_e2e`, `repair_system`, `repair_e2e`; 46 schema migration SIGKILL cases, five restore crash points, PDF/request fidelity and original preservation | Full integrated regression and final extracted-artifact checks passed |
| R12 CLI/API parity, paging, byte streams and change replay | Passed offline | [CLI](../crates/nuncio-cli/src/main.rs), [protobuf contract](../crates/nuncio-proto/proto/nuncio/v2), [independent generated client](../clients/smoke/README.md) | Actual CLI suites, `cli_change_watch_replays_jsonl_and_rejects_a_future_revision`; external `clients/smoke/tests/daemon.rs` status/watch test and normal/build dependency-tree check | Frozen descriptor/client/archive verified; full integrated runner passed |
| R13 Independent stateful Google mock and fault effects | Passed offline | [mock service](../crates/nuncio-test-support/src/google/mod.rs), [contract suite](../crates/nuncio-test-support/tests/google_mock_contract.rs) | 26 independent contract tests pass, including OAuth/HTTP validation, faults, remote state, notifications and limited-writer private-event enforcement | Full integrated regression passed; mocks cannot establish live equivalence |
| R14 Separate real-store system and daemon/CLI E2E tests | Passed offline | [process harness](../crates/nuncio-test-support/src/process.rs), [system harness](../crates/nuncio-test-support/tests/support/system.rs), [runner](../scripts/verify.py) | All required named suites exist and have prior recorded runs; full `--all` passed. Independent generated-client test also passes under actual macOS egress denial | Full local run and independent-server egress passed; all ten current hosted jobs34704460234 passed |
| R15 Encryption, key separation, auth/TLS, inert display and test-hook exclusion | Passed offline | [security suites](../crates/nuncio-test-support/tests/security_system.rs), [subprocess security](../crates/nuncio-test-support/tests/security_e2e.rs), [release isolation](../crates/nuncio-test-support/tests/release_isolation.rs) | 196 invalid-auth cases; wrong-key/ordinary-SQLite rejection; encrypted DB/WAL/FTS/backup and original-log canaries; hostile-content/resource bounds; fresh production hook exclusion | Dependency findings resolved; Clean 164b021 archive and full regression passed; actual security run34704460237 passed; native-keystore acceptance remains in R02 |
| R16 Reproducible artifacts, CI, operating instructions and final evidence | Externally pending | [API](API.md), [running](RUNNING.md), [recovery](RECOVERY.md), [manual worksheet](MANUAL-ACCEPTANCE.md) | Local production binaries/hashes and signed checkpoints recorded; external-client foundation verified; manual worksheet prepared | Local archive, descriptor, full offline gate, actual10-jobCI and hosted installer passed; named live/native acceptance remains pending |

Live worksheet rows G01–G10, S01–S05 and X01 are all unapproved/unverified. The
status-email connector is separately authorized and provides no rebuilt-provider
acceptance evidence. No native application, public release, merge or normal
environment installation is part of the completed work.
