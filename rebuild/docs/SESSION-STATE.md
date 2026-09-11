# Rebuild session state

Updated September 11, 2026. Full Tasks01–16/R01–R16 goal is active and unbudgeted. Continue inline from this worktree; do not recreate the goal or restart planning. Full local Google and Synology mocks are the current provider scope. Live acceptance remains deferred/unapproved/unverified; it does not block remaining offline work. Native apps are excluded.

## Authorization and workspace

Worktree: `/Users/james.maes/Git.Local/KofTwentyTwo/nuncio/.worktrees/nuncio-google-first-rebuild`; branch `feature/nuncio-google-first-rebuild`, baseline `a268fd5a004977b65fd0347ceeb1c69755529aa3`. Cargo cwd: its `rebuild/`. Original checkout remains on dev; original source/data/unrelated changes are preserved. James explicitly authorized an initial checkpoint commit and regular commits/pushes after coherent changes pass relevant checks, on feature/nuncio-google-first-rebuild at github.com/KofTwentyTwo/nuncio. Initial signed checkpoint `2480bf94cdcff15fcf0da886691bd872e38edf31` was committed and pushed; git ls-remote verified the identical GitHub branch head. Gitleaks staged scan, whitespace check and byte equality for all389 staged files passed. Details are in VERIFICATION and test-results/checkpoint-initial/. Merges, releases, installation into the normal environment, remote settings changes, and live acceptance actions remain unapproved. The executive report was separately emailed to james@kof22.com using connected Gmail on explicit request; this is not rebuild-provider acceptance evidence.

Read AGENTS→CLAUDE, shared personal rules/style, rebuild/AGENTS, this file, TODO and newest VERIFICATION entries after reset. Approved spec/plan and September10 engineering audit live in the worktree's `docs/`, one above Cargo cwd. User instructions override issue/replanning/delegation/commit defaults.

## Immediate next action

**All known command handles are terminal. Startup gate shell87318 completed all7 commands exit0:** fmt/bothClippy, engine126, recoveryE2E4, IMAPE2E13, release-isolation2 with fresh production. `test-results/task13-startup-cleanup/summarize.py` completed0 and recorded results/counts/source hashes/artifact hashes. Daemon SHA256 `f67a6d53b62cd88d80f3e5b8f51d39fb6d36c0e14de562ba92f79563cf49acb1`; CLI `23f75c75837bc64b25a40cdac3d5a3e0abcff436bbf8d75d161664ac3659d4b1`. No failed/ignored tests. No schema/proto/dependency change. The initial checkpoint has been committed/pushed and its signature/remote hash verified. Keep checkpointing coherent verified changes with this standing authorization; merges/releases/install/live acceptance remain unapproved.

Next extend Task13 durable cleanup to earlier upload/backup artifact lifetimes and ordinary failed-job retirement. A restore job is currently recorded once staging begins; interrupted earlier uploads/exports have no recovery record. Normal failed restores leave a row until startup; repeated failures can consume the bounded64-row capacity. Start with an actual daemon/CLI crash test at upload/export boundaries or a deterministic repeated-failure regression, then fix with explicit ownership and independently verify source keys/files. Preserve unrecognized empty directories when no durable ownership exists. Do not implement filename-prefix sweeps. Static caller-owned restore helper and broader Task13–16 obligations below remain.

## Current code changes

Startup cancellation: `engine.rs::open_inner` creates Arc<File> profile ownership immediately after profile preparation. `engine/recovery/journal.rs::recover` clones it into each blocking cleanup closure. Cancelling Engine::open therefore cannot allow another engine to overlap still-running cleanup. Existing deterministic SecretStore.delete barrier test proves Locked during cleanup, normal restart afterward, original source keys/manifest, and removal of only owned stage/upload. Direct journal fixtures acquire a real profile file lock.

Relative paths: Journal creation and startup canonicalize the owner using directory-identity validation. `Journal::record` also normalizes the upload's owner before comparison, permitting macOS `/var` aliases while rejecting another owner's input. No final-component owner symlink is accepted. Tests cover failed restore reaching encrypted inspection, an actual persisted cleanup row, startup retirement, successful one-component relative restore with independent identity/API keys, owner symlink preservation, ancestor alias acceptance, and foreign-owner upload rejection. Source locations: `engine/recovery.rs`, `engine/recovery/journal{,_tests}.rs`, `tests/profile_lifecycle.rs`. Schema22, proto and dependencies are unchanged.

## Evidence for this change

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

Task13: earlier maintenance process-death cleanup and ordinary-failure retirement; caller-owned helper boundary; wider storage/WAL/export failures; historical durable payloads for other writes and disconnect retention/same-identity reconnect. Rich send histories10–21 and schema22 migration/crash cases are verified. No destructive account purge.

Task14: create/run named security_system, resource_system and multi_engine_system suites. Require three independent engines,10,000 metadata messages,16MiB attachments,64MiB payload limits, concurrency2, bounded queues, measured RSS/latency, canary scans and independent encryption checks. Audit actual outstanding defects rather than repeat completed fixes: post-submission storage failures, worker failures, parser bounds, aggregate attachments, legacy intents, calendar inheritance/Trash origins; writerWithoutPrivateAccess remains unverified. Run the complete offline verifier after implementing the missing suites.

Tasks03/15: explicit test egress denial (Docker bridge alone is not a firewall); external descriptor-generated client and contract freeze/docs; dependency advisory/security/license checks; package.py and extracted artifacts; CI definitions and required-job failure behavior. Task16: final R01–R16 evidence matrix plus operating/recovery/compatibility/risk report. MANUAL-ACCEPTANCE.md exists; all live acceptance is unapproved/unverified. Passing local mocks does not prove live Google/MailPlus compatibility; local checks do not prove remote CI ran.

Local binary locations: `target/production/release/{nunciod,nuncio-cli}` and feature-only `target/test-harness/debug/{nunciod,nuncio-cli}`. Production hashes were refreshed by the startup-cleanup gate above. These are not installed or packaged releases. No full Task13 or R01–R16 completion claim.
