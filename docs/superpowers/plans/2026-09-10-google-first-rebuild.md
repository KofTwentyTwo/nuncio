# Nuncio Google-First Rebuild Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task by task. Steps use checkboxes for tracking. Execute inline unless James requests delegation. Checkpoints record evidence; they do not require another approval for work already authorized by the execution goal.

**Goal:** Fully implement a dependable personal email/calendar engine and CLI for Google, followed by Synology MailPlus email, with independent provider mocks, system tests, subprocess E2E, recovery, and verified local release artifacts.

**Architecture:** A separate Rust workspace under `rebuild/` contains the headless engine, versioned protobuf contract, daemon, CLI, and unshipped test support. The daemon owns credentials, encrypted SQLite, synchronization, and durable operation execution. Provider projections can be rebuilt; drafts and operation intent survive resync and recovery.

**Tech Stack:** Rust 2021 / 1.97.1, Tokio, tonic/prost, Clap, serde, reqwest/rustls, rusqlite with SQLCipher/FTS5, OS keyring, established MIME and IMAP/SMTP libraries.

**Spec:** [Google-first rebuild specification](../specs/2026-09-10-google-first-rebuild.md). Read it together with this plan, the [audit](../../reviews/2026-09-10-nuncio-audit-and-rebuild.md), and the [execution goal](../../GOAL-nuncio-rebuild.md).

**Status:** Accepted and activated by James on September 10, 2026. Execution state is in `rebuild/docs/SESSION-STATE.md` within `.worktrees/nuncio-google-first-rebuild`. This plan governs the isolated rebuild; the root roadmap remains historical guidance for the original application. Latest steering requires full local mock services for both Google and Synology-compatible IMAP/SMTP; live-account acceptance is deferred.

## Global Constraints

- Primary personal runtime: macOS. Linux runs the offline CI suites. Native applications and Windows installation are outside this goal.
- Google Gmail and Google Calendar first; Synology MailPlus through IMAP/SMTP second. No Synology Calendar/CalDAV in this goal.
- Four production crates: `nuncio-engine`, `nuncio-proto`, `nunciod`, `nuncio-cli`. `nuncio-test-support` is never shipped.
- Use Rust 2021, the repository's pinned Rust 1.97.1, and warnings as errors. No production `unsafe`, `unwrap`, `expect`, `panic`, or placeholder success paths.
- Default endpoint `127.0.0.1:9421`; profile root `~/.nuncio-rebuild`; keyring service `mx.nuncio.rebuild`; API package `nuncio.v2`.
- SQLite/FTS/payloads/journal are encrypted through SQLCipher. Use a dedicated storage thread, typed requests, and memory temporary storage. No plaintext fallback.
- Blob chunks: 256 KiB. Default maximum single payload: 64 MiB, configurable downward. Bound raw, decoded, streamed, and decompressed sizes independently.
- Calendar agenda coverage: 30 days before today through 180 days after today, separate from canonical sync tokens.
- Default polling: 60 seconds; active-account limit: 2; request deadline: 30 seconds; retained change records: 10,000.
- All automated tests use synthetic data and local providers. No live credentials or live provider calls in tests or CI.
- Every capability needs engine behavior, authenticated RPC, CLI support, and offline E2E. Mock behavior alone cannot satisfy a product requirement.
- Preserve the original workspace, original data, unrelated changes, and old API. Do not migrate existing user state under this goal.
- Local implementation and verification are authorized only when the goal is adopted. Commits, pushes, merges, publication, installation into the user's PATH, and live provider actions require their stated authorization.

## Delivery sequence

| Milestone | Tasks | Reviewable outcome |
|---|---|---|
| A — foundation and test system | 01–04 | Encrypted daemon/CLI runs; independent mock Google, real test harness, and OAuth lifecycle work. |
| B — usable Google reader | 05–08 | Two-account Gmail and Calendar sync, offline mail/search/agenda, background recovery, observable API. |
| C — usable Google client | 09–11 | Durable drafts, sending, mail changes, calendar changes, conflicts, and uncertain outcomes. |
| D — full provider scope | 12–13 | Synology IMAP/SMTP plus safe export, backup, restore, and migrations. |
| E — completed release candidate | 14–16 | Failure/security checks, local packages, required CI definitions, and explicit provider acceptance evidence. |

Complete each milestone before starting the next. Complete Gmail and Calendar read-only acceptance before adding write operations. Establish the mock before implementing the production Google adapter. A milestone is a checkpoint, not permission to stop the overall goal.

## File and interface map

All paths in the following map are relative to `rebuild/`. Each task below identifies its files within this map. Create Rust module declarations and crate manifests as their first consumers need them; do not create empty speculative modules.

| Path | Responsibility |
|---|---|
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/config.toml` | Independent workspace, pinned toolchain/dependencies, lint settings. |
| `crates/nuncio-engine/src/domain/{identity,mail,calendar,operations}.rs` | Stable account-scoped identity, time, draft, and operation types. |
| `crates/nuncio-engine/src/store/{mod,worker,migrations,mail,calendar,operations,backup}.rs` | SQLCipher connection and typed storage operations on a dedicated thread. |
| `crates/nuncio-engine/src/secrets/{mod,keyring,test_store}.rs` | Secret interface, production OS keyring, feature-gated deterministic test secret store. |
| `crates/nuncio-engine/src/{engine,accounts,mail,calendar,operations,scheduler,changes}.rs` | Application operations; all provider/store composition remains here. |
| `crates/nuncio-engine/src/providers/google/{http,oauth,gmail,calendar}.rs` | Google wire types, HTTP requests, pagination, error classification. |
| `crates/nuncio-engine/src/providers/{imap,smtp}.rs` | IMAP placements, capability negotiation, SMTP delivery and Sent bookkeeping. |
| `crates/nuncio-proto/proto/nuncio/v2/{common,system,accounts,mail,calendar,operations,maintenance}.proto` | Public service contract and field documentation. |
| `crates/nuncio-proto/{build.rs,src/lib.rs,src/client.rs,tests/contract.rs}` | Bindings, authenticated client helper, descriptor compatibility test. |
| `crates/nunciod/src/{main,config,server}.rs`, `src/services/*.rs` | Lifecycle, configuration, one thin handler module per service, authentication. |
| `crates/nuncio-cli/src/{main,args,output}.rs`, `src/commands/*.rs` | CLI grammar, client-side file handling, output, exit statuses. |
| `crates/nuncio-test-support/src/google/{mod,state,oauth,gmail,calendar,faults}.rs` | Independent stateful Google HTTP service. |
| `crates/nuncio-test-support/src/{fixtures,process,imap}.rs`, `src/bin/mock-google.rs` | Synthetic fixtures, subprocess harness, independent mail-server orchestration. |
| `crates/nuncio-test-support/tests/support/{mod,system}.rs` | System harness composition using production engine/proto dev dependencies, excluded from the mock binary. |
| `crates/nuncio-test-support/fixtures/{google,mime,calendar}` | Hand-authored wire examples and expected bytes, with source/provenance notes. |
| `crates/nuncio-test-support/tests/*.rs` | Provider conformance, system, E2E, compatibility, and failure scenarios. |
| `scripts/{verify.py,package.py}`, `tests/imap/compose.yaml` | Reproducible validation, local artifacts, pinned local IMAP/SMTP services. |
| `docs/{SESSION-STATE.md,TODO.md,VERIFICATION.md,RUNNING.md,TESTING.md,API.md,RECOVERY.md}` | Execution state, evidence, operation and client contracts. |

`Engine` is the application entry point, holding provider registries, store handle, and workers. Its methods consume domain request types and return `Result<T, EngineError>`. RPC handlers convert protobuf values into these types; the CLI uses protobuf clients only. Avoid a universal provider trait that erases Gmail labels, IMAP placements, or calendar versions. Share transport/retry utilities where semantics actually match.

The operation envelope is common; action payloads are typed variants:

```rust
pub struct RequestIdentity {
    pub account_id: AccountId,
    pub request_id: String,
}

pub enum OperationState {
    Queued,
    Running,
    Applied,
    RetryWait,
    Conflict,
    Uncertain,
    Failed,
    Cancelled,
}
```

Task 01 defines `AccountId` as a validated opaque local identifier. Task 09 defines `OperationId`, `Operation`, the canonical payload fingerprint, and the full transition implementation. The request ID is client-generated; a new RPC transport request is not a new operation.

### RPC ownership

The names below are the required public methods, grouped by the task that introduces them. Define request/response fields alongside each vertical; reserve deleted field numbers and update the descriptor golden deliberately.

| Task | Service methods |
|---|---|
| 01 | `System.GetStatus`, `System.Shutdown` |
| 04 | `Accounts.BeginGoogleAuth`, `Accounts.GetAuthStatus`, `Accounts.ListAccounts`, `Accounts.DisconnectAccount` |
| 05 | `Accounts.SyncAccount`, `Accounts.GetSyncStatus`, `Accounts.CancelSync`, `Mail.ListCollections`, `Mail.ListMessages`, `Mail.GetMessage`, `Mail.SearchMessages`, `Mail.FetchMessage`, `Mail.DownloadRaw`, `Mail.DownloadAttachment` |
| 07 | `Calendar.ListCalendars`, `Calendar.GetEvent`, `Calendar.ListAgenda`, `Calendar.RefreshAgenda` |
| 08 | `System.WatchChanges` and expanded sync status in `System.GetStatus` |
| 09 | `Mail.SaveDraft`, `Mail.ListDrafts`, `Mail.GetDraft`, `Mail.DeleteDraft`, `Mail.PrepareReply`, `Mail.PrepareForward`, `Mail.UploadAttachment`; `Operations.ListOperations`, `Operations.GetOperation`, `Operations.CancelOperation`, `Operations.ResolveOperation` |
| 10 | `Mail.SendDraft`, `Mail.ChangeMessage` |
| 11 | `Calendar.ChangeEvent`, `Calendar.QueryFreeBusy` |
| 12 | `Accounts.AddImapAccount`; provider capability handling in existing mail methods |
| 13 | `Maintenance.CreateBackup`, `Maintenance.InspectBackup`, `Maintenance.RestoreBackup`, `Maintenance.RepairProjection` |

`PrepareReply` takes explicit reply/reply-all mode. `ChangeEvent` has typed create/update/delete/respond variants and explicit series/occurrence scope. Download/create-backup responses stream bounded byte chunks; uploads/restore use bounded client streams. File paths are client concerns, never arbitrary daemon file-read RPCs. Auth sessions and sync-run handles are inspectable separately from durable write operations.

## Execution method and verification commands

For each checked step, record the implementation path and evidence in `rebuild/docs/VERIFICATION.md`. Start each behavior with a failing assertion that demonstrates the intended failure, implement it, then rerun that assertion. Do not interpret a compiler error or a missing binary as proof that the behavior was exercised. Finish each task with its whole suite and applicable lint checks. Record the next task and unresolved issues in the local session-state files. Commit only if separately authorized.

Task 03 creates a Python standard-library runner with this stable interface, invoked from the repository root:

```bash
python3 rebuild/scripts/verify.py --suite google_mock_contract
python3 rebuild/scripts/verify.py --suite google_system
python3 rebuild/scripts/verify.py --suite google_e2e
python3 rebuild/scripts/verify.py --all
```

`--suite NAME` builds test-harness daemon/CLI binaries in `rebuild/target/test-harness`, sets absolute `NUNCIO_E2E_DAEMON` / `NUNCIO_E2E_CLI` paths, and runs `cargo test --manifest-path rebuild/Cargo.toml -p nuncio-test-support --test NAME`. `--all` runs fmt, Clippy, unit/contract/system/E2E/IMAP suites, then release-isolation checks. It uses `RUST_TEST_THREADS=2`, checks every subprocess exit status, and fails when required prerequisites or suites are missing. Test integration code fails with an actionable message if binary paths are absent; it never silently skips.

Run lint in both normal and test-harness configurations. Build release artifacts in a different target directory with production packages selected and **without** test features, avoiding Cargo feature unification contaminating release checks. The equivalent basic gates are:

```bash
cargo fmt --manifest-path rebuild/Cargo.toml --all -- --check
cargo clippy --manifest-path rebuild/Cargo.toml --workspace --all-targets -- -D warnings
cargo clippy --manifest-path rebuild/Cargo.toml --workspace --all-targets --features nunciod/test-harness -- -D warnings
```

The runner supplies the E2E binary environment when invoking workspace tests. Before Task 03 exists, run the exact package/test commands stated below. Root gates remain unchanged; run them if root runtime/build files are touched. Sandbox port denial is an environment restriction: request narrow escalation, retaining isolation and synthetic data.

## Task 01 — Isolated workspace, encrypted lifecycle, and status vertical

**Depends on:** Adoption of the execution goal. **Requirements:** R01, R12, R15.

**Files:** Create workspace manifests/config, `domain/identity.rs`, `store/{mod,worker,migrations}.rs`, `secrets/{mod,keyring}.rs`, engine entry point, System proto/client, daemon lifecycle, CLI system commands, `nuncio-engine/tests/store_lifecycle.rs`, and `nunciod/tests/lifecycle.rs`.

**Interfaces:** Define `AccountId` and `ProfileId` string newtypes with validated parsing; `SecretStore` get/put/delete of zeroizing secret bytes by profile/account/key purpose; `Store::open(StoreConfig, SecretStore)`; async typed store requests; `Engine::open(EngineConfig)`; `Engine::status()`; `Engine::shutdown()`. Status contains build/API version, profile ID, account states, storage state, revision, and protected data paths, never key material.

- [x] Record the starting SHA and dirty files; create/use `feature/nuncio-google-first-rebuild` only under the adopted goal's branch authorization. Carry plan/spec into an isolated worktree if one is needed; do not stash or move unrelated work. Record no issue required for this personal build when the goal says so.
- [x] Create the nested workspace, lints, manifests, and minimal store/lifecycle tests. Verify an encrypted store opens with its key, fails with a wrong key and ordinary SQLite, and retains data through close/reopen.
- [x] Implement the storage worker and migration transaction. Turn on foreign keys, WAL, bounded busy timeout, memory temp storage, and required SQLCipher settings before data access. Use bounded queues; never block a Tokio worker on SQLite. A second daemon cannot own the same store.
- [x] Implement OS keyring access and distinct database/API/provider key purposes. A missing key for an existing database fails closed; never generate a replacement key over it. Create data directories with restrictive permissions.
- [x] Implement authenticated status and graceful shutdown through the real CLI. Bind only loopback, acquire profile lock first, stop admission and workers before closing store. Read only bounded header bytes during startup checks.
- [x] Run `cargo test --manifest-path rebuild/Cargo.toml -p nuncio-engine --test store_lifecycle` and `cargo test --manifest-path rebuild/Cargo.toml -p nunciod --test lifecycle`; run the normal fmt/Clippy gates. Record wrong-key, second-owner, non-loopback, and missing-daemon exit results.

Core schema identity rule to test, not merely document:

```sql
CREATE TABLE accounts (id TEXT PRIMARY KEY NOT NULL);
CREATE TABLE messages (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    PRIMARY KEY (account_id, id),
    UNIQUE (account_id, provider_id)
);
```

This is the identity fragment of migration 1; add message fields when Task 05 introduces them. Memberships reference `(account_id, id)`. Identical provider IDs in different accounts must coexist.

## Task 02 — Independent stateful mock Google

**Depends on:** 01. **Requirements:** R13, R14.

**Files:** Create test-support crate, Google modules, fixtures/provenance notes, `src/bin/mock-google.rs`, and `tests/google_mock_contract.rs`.

**Interfaces:** `MockGoogle::start(Seed) -> Result<MockGoogle, TestError>` binds ephemeral loopback sockets; `Seed::TwoAccounts` supplies deterministic synthetic account identifiers. `MockGoogle::base_url()`, `control()`, and async `shutdown()`. `GoogleControl` exposes typed seed/edit/delete operations, `set_page_cap(usize)`, `expire_cursor(account, scope)`, `inject(Fault)`, `snapshot()`, and `accepted_sends(account)`. Snapshots expose remote IDs, labels, events, and effect counters. Fault selectors identify endpoint, account, call ordinal, and before/after-acceptance phase.

- [x] Write raw HTTP conformance tests independent of engine/proto structs. Cover valid and invalid field names/types, identity, label semantics, pagination, OAuth state/PKCE/code reuse, and history/sync expiry.
- [x] Implement separate mock wire structs or hand-authored JSON schemas. Reject unsupported routes and invalid operations explicitly. Use synthetic MIME and fixed calendar occurrence fixtures grounded in the specification's official references.
- [x] Implement all required OAuth/Gmail/Calendar endpoint subsets from spec section 6. Store accounts, tokens, messages, labels, histories, calendars, versions, recurring instances, and side-effect counters independently of the client.
- [x] Implement opaque page tokens scoped to query/account/generation, with repeat requests returning the same page until expiry; keep provider sync cursors separate. Enforce configured page caps even if the client requests more. Include sparse history IDs above 2^53 as JSON strings. OAuth authorization codes, unlike page tokens, are one-use.
- [x] Add fault controls for every listed status/body/connection fault, including apply-then-drop. Prove the drop occurs after state/effect counters change. Keep controls on an out-of-band harness channel, inaccessible to the adapter's base URL.
- [x] Add standalone reset/seed/start operation, readiness file, deterministic shutdown, and usage instructions. Run `cargo test --manifest-path rebuild/Cargo.toml -p nuncio-test-support --test google_mock_contract`.

Required hand-authored history response fixture:

```json
{
  "history": [{
    "id": "9007199254740997",
    "labelsAdded": [{
      "message": {"id": "m-001", "threadId": "t-001"},
      "labelIds": ["STARRED"]
    }]
  }],
  "historyId": "9007199254741009",
  "nextPageToken": "page-account-a-2"
}
```

The next response has no page token and returns the final durable `historyId`. Contract tests assert that using a page token as `startHistoryId` fails. The mock's fixture provenance includes a source URL and the field/behavior justified by it; credentials and personal content are forbidden.

## Task 03 — System and subprocess E2E harnesses

**Depends on:** 01–02. **Requirements:** R01, R12–R15.

**Files:** Create test-support `src/{process,fixtures}.rs` and `tests/support/{mod,system}.rs`; feature-gated engine test secret store and daemon test configuration; `scripts/verify.py`; `tests/{google_system,google_e2e,release_isolation}.rs`; `docs/TESTING.md`.

**Interfaces:** `SystemHarness` runs real Engine, SQLCipher, and authenticated RPC on loopback with real HTTP adapters. `E2eHarness` owns actual child binaries, profile directory, mock Google, and test credentials. Both support bounded shutdown; E2E additionally supports force-kill/restart while retaining profile and mock state. `CliOutput` contains `status: i32`, stdout/stderr bytes, and `json() -> Result<serde_json::Value, TestError>`. `E2eHarness::cli(&[&str]) -> Result<CliOutput, TestError>` invokes the binary directly without a shell. Fixtures and query helpers use public API; the mock never imports engine provider serialization types.

- [x] Add a process test for successful authenticated status, unauthenticated rejection, JSON stderr separation, and nonzero missing-daemon exit. Before implementation, assert the precise missing behavior.
- [x] Implement readiness-file plus authenticated-health waiting, deadline-based polling, child log capture with redaction, and cleanup on success/failure. Preserve logs on test failure. No fixed sleeps or global fixed ports.
- [x] Add test-only injection for loopback provider URLs, dummy secrets, clock/limits, and deterministic operation barriers. Compile it only with `test-harness`; production cannot enable it through an environment variable. Test credentials are synthetic and cannot name production keyring entries.
- [ ] Implement the runner defined above. Clear inherited provider credential/config variables for children, bind all mocks locally, and configure HTTP clients without ambient proxies in tests. CI denies external provider egress while allowing loopback and pre-pulled local containers.
- [x] Add release binary rejection of every test-only flag/env setting. Keep feature and production builds in separate target directories.
- [x] Run `python3 rebuild/scripts/verify.py --suite google_e2e` and `--suite release_isolation`. Capture the subprocess exit codes, not just logged result objects.

Required executable assertion shape in `google_e2e.rs`:

```rust
let h = E2eHarness::start(Seed::TwoAccounts).await?;
let output = h.cli(&["--json", "system", "status"]).await?;
assert_eq!(output.status, 0);
assert!(output.json()?["result"]["profile_id"].is_string());
h.shutdown().await?;
let offline = h.cli(&["--json", "system", "status"]).await?;
assert_eq!(offline.status, 4);
```

`shutdown` stops the daemon while leaving fixture paths available until harness drop; final drop cleans remaining children. Every later E2E uses this subprocess boundary.

## Task 04 — Google OAuth and account lifecycle

**Depends on:** 03. **Requirements:** R02, R15.

**Files:** Engine `accounts.rs`, Google `{http,oauth}.rs`, Accounts proto/handler/CLI, `tests/{google_system,google_e2e}.rs`.

**Interfaces:** `Engine::begin_google_auth(GoogleAuthRequest) -> AuthSession`; `auth_status(AuthSessionId)`; `list_accounts()`; `disconnect_account(AccountId)`. Session carries a browser URL and expiry, not verifier/token. `GoogleHttp` owns token refresh coordination, deadlines, redacted errors, and validated production endpoints.

- [x] Test full mock authorization redirect/code exchange/refresh with two accounts, denial, state mismatch, wrong PKCE, expired/reused code, and revoked refresh token.
- [x] Implement installed-app browser OAuth with loopback callback, random state and PKCE. CLI passes client registration configuration securely; scope requests cover the shipped Gmail/Calendar capabilities. Explain Google consent/test-user setup and any testing-mode token lifetime in `RUNNING.md`, verified against current official docs.
- [x] Persist account plus credential references with rollback/compensation on partial failure. Serialize refresh per account and preserve rotated refresh tokens. Never put token values into process arguments, logs, JSON results, database fields, or error URLs.
- [x] Implement disconnect as a durable pause plus credential removal, retaining cached data, drafts, and operation records. It neither purges data nor revokes the user's wider Google application grant silently. Reconnect must verify the provider account matches before resuming.
- [x] Add CLI `connect-google`, account list/disconnect, and inspectable authorization status. Harness follows mock browser redirects; production opens the actual consent page. Test one account requiring reauthorization while another continues.
- [x] Run `--suite google_system` and `--suite google_e2e`; prove offline test execution without a production Google client ID.

## Task 05 — Gmail initial sync, MIME, attachments, and query vertical

**Depends on:** 04. **Requirements:** R03, R12, R15.

**Files:** Engine Google `gmail.rs`, `domain/mail.rs`, `store/mail.rs`, `mail.rs`; Mail proto/handler/CLI; fixture MIME; `tests/{google_system,google_e2e}.rs`.

**Interfaces:** `Engine::sync_account(SyncRequest) -> SyncRunId`; mail list/get/search/collection queries return revision and coverage; `fetch_message(MessageRef)` explicitly downloads missing payload; raw/attachment downloads expose bounded streams. `MessageRef` includes account ID and local message ID. Initial sync stages provider IDs and pages under a sync-run generation.

- [x] Seed multiple pages, a message in several labels, shared thread membership, missing optional fields, HTML-only content, an exact multipart attachment, and duplicate provider IDs across accounts. Test complete ingestion through HTTP, then disable the provider and read/search/download locally.
- [x] Implement label/profile/message enumeration. Capture a start history cursor before initial listing; walk every page, ingest raw MIME/metadata, then catch up history from that start before marking the generation current. If the cursor expires, discard/restart that incomplete generation safely.
- [x] Preserve original MIME bytes, headers, attachment bytes, and raw provider IDs. Decode base64url and millisecond timestamp strings without floating-point conversion. Retain explicit body availability and enforce payload limits during streaming.
- [x] Create encrypted FTS indexes from normalized content, with account-scoped SQL parameters and bounded search expressions. Sanitize terminal control sequences for displayed text; raw downloads preserve original bytes.
- [x] Implement collection/message listing, read/search, explicit fetch, and streaming downloads through API and CLI. Use revision-bound keyset pagination. Return missing/too-large content honestly.
- [x] Run both Google suites. Assert SHA-256 of downloaded MIME/PDF equals hand-authored fixture hashes, and that label membership does not duplicate the message entity.

Test fragment, with fixture IDs defined by `Seed::TwoAccounts` and `connect_and_sync_google` performing OAuth and CLI sync:

```rust
let h = E2eHarness::start(Seed::TwoAccounts).await?;
let account = h.connect_and_sync_google("account-a").await?;
let output = h.cli(&["--json", "mail", "search", "--account", &account,
    "--query", "synthetic-needle"]).await?;
assert_eq!(output.status, 0);
assert_eq!(output.json()?["result"]["items"].as_array().unwrap().len(), 1);
```

The harness helper returns the actual local account ID obtained from API output; it cannot preseed storage or fake sync results. Tests may use `unwrap` for assertions under test-only lint allowances.

## Task 06 — Gmail incremental sync and crash-safe reconciliation

**Depends on:** 05. **Requirements:** R03, R08, R14.

**Files:** Google `gmail.rs`, `store/mail.rs`, sync tables/migrations, `tests/google_system.rs`, `tests/google_e2e.rs`.

**Interfaces:** Sync scopes store typed cursor, query fingerprint, run status, and projection generation. Applying a page is idempotent; promoting a complete generation and final cursor is transactional. Engine startup resumes or safely restarts incomplete sync runs.

- [x] Test page overlap, deletion between list/get, last-page failure, label-only history, history expiry/404, and out-of-order IDs. Assert that a partial listing never deletes unseen local messages.
- [x] Implement history handling for additions, deletions, and label changes. Commit the final response's `historyId` only after every page is durably applied; do not use `nextPageToken` as a checkpoint.
- [x] Implement expired-history full reconciliation using staging. Delete only absent provider projection records after a completed authoritative run. Preserve drafts and operations even when their source message disappears.
- [x] Add test barriers before page commit, before generation promotion, and after promotion. Force-kill the real daemon at each barrier; restart and assert stable identities, complete membership, and correct final cursor behavior through mock request observations.
- [x] Run both Google suites with server page cap 1 and then larger caps. Assert the same final data and no lost changes after sync retries.

The transaction contract is:

```sql
BEGIN IMMEDIATE;
-- Apply the verified complete generation and its deletions here.
UPDATE sync_scopes SET cursor = ?1, active_generation = ?2
WHERE account_id = ?3 AND scope_id = ?4;
-- Insert a change-log revision for the same committed projection here.
COMMIT;
```

The comments mark transaction ordering, not permission to omit projection/change-log writes. Task 06 implements those writes against the schema introduced in Tasks 01/05.

## Task 07 — Google Calendar canonical sync and agenda

**Depends on:** 06. **Requirements:** R04, R12, R14.

**Files:** Google `calendar.rs`, `domain/calendar.rs`, `store/calendar.rs`, `calendar.rs`; Calendar proto/handler/CLI; fixed occurrence fixtures; both Google suites.

**Interfaces:** Calendar references carry account/calendar/event IDs. `EventTime` is `Date { date }` or `DateTime { rfc3339, time_zone }`. Engine calendar list/get/agenda operations return canonical version and occurrence coverage. `refresh_agenda(AgendaWindow)` is explicit and inspectable.

- [x] Test two accounts with the same event ID; all-day exclusive ends; DST; recurring masters; moved/cancelled exceptions; multiple calendars; deleted calendars; and token expiry/410. Expected occurrences are hand-authored, not generated by the production parser.
- [x] Implement paginated calendar discovery and canonical event sync with `singleEvents=false`, `showDeleted=true`, and a stable query fingerprint. Persist only the final `nextSyncToken`; never attach time bounds to incremental token requests.
- [x] Store raw event fields, ETag, recurrence identity, original start, attendee data, and tombstones. Stage full resets so a failed page cannot delete the old complete projection. Reconcile removed calendars only after complete discovery.
- [x] Implement a separate provider-expanded agenda cache for the spec's rolling window. Invalidate affected coverage after canonical changes; refresh expands recurring instances using Google, preserving cancellations and exceptions. No second local recurrence engine in this release.
- [x] Implement calendar list/get/agenda and explicit refresh in RPC/CLI. Validate date ranges and expose unavailable/stale/out-of-window coverage.
- [x] Run both Google suites. Verify fixture `all-day-2` has start `2026-10-31`, exclusive end `2026-11-02`, and occupies exactly October 31 and November 1 in the agenda.

Required public time representation:

```proto
message EventTime {
  oneof value {
    string date = 1;
    ZonedDateTime date_time = 2;
  }
}
message ZonedDateTime {
  string rfc3339 = 1;
  string time_zone = 2;
}
```

## Task 08 — Background operation and recoverable change stream

**Depends on:** 07. **Requirements:** R10, R12.

**Files:** Engine `scheduler.rs`, `changes.rs`, store change log; status/watch proto and CLI; Google system/E2E suites.

**Interfaces:** `Engine::watch_changes(after_revision)` returns committed revision events or a resnapshot-required error. Scheduler owns one account conflict lane, separate mail/calendar schedules, bounded global concurrency, cancellation tokens, and per-provider retry deadlines. Status identifies phase, last success, age, next attempt, error class, and incomplete coverage.

- [x] Test polling actually changes the local mailbox and calendar after a mock edit without a manual sync command. Test canceled work, one broken account, rate limit guidance, disconnect/reconnect, and simulated wake/catch-up.
- [x] Implement the concrete defaults from the spec and monotonic deadline scheduling. Coalesce repeated requests for the same scope; permit other accounts while one backs off. Credential revocation pauses that account instead of retrying forever.
- [x] Persist local changes and operation transitions with revisions in the same transaction. Stream from the bounded change log and signal overflow/expired revision explicitly. A slow consumer never silently misses updates.
- [x] Implement watch, status, sync wait, and consistent CLI exit mapping. A queue receipt is distinguishable from successful completion. Keep engine errors structured and safe for CLI diagnostics.
- [x] Run both Google suites. Pass milestone B only after reboot/offline read and background mail/calendar refresh work from actual CLI processes.

Required JSON failure shape, with no successful process exit:

```json
{"schema_version":1,"error":{"code":"unavailable","message":"Daemon is unavailable","retryable":true}}
```

Reader milestone B verified offline in `rebuild/test-results/task08`; operation enqueue/start/finish and audited resolution/resend revisions are additionally verified by operation_system and google_e2e (17 tests at the send crash-boundary checkpoint).

## Task 09 — Durable drafts and operation journal

**Depends on:** 08. **Requirements:** R05, R08, R12.

**Files:** Domain/store/application operations modules; draft methods in mail modules; Operations proto/handler/CLI; `tests/{operation_system,google_e2e}.rs`.

**Interfaces:** `Engine::save_draft(DraftInput) -> Draft`; draft list/get/delete; reply/forward preparation; `enqueue(RequestIdentity, OperationPayload) -> Operation`; get/list/cancel/resolve. `Operation` contains ID, account, request ID, state, timestamps, attempt receipts, preconditions, and redacted error. Typed payloads cover send, mail change, and calendar change.

- [x] Test draft persistence across forced termination and provider resync, multi-recipient reply-all with self-address exclusion, forwarding original attachments, and invalid recipient/header injection rejection. Upload attachments through chunked RPC, not daemon file paths.
- [x] Implement versioned local drafts with optimistic version checks. Compose replies with correct headers/thread context; preserve original attachments and MIME semantics. Draft editing cannot mutate an already enqueued send payload.
- [ ] Implement atomic desired-local-state plus operation intent, unique `(account_id, request_id)`, and canonical payload fingerprint. Same request returns the original operation; a changed payload under the same request ID fails.
- [ ] Implement the full state machine and transition constraints. Only queued/not-dispatched work is safely cancellable. Running work after restart enters reconciliation; classification never blindly resends it. Expose conflict/uncertain resolution with an explicit decision and audit trail.
- [ ] Implement draft and operation RPC/CLI methods. Resolution can acknowledge positively established success, abandon unresolved work, or explicitly create a new-send request with duplication risk shown; it cannot relabel unknown delivery as safely failed.
- [ ] Run `--suite operation_system` and `--suite google_e2e`. Assert queue and draft content survive backup-independent restart and full projection reset.

Required atomic uniqueness constraint:

```sql
CREATE UNIQUE INDEX operations_request_identity
ON operations(account_id, request_id);
```

On conflict, load and compare the canonical payload fingerprint inside the transaction. Never implement deduplication as a non-atomic “lookup, then insert.”

## Task 10 — Gmail send and mail mutations

**Depends on:** 09. **Requirements:** R05–R06, R08, R14.

**Files:** Google `gmail.rs`, engine `mail.rs`/`operations.rs`, Mail proto/CLI, both Google suites, operation suite.

**Interfaces:** `Engine::send_draft(SendDraftRequest) -> Operation`; `change_message(ChangeMessageRequest) -> Operation`. Provider executor accepts frozen MIME/desired labels and returns a receipt, definitive failure, conflict, or uncertainty. Mail action capability metadata distinguishes labels from folder placements.

- [x] Test compose/reply/reply-all/forward with attachments through the actual CLI and inspect MIME received by mock `messages.send`. Verify explicit From account, envelope recipients, thread ID and reply headers, and one accepted send per successful operation.
- [x] Implement send from frozen original bytes with stable Message-ID and request identity. Reconcile positively using provider receipts/search evidence when available; do not infer non-delivery from absence or claim a custom header creates Gmail idempotency.
- [x] Implement read/unread, star/unstar, archive, trash/untrash, and add/remove existing labels as typed actions. Preserve unrelated labels and classify repeatable desired-state actions separately from send.
- [x] Inject response loss after acceptance, kill the daemon, restart, and issue the same request ID again. Assert accepted-send count stays 1, while state is applied only with positive evidence or otherwise uncertain. A new explicit resend request is a separate operation.
- [x] Exercise definitive pre-acceptance failure, 429/5xx retry rules, draft edits after enqueue, cancel races, and concurrent duplicate requests. Run all three affected suites.

Required independent side-effect assertion:

```rust
assert_eq!(h.google.control().accepted_sends("account-a").await?, 1);
```

This must accompany CLI/journal assertions after every send crash/retry scenario; checking only that the local operation says “sent” is insufficient.

## Task 11 — Calendar writes, conflicts, responses, and free/busy

**Depends on:** 10. **Requirements:** R07–R08, R14.

**Files:** Google/calendar application/operation modules, Calendar proto/CLI, both Google suites.

**Interfaces:** `Engine::change_event(CalendarChangeRequest) -> Operation`; `query_free_busy(FreeBusyRequest) -> FreeBusyResult`. Requests specify action, identity, expected ETag where applicable, occurrence/series scope, field patch, and notification policy.

- [x] Test create/update/delete, attendee response, all-day edit, one moved occurrence, whole-series edit, and free/busy. Assert notification counts independently of final event objects.
- [x] Implement provider-compatible stable event IDs for creates and reconciliation. Updates/deletes use `If-Match`; 412 records a conflict and preserves local intent. Patch named fields so unknown/provider-managed data is not silently discarded.
- [x] Require explicit occurrence versus whole-series scope and notification policy. Reject unsupported “this and following.” Validate organizer/attendee permissions; RSVP changes only the selected user's supported response fields.
- [x] Handle apply-then-drop for create/update/delete without duplicate events or repeated notifications. Positively reconcile versions/content before retrying; unresolved notification outcome remains visible.
- [x] Implement JSON action-file validation, free/busy result coverage/errors, and CLI examples. Run both Google suites and finish milestone C only with all writes reachable through the CLI.

Concrete conflict acceptance: mock event ETag changes from `v1` to `v2` between read and edit; a request with `If-Match: v1` receives 412, leaves remote `v2` unchanged, and produces CLI exit 5 with an inspectable conflict operation.

Offline verification: `rebuild/test-results/task11/results.json`; all12commands exit0, normal/feature142 each (overlapping), separate mock25/system21/E2E26/operation6/release1. Live compatibility remains deferred and unverified.

## Task 12 — Synology MailPlus through IMAP/SMTP

**Depends on:** 11. **Requirements:** R02, R05–R06, R08–R09, R14–R15.

**Files:** Engine IMAP/SMTP adapters and account configuration, placement storage, Accounts/Mail proto/CLI capability handling, test-support `imap.rs`, `tests/{imap_contract,imap_system,imap_e2e}.rs`, `tests/imap/compose.yaml`.

**Interfaces:** `Engine::add_imap_account(ImapAccountConfig)` accepts host/port/TLS/account/Sent-folder configuration with credential entry separate from config. A placement reference is `(account, mailbox identity, UIDVALIDITY, UID)`. SMTP result and Sent-copy status are separate operation substeps.

- [x] Add a pinned local Dovecot instance and SMTP capture server, such as Mailpit, in the test composition. Bind mapped ports to loopback; use synthetic users and a local test CA. Use official maintained images and pin tested versions/digests at implementation time. Downloads are setup; tests contact local services only. References: [Dovecot Docker setup](https://doc.dovecot.org/main/installation/docker.html), [Mailpit Docker setup](https://mailpit.axllent.org/docs/install/docker/).
- [x] Write strict command/response tests for mailbox quoting/encoding, quiet delta, UIDVALIDITY reset, flags, deletion, truncated FETCH, and capability negotiation. Independently exercise the adapter against Dovecot, including servers without optional extensions.
- [x] Implement full/delta mail ingestion through the same storage/query contract. Discover folders and special-use roles. Do not assume message IDs or UIDs survive a UIDVALIDITY change; stage resync without deleting durable local state.
- [x] Implement read/star/archive/trash/restore and folder move/copy using explicit placement. Use safe UID-scoped expunge where available; if fallback cannot avoid unrelated deletion, return unsupported. Keep prior placement metadata for restore and report missing destination conflicts.
- [x] Implement SMTP sending and configurable server-auto-Sent versus client-append behavior. Accepted SMTP delivery is never retried because APPEND failed. Unknown send/COPY/APPEND outcomes enter reconciliation/uncertainty, with remote counters proving no blind repeat.
- [x] Implement validated production TLS, hostname checks, secure credential entry, and account capabilities in API/CLI. Never allow a remote plaintext-auth fallback. No SMTP/IMAP password in a config file or argv.
- [x] Run `--suite imap_contract`, `--suite imap_system`, and `--suite imap_e2e`, including actual CLI sends and downloaded-byte equality. Keep Google suites green. Record exact supported extensions; Dovecot passing does not claim live MailPlus compatibility.

Offline SMTP/Sent checkpoint: `rebuild/test-results/task12-smtp-server/results.json` all12commands0; independentIMAPcontract2(wrapping18Python),engine78,protoCLI4,IMAPsystem17,actualIMAPE2E6,Google21/26, nofailed/ignored. BothSentpolicies andTLS modes, negativeevidence/crash cases verified. Explicitresend follow-up status is in rebuild/docs/SESSION-STATE.md. ManualSMTPconfirmation and remaining profile/legacy/security work are still tracked; no live compatibility claim.

The empty-delta regression assertion is a protocol observation: when no new UID exists, the adapter issues no empty FETCH command, leaves the current checkpoint unchanged, and reports a successful quiet sync.

Latest offline Task12 evidence: rebuild/test-results/task12-smtp-confirmation (engine80/protoCLI8/IMAP19/Google6+21+26, all9commands0) and task12-smtp-profiles (IMAPsystem21/actualCLI9, fmt/bothClippy, all5commands0). SMTP manual confirmation/abandonment and missing-UIDPLUS/queued-endpoint profiles now verified. Full goal still requires Task13 recovery/migration fixtures, Task14 cross-cutting audit, Task15 artifacts/CI and Task16 acceptance; no live MailPlus compatibility claim.

## Task 13 — Export, backup, restore, migrations, and repair

**Depends on:** 12. **Requirements:** R11, R15.

**Files:** Store `backup.rs`/migration runner, Maintenance proto/handler/CLI, `tests/recovery_e2e.rs`, `docs/RECOVERY.md`.

**Interfaces:** Backup produces a consistent encrypted snapshot including drafts, operations/attempts, configuration, and projection state. Inspect validates format/version/integrity. Restore accepts a streamed backup into a new profile only. `repair_projection(account, scope)` stages a fresh provider projection, never discards durable state.

- [ ] Test raw EML and attachments byte-for-byte; reject output paths aliasing live database/WAL/config by canonical path and existing-file identity. Use atomic temporary-file completion, no success on partial writes, and no path traversal through supplied filenames.
- [ ] Implement SQLCipher-consistent backup from a defined database snapshot, including WAL-visible committed state. Encrypt the backup under a separate recovery passphrase using SQLCipher's supported key derivation/export facilities; read passphrase through hidden terminal input or protected input channel, never argv/config/logs. Preserve schema-version metadata explicitly across export and verify a real read after keying, since setting a key alone does not validate it. Do not build custom cryptography. Reference: [SQLCipher keying and export API](https://www.zetetic.net/sqlcipher/sqlcipher-api/).
- [ ] Exclude provider tokens and the daemon bearer token from backups. Document that the recovery passphrase is required and has no reset path. On restore, generate fresh profile/API keys and require provider reconnection; pending outbound work remains held for reconciliation, never auto-replayed.
- [ ] Implement inspection and restore into a new directory/profile with checksums, SQLCipher integrity verification, disk-space/error handling, atomic activation, and rollback. Wrong key, truncated backup, newer schema, or failed migration leaves source and target originals intact.
- [ ] Add migration fixtures from every supported rebuild schema. Test interrupted migrations, corruption of a copied database, failed WAL/backup operations, and repair while drafts/queued operations exist. Repair refuses to wipe an unreadable store; retain originals and provide a recovery error.
- [ ] Implement `backup create/inspect/restore` and `repair` CLI/RPC, plus dry-run reporting of affected projections. Run `--suite recovery_e2e` and retain restore comparison evidence for all durable tables and payload hashes.

Required recovery case: queue a send without dispatch, save a draft with a PDF, create backup, restore to a new profile, reconnect against the mock, and verify the draft/PDF/request identity survive while accepted-send count remains zero until explicit resume/reconciliation permits dispatch.

## Task 14 — Adversarial acceptance and resource bounds

**Depends on:** 13. **Requirements:** R01–R15.

**Files:** `tests/{multi_engine_system,security_system,resource_system}.rs`; relevant implementation fixes; `docs/VERIFICATION.md`.

**Interfaces:** Reuse real system/process harnesses and independent provider observations. Resource instrumentation reports queue depths, active requests, fetched bytes, storage page batches, and child RSS without content/secret leakage.

- [ ] Run three independent engines/profiles against one mock account. Apply external message/calendar changes while engines restart at different points. Require all provider projections to converge and assert separate send/copy/notification counters. Local drafts intentionally remain profile-local.
- [ ] Test all services without auth and with wrong/expired tokens, account crossover, revoked credentials, malicious MIME/HTML/terminal escapes, filename traversal, header injection, malformed provider IDs, and oversized streams.
- [ ] Search database, WAL, backup, temporary files, stdout/stderr, and retained logs for synthetic content/secret canaries. Also prove wrong-key/ordinary-SQLite rejection; byte scanning alone is not an encryption proof.
- [ ] Exercise 10,000 metadata messages across multiple pages and 16 MiB attachments; test rejection above the configured 64 MiB limit. Enforce request concurrency 2 and bounded queues. Report latency/RSS on the actual machine; fail unbounded growth, not an invented universal speed target.
- [ ] Run deterministic disconnect/retry/cancel/kill scenarios at every persisted-operation boundary. Classify findings by risk; fix all defects that violate an acceptance requirement, then rerun the affected suites. Do not weaken mocks or discard assertions to pass.
- [ ] Run all three new suites and `python3 rebuild/scripts/verify.py --all`. Record newly observed counts and zero ignored tests.

## Task 15 — Local release candidate, contract, and CI

**Depends on:** 14. **Requirements:** R12, R15–R16.

**Files:** `scripts/package.py`, contract descriptor/golden and tests, `docs/{API,RUNNING,TESTING}.md`, `README.md` within rebuild; root `.github/workflows/rebuild-ci.yml` as the only planned root build-integration addition.

**Interfaces:** `python3 rebuild/scripts/package.py --output rebuild/dist` builds production-only binaries into a clean target directory, verifies them, and emits versioned local archives with SHA-256, contract descriptor, license notices, and build metadata. No publishing or automatic installation.

- [ ] Pin the dependency lockfile and required build tools; document platform-specific SQLCipher prerequisites. Review dependency advisories/licenses and secret scan results. Address findings or explicitly record an accepted limitation; no fabricated clean security scan when a tool is unavailable.
- [ ] Freeze `nuncio.v2` field numbers for this release and add descriptor compatibility checks. Compile a tiny external generated-client status/watch smoke test with no engine dependency. This proves the native-client boundary without starting a native app project.
- [ ] Add CI jobs named `rebuild-lint`, `rebuild-mock-contract`, `rebuild-system`, `rebuild-e2e`, `rebuild-imap`, and `rebuild-release-check`, using the same runner on Linux and the platform-relevant macOS checks. Pre-pull local server images, then prevent external provider access. Write workflow files; do not change remote branch protection without authorization.
- [ ] Ensure CI and package checks reject test flags and exclude mock binary/test dependencies from production artifacts. Validate every required job fails when its underlying suite fails. Versioned artifacts are local release candidates, not a public functional-release claim.
- [ ] Write first-run/OAuth/credential setup, offline usage, action-file examples, JSON/exit semantics, backup recovery, install/uninstall instructions, and compatibility limits. Distinguish normal setup from manual live acceptance.
- [ ] Run `python3 rebuild/scripts/verify.py --all`, then the package command. Exercise extracted binaries in a temporary location. Run root fmt/Clippy/tests if root build behavior changed. Record artifact paths/hashes and local-equivalent CI results; remote CI remains unobserved until authorized push.

## Task 16 — Provider acceptance and final completion report

**Depends on:** 15; named disposable accounts and action authorization for live acceptance. **Requirements:** R01–R16.

**Files:** `docs/VERIFICATION.md`, `docs/SESSION-STATE.md`, `docs/TODO.md`, `docs/RUNNING.md` within rebuild.

- [ ] Ask early in execution for the prerequisites that only James can provide: Google installed-app registration and named disposable test identity, Synology hostname/TLS/account plus MailPlus version, and permitted test recipients/calendars/actions. Do not ask for plaintext credentials in chat. Continue independent offline implementation while access is pending.
- [ ] Prepare a concrete manual acceptance worksheet with exact commands, named resources, expected changes, cleanup steps, and evidence fields. Obtain explicit authorization before any live read/write/send/invite. These manual checks are separate from automated tests.
- [ ] On Google, verify actual OAuth/refresh, multi-page sync, external change reconciliation, raw/attachment fidelity, one authorized send/reply, label changes, and isolated calendar create/edit/respond/delete with named notification policy.
- [ ] On Synology, verify certificate/hostname, negotiated IMAP capabilities, folder encoding, UID/flag/deletion sync, one authorized SMTP submission, and actual Sent-copy behavior. Record server/package versions and any compatibility restrictions. Fix discovered product defects and rerun offline regressions before repeating the relevant manual check.
- [ ] Complete the requirement matrix below with source locations, actual test names and run artifacts, manual evidence, limitations, and package hashes. Mark each requirement passed, failed, or externally pending; do not convert missing evidence to passed.
- [ ] Produce final implementation status. The entire goal is complete only when required software, offline gates, local artifacts, and authorized provider acceptance pass. If live access remains pending, report “implemented and offline-verified; live acceptance pending” and preserve that outstanding goal condition. Follow the goal tool's actual blocked-state policy rather than marking success to end a turn.

## Requirement-to-task traceability

| Requirement | Implemented in tasks | Required evidence |
|---|---|---|
| R01 | 01, 03, 14 | lifecycle, actual daemon/CLI, lock/restart/auth tests |
| R02 | 04, 12, 16 | OAuth and account isolation, keyring/TLS, manual providers |
| R03 | 05–06, 16 | Gmail wire/system/E2E, MIME hashes, cursor/crash tests |
| R04 | 07, 16 | canonical/occurrence fixtures, cross-account/deletion/token tests |
| R05 | 09–10, 12 | draft durability, received MIME, attachment and send counters |
| R06 | 10, 12 | Gmail label and IMAP placement/move/copy observations |
| R07 | 11, 16 | event/RSVP/free-busy tests, versions and notification counts |
| R08 | 06, 09–12, 14 | journal state machine, repeated IDs, crash/uncertainty tests |
| R09 | 12, 16 | strict IMAP/SMTP plus independent server and live MailPlus |
| R10 | 08, 14 | actual automatic refresh, cancellation, backoff/isolation |
| R11 | 13 | byte fidelity, restored durable state, failed-recovery originals |
| R12 | 01, 03, 05, 07–13, 15 | CLI/RPC matrix, exits, streams, descriptor/client smoke |
| R13 | 02–03 | independently sourced mock conformance and fault-effect tests |
| R14 | 03–14 | required system and subprocess suites with no live network |
| R15 | 01, 03–05, 12–15 | encryption/secret/auth/TLS/limits/release-isolation evidence |
| R16 | 15–16 | reproducible local artifacts, CI definitions, manuals, report |

## Scope changes and completion discipline

The executor may choose private helper names and compatible library versions while preserving the defined interfaces and behavior. Record justified implementation refinements in session state. Changes to providers, product scope, encryption guarantees, send semantics, native-app scope, or completion requirements need James's decision. Missing credentials, a difficult test, or time spent is not permission to remove a requirement.

This plan deliberately has no fixed delivery-date promise. The highest-uncertainty work is synchronization under change, uncertain remote writes, calendar exceptions, and MailPlus compatibility. Resolve those risks with the specified tests and manual evidence. Do not claim success by completing only the scaffold, mock, read-only milestone, or an impressive test count.
