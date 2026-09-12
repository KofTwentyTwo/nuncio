# Rebuild session state

Updated September 11, 2026. Full Tasks01–16/R01–R16 goal is active and unbudgeted. Continue inline from this worktree; do not recreate the goal or restart planning. Full local Google and Synology mocks are the current provider scope. Live acceptance remains deferred/unapproved/unverified; it does not block remaining offline work. Native apps are excluded.

## Authorization and workspace

Worktree: `/Users/james.maes/Git.Local/KofTwentyTwo/nuncio/.worktrees/nuncio-google-first-rebuild`; branch `feature/nuncio-google-first-rebuild`, baseline `a268fd5a004977b65fd0347ceeb1c69755529aa3`. Cargo cwd: its `rebuild/`. Original checkout remains on dev; original source/data/unrelated changes are preserved. James explicitly authorized an initial checkpoint commit and regular commits/pushes after coherent changes pass relevant checks, on feature/nuncio-google-first-rebuild at github.com/KofTwentyTwo/nuncio. Initial signed checkpoint `2480bf94cdcff15fcf0da886691bd872e38edf31` was committed and pushed; git ls-remote verified the identical GitHub branch head. Gitleaks staged scan, whitespace check and byte equality for all389 staged files passed. Details are in VERIFICATION and test-results/checkpoint-initial/. Merges, releases, installation into the normal environment, remote settings changes, and live acceptance actions remain unapproved. The executive report was separately emailed to james@kof22.com using connected Gmail on explicit request; this is not rebuild-provider acceptance evidence.

Read AGENTS→CLAUDE, shared personal rules/style, rebuild/AGENTS, this file, TODO and newest VERIFICATION entries after reset. Approved spec/plan and September10 engineering audit live in the worktree's `docs/`, one above Cargo cwd. User instructions override issue/replanning/delegation/commit defaults.

## Immediate next action

Continue the approved goal inline; finish demonstrated R01–R16 failures and missing
Task14/15 deliverables. Do not reopen speculative recovery audits or completed work.

Last pushed checkpoint: `c1180f1dc8257eabbbb3874e0d692ccf335c5a26` (resource workloads).
Prior security checkpoint: `8811acdd14f86784993d525de24c582f3f010563`. Both signatures,
pushes, exact remote hashes, staged Gitleaks and whitespace checks passed. Security
nine-command plus four-command follow-up gates passed; resource six-command gate
passed (shell34619, resource_system2/resourceE2E2/securityE2E2 andfmt/bothClippy).
Evidence: `test-results/task14-security{,-trap}/` and `task14-resource-workloads/`.
Implementation locations and precise limits/counts are in newest VERIFICATION.

Verified changes ready for signed checkpoint/push bound account-request admission to64 (active limit2),
preallocate bounded Google response buffers to reduce allocator churn, and defer
queued operation preparation when admission is full. No schema/proto/dependency
change. Test-only RSS diagnostics retain samples before assertions and optionally
capture content-free native allocation summaries; the128MiB post-warm-up RSS
bound remains unchanged.

The14-command gate `python3 test-results/task14-resource-admission/run-gate.py`
completed all0 (shell98949): fmt/bothClippy, engine129, resource system3/E2E2,
security3/2, Google21/26, operations6, IMAP26/13, production-isolation2. Counts,
source hashes, artifact hashes, metrics and logs are retained alongside results.json.
This gate predates the operation-worker fix below. Prior failed RSS gate14707/1
and investigation evidence remain in task14-queue-admission/; VERIFICATION records
the failure, allocation evidence and subsequent unchanged-bound passes.

New regression reproduced a direct admission interaction: full account-request
capacity caused operation preparation to return Busy and permanently stop the
worker (shell60230/101). It confirms no attempt/send occurred while saturated.
Minimal fix treats pre-attempt Busy as deferral, keeping durable intent eligible
for a later worker tick. Focused verification shell49015 passed1/exit0:
`python3 test-results/task14-operation-admission/run-focused.py green`.
Test verifies worker health, exactly one applied attempt and independent accepted
send after capacity returns, without restart. Nine-command affected gate81153
completed all0: fmt/bothClippy, engine129, resource_system4, operation_system6,
GoogleE2E26, IMAPE2E13 and release_isolation2. Fresh normal artifacts include the
fix. Exact results/counts/source/artifact hashes: task14-operation-admission/.
Immediate action: signed checkpoint/push these verified fixes, then continue below.

After that: shared admission for spawned mail/calendar jobs (including backoff
waiters), actual request/queue/byte/storage-batch instrumentation through engine,
API and CLI, independent concurrency saturation and actual process observations.
Existing Store queue64, upload4 and compose1 limits must remain intact. Completed
workloads cover10,000 messages/100 provider+API pages, configured body refusal,
8 exact16MiB attachment cycles with child RSS/latency, and64MiB+1 CLI-file refusal.
Do not repeat those as new deliverables; integrate their evidence with new metrics.
Then full offline verifier and Task15 contract/egress/dependency/package/CI work.

## Hourly status emails

James authorized hourly progress emails to james@kof22.com beginning immediately.
First hourly email sent September11 at approximately22:57UTC/17:57Central using
connected Gmail, subject `Nuncio rebuild — hourly status — September 11, 2026,
5:57 p.m. CT`; Gmail ID/thread `1a092b0a4e84f109`, SENT confirmed.
Second hourly email sent September11 23:57UTC/18:57Central, confirmed at23:57:17UTC;
ID/thread `1a092e77786bec8b`, SENT. Includes verified security/workload checkpoints
and the new RSS failure. Exact body/receipt: test-results/status-emails/2026-09-11-1857.md.
Next due: September12 00:57UTC / September11 19:57Central. Check the clock during active goal execution;
send one concise update each hour with actual progress, test results, blockers,
remaining deliverables and next action, then update this timestamp and message ID.
Do not send catch-up bursts or infer progress while paused. This authorization is
for project status mail, not rebuilt-provider acceptance. No persistent schedule
is configured: no thread scheduling tool is exposed, and Computer Use refuses
Codex access. The user was told that delivery during pauses/stops is unconfigured.

## Current local artifact evidence

Operation-admission gate shell81153 completed all9 commands exit0. Daemon:
`target/production/release/nunciod`,16352624 bytes,SHA256
`a9170b24aa58eabc14e125d28a4f7b50e40609526d19114b54def74a1175d347`.
CLI:`target/production/release/nuncio-cli`,3506544 bytes,SHA256
`23f75c75837bc64b25a40cdac3d5a3e0abcff436bbf8d75d161664ac3659d4b1`.
Both exclude production test hooks. These are local binaries, not packaged or
installed releases. No remote-CI/live-compatibility claim. Older startup evidence
below remains valid for that earlier checkpoint.

## Historical startup-cleanup checkpoint

Startup cancellation: `engine.rs::open_inner` creates Arc<File> profile ownership immediately after profile preparation. `engine/recovery/journal.rs::recover` clones it into each blocking cleanup closure. Cancelling Engine::open therefore cannot allow another engine to overlap still-running cleanup. Existing deterministic SecretStore.delete barrier test proves Locked during cleanup, normal restart afterward, original source keys/manifest, and removal of only owned stage/upload. Direct journal fixtures acquire a real profile file lock.

Relative paths: Journal creation and startup canonicalize the owner using directory-identity validation. `Journal::record` also normalizes the upload's owner before comparison, permitting macOS `/var` aliases while rejecting another owner's input. No final-component owner symlink is accepted. Tests cover failed restore reaching encrypted inspection, an actual persisted cleanup row, startup retirement, successful one-component relative restore with independent identity/API keys, owner symlink preservation, ancestor alias acceptance, and foreign-owner upload rejection. Source locations: `engine/recovery.rs`, `engine/recovery/journal{,_tests}.rs`, `tests/profile_lifecycle.rs`. Schema22, proto and dependencies are unchanged.

## Historical startup-cleanup evidence

All logs: `test-results/task13-startup-cleanup/`.

- Original startup red: shell98698/101, red-corrected-fixture.log (1 failed). Earlier red.log includes a fixture mutex-lifetime bug, corrected before the product failure was confirmed.
- Lock fix: shell82528/0, green-lock.log, journal4 passed.
- Strengthened relative red: shell82643/101, relative-strong-red.log, InvalidPath before encrypted inspection. The earlier relative-red.log passing test was too weak and is not accepted as recovery proof.
- Relative fix: shell75532/0, relative-green.log, profile5 passed.
- First two broad attempts stopped on new-test Clippy/type errors (shell24447/101, shell19129/101); preserved as gate-first-* and gate-second-*.
- Next broad attempt (shell71857/101) passed fmt/bothClippy but exposed `/var` versus canonical upload-owner mismatch in maintenance2tests; gate-alias-failure-* preserves output. Product comparison fixed, no assertions weakened.
- Focused alias/path/lifecycle run: shell56304/0, alias-focused.log, 13 passed/0 failed/0 ignored. Includes successful relative restore, existing cancellation ownership, path replacement and cleanup-failure cases.
- Full gate shell87318 is terminal: all7 commands0, engine126/recovery4/IMAP13/release2. New library alias/foreign-owner and owner-symlink checks passed; source/artifact hashes retained.

## Previously verified foundation

Last fully verified gate before these changes: `task13-maintenance-ownership`, all7 commands0, engine122/recoveryE2E4/IMAPE2E13/release-isolation2 plus format/bothClippy. Preceding `task13-restore-crash`, all12 commands0: engine119, protoCLI/daemon20, Google system21/E2E26, IMAP system26/E2E13, migrationE2E1, recoveryE2E4, release2 and format/bothClippy.

The schema22 migration E2E includes44 actual SIGKILL cases, before/after every migration commit from schemas0–21, with two verified restarts each; frozen histories1–21 remain unchanged. `migration-case-index.json` identifies exact evidence. Recovery E2E includes five restore kill boundaries: owned empty stage before export, completed stage, first key, second key, activation. Original profile/keys/draft/backup and independent provider state survive; activated targets retain keys. Separate recovery tests verify real PDF/raw bytes and held request identity through engine/API/CLI.

Earlier Sent/reconciliation/path/worker checkpoints and exact implementation locations are retained in VERIFICATION and `docs/history/2026-09-11-startup-cleanup-working-history.md`. Do not reimplement completed work.

## Invariants to preserve

Schema22 source-owned restore cleanup rows record owner/parent/stage/upload identities and activation intent. Cleanup preflights bounded allowlisted entries through directory handles before new-key deletion. Replaced/uncertain paths retain evidence and keys. Backups/restores exclude cleanup authority. Successful activation retains new keys even if later upload cleanup/fsync/reply fails. A closed Store leaves the durable row for startup.

MaintenanceLease retains the transfer semaphore and source profile lock through returned backup artifacts, upload/input objects and blocking work. Engine::restore_backup is the production RPC path. Engine::restore_profile is a caller-owned lower-level integration helper; do not silently claim it has a source cleanup journal. Avoid store-worker self-deadlocks and keep ownership after cancellation/shutdown.

Schema21 explicit reconciliation is durable/scoped/versioned through engine/API/CLI. Observe never mutates providers. Positive exact Sent evidence can resolve an accepted SMTP copy; missing/ambiguous reads never authorize resend/APPEND. SMTP acceptance, copies, and calendar notification uncertainty remain independently represented. Restored pending work remains held until an explicit safe reconciliation/duplicate-risk decision. Original requests, frozen MIME/private Bcc, receipts and attempt ordinals survive.

## Remaining full scope

Task13: restore-retry verification passed and checkpoint57dd648 is pushed. Existing encrypted WAL backup, rekey/hold, historical schema, projection repair and subprocess recovery evidence covers the functional task. Reconcile any remaining explicit Task13 acceptance gaps against that evidence during the full gate. Earlier artifact-lifetime and caller-owned-helper audit ideas are not an open-ended prerequisite for Task14. Rich send histories10–21 and schema22 migration/crash cases are verified. No destructive account purge.

Task14: multi_engine_system and security_system/security_e2e with relevant gates pass. Create/run resource_system and actual subprocess resource checks next. Require three independent engines,10,000 metadata messages,16MiB attachments,64MiB payload limits, concurrency2, bounded queues, measured RSS/latency, canary scans and independent encryption checks. Audit actual outstanding defects rather than repeat completed fixes: post-submission storage failures, worker failures, parser bounds, aggregate attachments, legacy intents, calendar inheritance/Trash origins; writerWithoutPrivateAccess remains unverified. Run the complete offline verifier after implementing the missing suites.

Tasks03/15: explicit test egress denial (Docker bridge alone is not a firewall); external descriptor-generated client and contract freeze/docs; dependency advisory/security/license checks; package.py and extracted artifacts; CI definitions and required-job failure behavior. Task16: final R01–R16 evidence matrix plus operating/recovery/compatibility/risk report. MANUAL-ACCEPTANCE.md exists; all live acceptance is unapproved/unverified. Passing local mocks does not prove live Google/MailPlus compatibility; local checks do not prove remote CI ran.

Local binary locations: `target/production/release/{nunciod,nuncio-cli}` and feature-only `target/test-harness/debug/{nunciod,nuncio-cli}`. Production hashes were refreshed by the restore-retry gate above. These are not installed or packaged releases. No full Task13 or R01–R16 completion claim.
