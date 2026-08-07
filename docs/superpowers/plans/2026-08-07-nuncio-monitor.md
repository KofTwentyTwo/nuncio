# Nuncio Monitor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Windows system-tray dev instrument (`crates/nuncio-monitor`) that starts and stops `nunciod`, follows its logs live with structured filtering, and shows configured accounts with per-account sync state and outbox depth — plus the three engine changes it needs.

**Architecture:** The monitor is a thin client reading from three separate sources: gRPC on `127.0.0.1:9420` for facts, the daemon's rotating JSON log file for the log viewer, and the `<db_path>.lock` single-instance lock for liveness. It never touches SQLite. Engine-side work adds a `System.Shutdown` RPC, an `account_id` column on `pending_remote_mutations`, and `System.GetHealth` (issue #296).

**Tech Stack:** Rust 1.97.1 (pinned), `eframe`/`egui` for the window, `tray-icon` for the tray, `tonic`/`prost` via the existing `nuncio-proto` client helpers, `tokio`, `serde_json`, `sqlx` (store only).

Design: [`docs/superpowers/specs/2026-08-07-nuncio-monitor-design.md`](../specs/2026-08-07-nuncio-monitor-design.md)

## Global Constraints

- Toolchain is pinned to **1.97.1** by `rust-toolchain.toml`. Do not bump it.
- Workspace lints are **deny**: `warnings`, `unsafe_code`, `unused_must_use`, and clippy's `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable`. This applies to the GUI crate too. In tests these are allowed; in production code they are compile errors.
- The gate is three commands, run in order: `cargo fmt --all -- --check`, `cargo check-all`, `cargo test-all`. The pre-commit hook runs exactly this.
- **No live network in tests.** Mock with `wiremock` or the `Mock*Backend` traits; mock the keyring with `SecretManager::mock()`.
- **No fabricated success.** A path that cannot do the real thing returns an honest error, never canned data. In the monitor UI this means unknown must render differently from zero.
- **The monitor adds no redaction of its own.** Secrets are kept out of logs at the source by `Redacted<T>` and `no_secrets_canary_test` (see `docs/LOGGING.md`). A second redaction layer in the viewer would mask exactly the canary regressions worth catching, so a secret appearing in the monitor must look like the engine bug it is. Do not add filtering or masking to `log_tail.rs` or `ui.rs`.
- Conventional Commits, imperative, subject under 72 chars, **no AI attribution**.
- Do not put issue numbers, story ids, or `Phase N` breadcrumbs in `.rs` or `.proto` comments. That belongs in git history and `docs/`.
- **Only one proto-touching task may be in flight at a time** (`docs/HANDOFF.md`). Tasks 2 and 5 both touch `nuncio.v1`; do not start Task 5 until Task 2 is merged.
- Any change to `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto` requires regenerating the committed descriptor golden `descriptor.bin`, or the contract-stability test fails.
- Kill any running `nunciod.exe` before building on Windows: a live daemon holds `target\debug\nunciod.exe` and the link step fails with `Access is denied. (os error 5)`.

## File Structure

**Phase 1 — store**
- Modify: `crates/nuncio-store/src/db.rs` — add `ensure_pending_mutations_account_column()`, call it from `migrate()`, add `count_pending_mutations_by_account()`.

**Phase 2 — Shutdown RPC**
- Modify: `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto` — `rpc Shutdown` on `System`, plus request/response messages.
- Modify: `crates/nuncio-proto/proto/nuncio/v1/descriptor.bin` — regenerated golden.
- Modify: `crates/nuncio-proto/src/client.rs` — nothing new; `connect_system` already returns the client that gains the method.
- Modify: `crates/nunciod/src/grpc.rs` — `shutdown` field on `SystemGrpcService`, `shutdown()` handler.
- Modify: `crates/nunciod/src/main.rs` — share the `ShutdownController` with the gRPC server.
- Modify: `crates/nuncio-cli/src/args.rs`, `crates/nuncio-cli/src/runner.rs` — `system shutdown` command.
- Create: `crates/nunciod/tests/shutdown_e2e_test.rs`.

**Phase 3 — GetHealth**
- Modify: `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto` — `rpc GetHealth`, `GetHealthResponse`, `AccountQueueDepth`.
- Modify: `crates/nunciod/src/grpc.rs` — `get_health()` handler.
- Modify: `crates/nuncio-cli/src/args.rs`, `runner.rs` — `system health` command.
- Create: `crates/nunciod/tests/health_e2e_test.rs`.

**Phase 4 — monitor**
- Create: `crates/nuncio-monitor/Cargo.toml`, `src/main.rs`, `src/log_tail.rs`, `src/engine.rs`, `src/status.rs`, `src/state.rs`, `src/ui.rs`, `src/tray.rs`.
- Modify: root `Cargo.toml` — add the workspace member and the three new workspace dependencies.

One file per responsibility. `state.rs` holds `AppState`, the only thing `ui.rs` reads, which is what keeps the logic testable without a GUI harness.

---

## Phase 0 — Make the gate trustworthy

Added after the pre-flight scan. Every later task ends with "run the gate, expect green", which is meaningless while `dev` fails CI intermittently. Two of the three known-flaky tests live in the exact files this plan edits.

### Task 0: Convert racy log-assertion tests to `tracing-test`

**Root cause.** `tracing` caches callsite `Interest` globally, resolved once per callsite. A test installing a capturing subscriber with `tracing::subscriber::set_default` only observes events whose callsite was first evaluated while that subscriber was active. Under `cargo test`'s default parallelism, whichever test reaches a callsite first wins the cache, so these tests pass or fail on thread scheduling. They pass locally and fail on CI because core count and ordering differ.

**Scope.** Only the two tests that actually fail CI *and* sit in files this plan modifies. Do not refactor the three `test_tracing.rs` helper modules or the other 28 hand-rolled captures; that is issue #375's full scope and is not needed here.

The other two known-flaky tests have **different root causes** and are explicitly out of scope: `a_timed_out_item_does_not_block_a_later_item_in_a_subsequent_pass` (timing-sensitive, `crates/nunciod/tests/outbox_executor_e2e_test.rs`) and `mail_and_folder_report_honest_errors_when_daemon_unreachable` (port-reuse race, issue #347).

**Files:**
- Modify: `crates/nuncio-store/Cargo.toml` (add `tracing-test = "0.2"` to `[dev-dependencies]`)
- Modify: `crates/nuncio-store/src/db.rs:3885` (`migrate_logs_when_an_additive_column_migration_actually_runs`)
- Modify: `crates/nunciod/Cargo.toml` (add `tracing-test = "0.2"` to `[dev-dependencies]`)
- Modify: `crates/nunciod/src/grpc.rs` (`create_rule_handler_logs_domain_event_under_request_scope`)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing. Behaviour-preserving test-infrastructure change.

**Reference pattern — already in this repo.** `crates/nuncio-mail/src/imap.rs:1442` and `crates/nuncio-mail/src/smtp.rs:354` already use `tracing_test::traced_test`. Match that usage exactly rather than inventing a variant.

- [ ] **Step 1: Confirm the current failure mode**

Run: `cargo test -p nuncio-store migrate_logs_when_an_additive_column_migration_actually_runs -- --test-threads=1`
Then: `cargo test -p nuncio-store -- --test-threads=8`
Expected: passes single-threaded, and is the test most likely to fail under parallelism. It may pass both times; that is the nature of the race and is not evidence the problem is absent.

- [ ] **Step 2: Convert the store test**

Add `tracing-test = "0.2"` to `[dev-dependencies]` in `crates/nuncio-store/Cargo.toml`. Replace the hand-rolled `CapturedLogs` writer, its `std::io::Write` impl, its `MakeWriter` impl, and the `set_default` guard with the attribute macro:

```rust
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn migrate_logs_when_an_additive_column_migration_actually_runs() {
        // ... existing arrange/act body, minus all subscriber setup ...

        assert!(logs_contain("applying schema migration"));
        assert!(logs_contain("pending_remote_mutations"));
    }
```

Delete the now-unused `use std::sync::{Arc, Mutex};` and `use tracing_subscriber::fmt::MakeWriter;` from inside the test. `logs_contain` is injected into scope by the macro; do not import it.

- [ ] **Step 3: Run the store test**

Run: `cargo test -p nuncio-store migrate_logs`
Expected: PASS.

- [ ] **Step 4: Convert the daemon test the same way**

Add `tracing-test = "0.2"` to `[dev-dependencies]` in `crates/nunciod/Cargo.toml`. Apply the identical treatment to `create_rule_handler_logs_domain_event_under_request_scope` in `crates/nunciod/src/grpc.rs`, replacing its hand-rolled capture with `#[tracing_test::traced_test]` and `logs_contain(...)` assertions that check the same strings the original asserted. Do not weaken an assertion to make it pass: if the original checked a span field, check the same field.

- [ ] **Step 5: Run the full gate repeatedly**

```bash
cargo fmt --all -- --check && cargo check-all
cargo test-all
cargo test-all
cargo test-all
```

Expected: green all three runs. Three consecutive passes is weak evidence against a race but it is the evidence available locally; CI is the real check.

- [ ] **Step 6: Commit**

```bash
git add crates/nuncio-store/Cargo.toml crates/nuncio-store/src/db.rs crates/nunciod/Cargo.toml crates/nunciod/src/grpc.rs
git commit -m "test: use tracing-test for racy log assertions"
```

---

## Phase 1 — Store: per-account outbox

### Task 1: Add `account_id` to `pending_remote_mutations` with backfill

`pending_remote_mutations` today is `id, rule_id, message_id, mutation_type, payload, status, retry_count, created_at` (`crates/nuncio-store/src/db.rs:575`). There is no account column, so per-account outbox counts are not derivable. `messages.account_id` exists (`crates/nuncio-store/src/db.rs:505`), so the backfill joins through `message_id`.

**The migration alone is not sufficient.** Backfilling existing rows fixes history; new mutations would still write `NULL` because the write path has no account in it. All four of these must change together or per-account counts work once and then silently stop:

- `PendingRemoteMutation` (`crates/nuncio-filter/src/ast.rs:322`) has 8 fields and no `account_id`.
- `OutboxManager::create_mutation` (`crates/nuncio-filter/src/outbox.rs:35`, re-exported at the crate root by `pub use outbox::*`) takes `(rule_id, message_id, action_type, target)` — no account.
- `DatabaseEngine::save_pending_mutation` (`crates/nuncio-store/src/db.rs:2201`) binds only those 8 columns.
- Call sites: `crates/nunciod/src/grpc.rs:3654` and `crates/nunciod/src/outbox.rs:608` (both tests), plus the production caller in the filter-action path.

**Files:**
- Modify: `crates/nuncio-filter/src/ast.rs:322` — add `pub account_id: String`.
- Modify: `crates/nuncio-filter/src/outbox.rs:35` — add an `account_id` parameter to `create_mutation`, positioned first since it is the broadest scope.
- Modify: `crates/nuncio-store/src/db.rs` — migration near `ensure_accounts_dav_columns:954`; call site at `:753`; `CREATE TABLE` at `:575`; bind `account_id` in `save_pending_mutation:2201`; new count query near `count_pending_mutations:464`.
- Modify: call sites listed above.
- Test: `crates/nuncio-store/src/db.rs` inline `mod tests`, following `migrate_backfills_smtp_columns_for_a_pre_existing_accounts_table:3771`.

**Interfaces:**
- Consumes: nothing.
- Produces: `DatabaseEngine::count_pending_mutations_by_account() -> Result<Vec<(String, u64, u64)>, DatabaseError>` returning `(account_id, pending, failed)`, ordered by `account_id`. Task 4 calls this. Also `PendingRemoteMutation.account_id: String` and the new first parameter on `create_mutation`.

**Where the account comes from.** The filter-action path knows the `Email` being acted on, and `Email` carries its account. Thread that value into `create_mutation` rather than re-deriving it from the message id. If the production call site genuinely has no account in scope, STOP and report it — do not pass an empty string, which would silently reproduce the bug this task exists to fix.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/nuncio-store/src/db.rs`:

```rust
#[tokio::test]
async fn migrate_backfills_account_id_on_a_pre_existing_outbox_table() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("pre_account_id.db");
    let secrets = crate::vault::SecretManager::mock();

    // OLD-schema outbox table: no account_id column.
    {
        let engine = DatabaseEngine::new(&db_path, secrets.clone())
            .await
            .expect("engine");
        sqlx::query("DROP TABLE IF EXISTS pending_remote_mutations")
            .execute(&engine.pool)
            .await
            .expect("drop");
        sqlx::query(
            "CREATE TABLE pending_remote_mutations (
                id TEXT PRIMARY KEY NOT NULL,
                rule_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                mutation_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                retry_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&engine.pool)
        .await
        .expect("create old table");

        sqlx::query(
            "INSERT INTO messages (id, account_id, folder_id, remote_id)
             VALUES ('msg-1', 'acct-A', 'inbox', 'r1')",
        )
        .execute(&engine.pool)
        .await
        .expect("seed message");

        sqlx::query(
            "INSERT INTO pending_remote_mutations
             (id, rule_id, message_id, mutation_type, payload, status, retry_count, created_at)
             VALUES ('mut-1', 'rule-1', 'msg-1', 'move', '{}', 'pending', 0, 0)",
        )
        .execute(&engine.pool)
        .await
        .expect("seed mutation");
    }

    // Reopening runs migrate(), which must add and backfill account_id.
    let engine = DatabaseEngine::new(&db_path, secrets)
        .await
        .expect("reopen");

    let counts = engine
        .count_pending_mutations_by_account()
        .await
        .expect("counts");

    assert_eq!(counts, vec![("acct-A".to_string(), 1, 0)]);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nuncio-store migrate_backfills_account_id_on_a_pre_existing_outbox_table`
Expected: FAIL — `no method named 'count_pending_mutations_by_account'`.

- [ ] **Step 3: Add the migration**

In `crates/nuncio-store/src/db.rs`, after `ensure_accounts_dav_columns`, mirroring its `PRAGMA table_info` shape exactly:

```rust
    /// Additive, backfill-safe migration adding `account_id` to a pre-existing
    /// `pending_remote_mutations` table.
    ///
    /// The outbox originally recorded only the message a mutation targets, so
    /// queued work could not be attributed to an account. The column is added
    /// as `NULL`-able and backfilled by joining `message_id` to
    /// `messages.account_id`; a mutation whose message row is already gone
    /// keeps a `NULL` account and is reported separately rather than being
    /// silently dropped from the totals. SQLite has no `ADD COLUMN IF NOT
    /// EXISTS`, so presence is checked via `PRAGMA table_info` first, making
    /// this safe to run on every daemon startup.
    async fn ensure_pending_mutations_account_column(&self) -> Result<(), DatabaseError> {
        let existing_columns: Vec<String> =
            sqlx::query("PRAGMA table_info(pending_remote_mutations)")
                .fetch_all(&self.pool)
                .await
                .map_err(DatabaseError::Query)?
                .iter()
                .map(|row| row.get::<String, _>("name"))
                .collect();

        if !existing_columns.iter().any(|c| c == "account_id") {
            sqlx::query("ALTER TABLE pending_remote_mutations ADD COLUMN account_id TEXT")
                .execute(&self.pool)
                .await
                .map_err(DatabaseError::Query)?;
            tracing::info!(
                table = "pending_remote_mutations",
                column = "account_id",
                "applying schema migration"
            );

            sqlx::query(
                "UPDATE pending_remote_mutations
                 SET account_id = (
                     SELECT m.account_id FROM messages m
                     WHERE m.id = pending_remote_mutations.message_id
                 )
                 WHERE account_id IS NULL",
            )
            .execute(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        }

        Ok(())
    }
```

Add `account_id TEXT` to the `CREATE TABLE IF NOT EXISTS pending_remote_mutations` block at line 575 so fresh databases get it directly, and add the call after line 753:

```rust
        self.ensure_pending_mutations_account_column().await?;
```

- [ ] **Step 4: Add the per-account count query**

After `count_pending_mutations` at line 464:

```rust
    /// Per-account outbox depth as `(account_id, pending, failed)`, ordered by
    /// account id. Mutations whose `account_id` is `NULL` (their message row
    /// was deleted before the backfill ran) are grouped under the empty string
    /// rather than being dropped, so the reported totals always reconcile with
    /// [`Self::count_pending_mutations`].
    pub async fn count_pending_mutations_by_account(
        &self,
    ) -> Result<Vec<(String, u64, u64)>, DatabaseError> {
        let rows: Vec<(Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT account_id,
                    SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END)
             FROM pending_remote_mutations
             GROUP BY account_id
             ORDER BY account_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(id, pending, failed)| {
                (
                    id.unwrap_or_default(),
                    pending.max(0) as u64,
                    failed.max(0) as u64,
                )
            })
            .collect())
    }
```

- [ ] **Step 5: Close the write path**

Add `pub account_id: String` to `PendingRemoteMutation`, add `account_id: impl Into<String>` as the first parameter of `create_mutation`, set the field in the struct literal there, bind it in `save_pending_mutation`'s `INSERT` (add `account_id` to both the column list and the `VALUES` placeholders, and add `.bind(&item.account_id)` in matching position), and update every call site.

Add a test proving a freshly saved mutation is attributable:

```rust
#[tokio::test]
async fn a_newly_saved_mutation_is_attributed_to_its_account() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("ephemeral db");

    let mutation = nuncio_filter::OutboxManager::create_mutation(
        "acct-B",
        "rule-1",
        "msg-9",
        "MOVE",
        Some("Archive".to_string()),
    );
    engine
        .save_pending_mutation(&mutation)
        .await
        .expect("save mutation");

    let counts = engine
        .count_pending_mutations_by_account()
        .await
        .expect("counts");

    assert_eq!(counts, vec![("acct-B".to_string(), 1, 0)]);
}
```

Adjust the `create_mutation` path prefix to whatever type actually owns it in `crates/nuncio-filter/src/outbox.rs`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p nuncio-store migrate_backfills_account_id_on_a_pre_existing_outbox_table && cargo test -p nuncio-store a_newly_saved_mutation_is_attributed_to_its_account`
Expected: PASS.

- [ ] **Step 7: Run the full gate**

Run: `cargo fmt --all -- --check && cargo check-all && cargo test-all`
Expected: all green, 598+ tests passing (596 before this branch, plus your two new ones).

- [ ] **Step 8: Commit**

```bash
git add crates/nuncio-filter/src/ast.rs crates/nuncio-filter/src/outbox.rs \
        crates/nuncio-store/src/db.rs crates/nunciod/src
git commit -m "feat(store): attribute outbox mutations to an account"
```

---

## Phase 2 — `System.Shutdown`

> Proto-touching. Do not start Task 5 until this phase is merged.

### Task 2: Add the `Shutdown` RPC and handler

**Files:**
- Modify: `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto:78-90`
- Modify: `crates/nuncio-proto/proto/nuncio/v1/descriptor.bin` (regenerated)
- Modify: `crates/nunciod/src/grpc.rs:243` (struct), `:255` (impl), `:2979` (construction)
- Modify: `crates/nunciod/src/main.rs:52-54`
- Test: `crates/nunciod/src/grpc.rs` inline tests

**Interfaces:**
- Consumes: `ShutdownController::trigger()` from `crates/nunciod/src/lifecycle.rs:90`.
- Produces: `nuncio.v1.System/Shutdown` taking `ShutdownRequest{}` and returning `ShutdownResponse{}`. Tasks 3 and 11 call it.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/nunciod/src/grpc.rs`, alongside `get_status_rejects_missing_bearer_token` at line 3532:

```rust
#[tokio::test]
async fn shutdown_rejects_missing_bearer_token() {
    let event_bus = Arc::new(EventBus::new());
    let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

    let mut client = nuncio_proto::v1::system_client::SystemClient::connect(format!("http://{addr}"))
        .await
        .expect("connect");

    let status = client
        .shutdown(nuncio_proto::v1::ShutdownRequest {})
        .await
        .expect_err("must reject unauthenticated shutdown");

    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p nunciod shutdown_rejects_missing_bearer_token`
Expected: FAIL — `ShutdownRequest` does not exist.

- [ ] **Step 3: Add the proto surface**

In `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto`, inside `service System` (line 78):

```proto
  // Shutdown asks the daemon to begin a graceful shutdown: in-flight work is
  // allowed to finish, then the process exits. Returns before the process is
  // gone, so a caller that needs to observe the exit should poll the
  // single-instance lock rather than assume completion on response.
  //
  // This is the only way to stop the daemon cleanly on platforms without a
  // POSIX signal for it. It is guarded by the same bearer-auth interceptor as
  // every other RPC.
  rpc Shutdown(ShutdownRequest) returns (ShutdownResponse);
```

And after `GetStatusRequest` (line 90):

```proto
// ShutdownRequest carries no parameters. A future revision may add a drain
// deadline.
message ShutdownRequest {}

// ShutdownResponse is returned once shutdown has been *requested*, not once it
// has completed.
message ShutdownResponse {}
```

- [ ] **Step 4: Regenerate the descriptor golden**

The procedure is spelled out in the assertion message of `descriptor_matches_committed_golden` (`crates/nuncio-proto/src/lib.rs:112`). Concretely:

```bash
cargo build -p nuncio-proto
cp "$(find target/debug/build -name nuncio_v1_descriptor.bin -printf '%T@ %p\n' \
      | sort -rn | head -1 | cut -d' ' -f2-)" \
   crates/nuncio-proto/proto/nuncio/v1/descriptor.bin
cargo test -p nuncio-proto
```

**The `sort -rn` on mtime is load-bearing.** There are several
`target/debug/build/nuncio-proto-<hash>/out/` directories; picking by name
instead of newest-first can copy a STALE descriptor, committing a golden that
does not match the `.proto` and failing CI in a confusing way.

Expected: `descriptor.bin` updated; `cargo test -p nuncio-proto` passes.

- [ ] **Step 5: Wire the controller into the service**

In `crates/nunciod/src/grpc.rs`, add to `SystemGrpcService` (line 243):

```rust
    /// Triggers graceful shutdown when `Shutdown` is called. Optional because
    /// test servers are spawned without a real lifecycle controller; a `None`
    /// here makes `Shutdown` return `Unavailable` rather than pretending to
    /// succeed.
    shutdown: Option<Arc<ShutdownController>>,
```

Add the handler inside `impl System for SystemGrpcService`:

```rust
    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        match &self.shutdown {
            Some(controller) => {
                tracing::info!("shutdown requested over gRPC");
                controller.trigger();
                Ok(Response::new(ShutdownResponse {}))
            }
            None => Err(Status::unavailable(
                "this daemon instance has no lifecycle controller wired",
            )),
        }
    }
```

**There are six public serve entry points**, all funnelling to the single `SystemGrpcService` construction at `grpc.rs:2979`:

| Entry point | Line | Gets the controller? |
| :--- | :--- | :--- |
| `serve` | 2739 | No — pass `None` |
| `serve_on_listener` | 2790 | No — pass `None` |
| `serve_with_shutdown` | 2819 | **Yes** — this is the production path |
| `serve_on_listener_with_shutdown` | 2866 | **Yes** |
| `serve_on_listener_with_overrides` | 2915 | No — pass `None` |
| `serve_on_listener_with_overrides_and_shutdown` | 2963 | **Yes** |

**As implemented** (refined during Task 2 and accepted at review): the two outer shutdown-aware entry points, `serve_with_shutdown` and `serve_on_listener_with_shutdown`, take `shutdown_controller: Arc<ShutdownController>` **required**. Only the innermost `serve_on_listener_with_overrides_and_shutdown` carries `Option<Arc<ShutdownController>>`, and that Option is what reaches `SystemGrpcService`.

This is stricter than the original design and deliberately so: a function named `..._with_shutdown` that could be handed no controller represents an invalid state — a shutdown-aware server with no way to shut down. Requiring the `Arc` makes that unrepresentable at compile time instead of deferring it to a runtime `Unavailable`. The three non-shutdown entry points still pass `None`, which is why the field on the service stays an `Option` and why the handler returns `Unavailable` rather than fabricating success when unwired.

It is a breaking change to two `pub` signatures, accepted because the crate is pre-1.0 with no out-of-tree consumers and both in-tree callers were updated.

**`serve_with_shutdown` is the one production uses** (`crates/nunciod/src/main.rs:288`). Wiring only `serve_on_listener_with_overrides` would leave the real daemon returning `Unavailable` while every test passed.

- [ ] **Step 6: Share the controller in main.rs**

The controller is **already** an `Arc` at `crates/nunciod/src/main.rs:53`:

```rust
let (shutdown_controller, shutdown_signal) = ShutdownController::new(event_bus.clone());
let shutdown_controller = Arc::new(shutdown_controller);
let signal_task = tokio::spawn(install_signal_handlers(shutdown_controller));
```

Line 54 **moves** it into the signal task. Do not wrap it again — insert an `Arc::clone` before that spawn and pass the clone to `serve_with_shutdown`.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p nunciod shutdown_rejects_missing_bearer_token && cargo test -p nuncio-proto`
Expected: PASS, including the contract-stability golden.

- [ ] **Step 8: Commit**

Adding a required method to the `System` trait and changing two serve signatures forces two more files into the same commit: `crates/nuncio-cli/src/runner.rs` (its test-only `StubSystem` needs a `shutdown` stub, mirroring the existing `subscribe` stub) and `crates/nunciod/tests/graceful_shutdown_test.rs` (its controller must be wrapped in `Arc`). Both are compile-fallout, not scope.

```bash
git add crates/nuncio-proto crates/nunciod/src/grpc.rs crates/nunciod/src/main.rs \
        crates/nuncio-cli/src/runner.rs crates/nunciod/tests/graceful_shutdown_test.rs
git commit -m "feat(proto): add System.Shutdown for graceful stop"
```

### Task 3: CLI command and offline E2E for `Shutdown`

Definition of done in `CLAUDE.md` requires engine, proto, CLI command, and offline E2E together.

**Files:**
- Modify: `crates/nuncio-cli/src/args.rs`, `crates/nuncio-cli/src/runner.rs`
- Create: `crates/nunciod/tests/shutdown_e2e_test.rs`

**Interfaces:**
- Consumes: `nuncio.v1.System/Shutdown` from Task 2; `nuncio_proto::client::connect_system(addr, token)`.
- Produces: `nuncio system shutdown` CLI command.

- [ ] **Step 1: Write the failing E2E**

Create `crates/nunciod/tests/shutdown_e2e_test.rs`, following the shape of the existing `crates/nunciod/tests/graceful_shutdown_test.rs`:

```rust
//! Proves an authenticated `System.Shutdown` actually stops the daemon, using
//! the same wiring `main.rs` assembles: one `ShutdownController` shared by the
//! gRPC server and the lifecycle path. Never opens a socket off 127.0.0.1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::system_client::SystemClient;
use nuncio_proto::v1::ShutdownRequest;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::serve_on_listener_with_shutdown;
use nunciod::lifecycle::ShutdownController;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tonic::metadata::MetadataValue;
use tonic::Request;

const JOIN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn authenticated_shutdown_stops_the_serving_task() {
    let event_bus = Arc::new(EventBus::new());
    let (controller, shutdown_signal) = ShutdownController::new(event_bus.clone());
    let controller = Arc::new(controller);

    let secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    let (db, _dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let db = Arc::new(db);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    let serve = tokio::spawn(serve_on_listener_with_shutdown(
        listener,
        event_bus,
        db,
        filter_engine,
        secrets,
        token.clone(),
        Arc::clone(&controller),
        async move {
            let mut sig = shutdown_signal;
            sig.wait().await;
        },
    ));

    let mut client = SystemClient::connect(format!("http://{addr}"))
        .await
        .expect("connect");

    let mut request = Request::new(ShutdownRequest {});
    request.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {token}")).expect("valid metadata"),
    );

    client.shutdown(request).await.expect("shutdown accepted");

    // The RPC returns once shutdown is REQUESTED; the serve task must then
    // finish on its own. A timeout here means the trigger never reached the
    // lifecycle signal.
    let joined = tokio::time::timeout(JOIN_TIMEOUT, serve).await;
    assert!(
        joined.is_ok(),
        "gRPC serve task did not stop within {JOIN_TIMEOUT:?} of Shutdown"
    );
}
```

Adjust the `serve_on_listener_with_shutdown` argument list to match whatever signature Task 2 Step 5 settled on; the existing call in `crates/nunciod/tests/graceful_shutdown_test.rs` is the reference for every argument except the new controller.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p nunciod --test shutdown_e2e_test`
Expected: FAIL — no CLI/serve wiring yet, or an empty test body asserting nothing.

- [ ] **Step 3: Add the CLI command**

In `crates/nuncio-cli/src/args.rs`, add a `Shutdown` variant to the `system` subcommand enum. In `runner.rs`, implement it next to the existing status handler, reusing the `connect_system` helper and the token-from-vault pattern at `crates/nuncio-cli/src/runner.rs:1152`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p nunciod --test shutdown_e2e_test && cargo test -p nuncio-cli`
Expected: PASS.

- [ ] **Step 5: Run the full gate and commit**

```bash
cargo fmt --all -- --check && cargo check-all && cargo test-all
git add crates/nuncio-cli crates/nunciod/tests/shutdown_e2e_test.rs
git commit -m "feat(cli): add system shutdown command"
```

---

## Phase 3 — `System.GetHealth` (closes #296)

> Proto-touching. Start only after Phase 2 is merged.

### Task 4: Add the `GetHealth` RPC and handler

**Files:**
- Modify: `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto`, `descriptor.bin`
- Modify: `crates/nunciod/src/grpc.rs`
- Test: inline tests in `grpc.rs`

**Interfaces:**
- Consumes: `DatabaseEngine::count_pending_mutations_by_account()` from Task 1.
- Produces: `nuncio.v1.System/GetHealth` returning `GetHealthResponse`. Tasks 6 and 10 call it.

- [ ] **Step 1: Write the failing test**

Mirror the seeding in the existing `get_status_reports_live_accounts_and_outbox_depth` (`crates/nunciod/src/grpc.rs:3640`); it already builds accounts and outbox rows the same way.

```rust
    #[tokio::test]
    async fn get_health_reports_per_account_queue_depth() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);

        for id in ["acct-q1", "acct-q2"] {
            let account = nuncio_core::AccountConfig {
                id: id.to_string(),
                name: "Queue".to_string(),
                email_address: format!("{id}@nuncio.mx"),
                keyring_secret_key: format!("nuncio/{id}"),
                sync_interval_secs: 60,
                transport: nuncio_core::Transport::Jmap(nuncio_core::JmapTransport {
                    endpoint_host: "jmap.nuncio.mx".to_string(),
                }),
            };
            db.save_account(&account).await.expect("save account");
        }

        // acct-q1: two pending, one failed. acct-q2: nothing queued.
        for (id, status) in [
            ("mut-p1", "pending"),
            ("mut-p2", "pending"),
            ("mut-f1", "failed"),
        ] {
            db.save_pending_mutation(&nuncio_filter::PendingRemoteMutation {
                id: id.to_string(),
                account_id: "acct-q1".to_string(),
                rule_id: "rule-1".to_string(),
                message_id: format!("msg-{id}"),
                mutation_type: "MOVE".to_string(),
                payload: "{}".to_string(),
                status: status.to_string(),
                retry_count: 0,
                created_at: 0,
            })
            .await
            .expect("save mutation");
        }

        let event_bus = Arc::new(EventBus::new());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let secrets = Arc::new(SecretManager::mock());
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            db,
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = nuncio_proto::client::connect_system(
            &format!("http://{addr}"),
            "correct-token",
        )
        .await
        .expect("connect");

        let health = client
            .get_health(GetHealthRequest {})
            .await
            .expect("get_health")
            .into_inner();

        let q1 = health
            .account_queues
            .iter()
            .find(|q| q.account_id == "acct-q1")
            .expect("acct-q1 must be reported");
        assert_eq!(q1.pending, 2);
        assert_eq!(q1.failed, 1);

        // An account with nothing queued must be PRESENT with zeros, never
        // omitted: absent and idle must not look the same to a client.
        let q2 = health
            .account_queues
            .iter()
            .find(|q| q.account_id == "acct-q2")
            .expect("an idle account must still be reported");
        assert_eq!(q2.pending, 0);
        assert_eq!(q2.failed, 0);
    }
```

Note the assertion on `acct-q2`: `count_pending_mutations_by_account` only returns rows that exist in `pending_remote_mutations`, so the handler must join against the configured account list to emit zero rows for idle accounts. If you implement it as a straight pass-through of the store query, this test fails — that is intentional.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p nunciod get_health_reports_per_account_queue_depth`
Expected: FAIL — `GetHealthResponse` does not exist.

- [ ] **Step 3: Add the proto surface**

```proto
  // GetHealth reports operational depth: per-account outbox backlog, the
  // per-account last-sync outcome, and database size signals. Distinct from
  // GetStatus, which answers "is the engine up"; this answers "is it keeping
  // up". Every value is read live when the RPC is served.
  rpc GetHealth(GetHealthRequest) returns (GetHealthResponse);
```

```proto
message GetHealthRequest {}

// Outbox backlog for one account. `pending` and `failed` are reported
// SEPARATELY and must never be summed into one number by a caller: a merged
// total hides a stuck queue behind a healthy-looking figure.
message AccountQueueDepth {
  string account_id = 1;
  uint64 pending = 2;
  uint64 failed = 3;
}

message GetHealthResponse {
  repeated AccountQueueDepth account_queues = 1;
  uint64 wal_size_bytes = 2;
  bool db_healthy = 3;
}
```

- [ ] **Step 4: Regenerate the golden and implement the handler**

Regenerate `descriptor.bin` using the exact command block in Task 2 Step 4, including the mtime sort.

Implement `get_health` on `SystemGrpcService`. It must **join the store counts against the configured account list**, not pass them through: `count_pending_mutations_by_account()` returns only accounts that have rows in the outbox, so a pass-through omits idle accounts entirely and the `acct-q2` assertion in Step 1 fails by design. Enumerate accounts with `self.db.list_accounts()` — the same call `get_status` already makes at `grpc.rs:270` — and emit a zero-filled entry for any account with no queued work.

Return an honest error on a store read failure rather than an empty list: empty means "no queued work", failure means "unknown", and the two must not be conflated.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p nunciod get_health && cargo test -p nuncio-proto`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/nuncio-proto crates/nunciod/src/grpc.rs
git commit -m "feat(proto): add System.GetHealth operational surface"
```

### Task 5: CLI command and offline E2E for `GetHealth`

**Files:**
- Modify: `crates/nuncio-cli/src/args.rs`, `runner.rs`
- Create: `crates/nunciod/tests/health_e2e_test.rs`

**Interfaces:**
- Consumes: `System/GetHealth` from Task 4.
- Produces: `nuncio system health` CLI command.

- [ ] **Step 1: Write the failing E2E**

Create `crates/nunciod/tests/health_e2e_test.rs`. Use `crates/nunciod/tests/graceful_shutdown_test.rs` as the harness reference for token minting and server startup.

```rust
//! Offline E2E for `System.GetHealth`: real gRPC over loopback, real store,
//! mock keyring. No network beyond 127.0.0.1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::GetHealthRequest;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use std::sync::Arc;
use tokio::net::TcpListener;

#[tokio::test]
async fn authenticated_get_health_reports_seeded_queue_depth() {
    let secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );
    let (db, _dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let db = Arc::new(db);

    let account = nuncio_core::AccountConfig {
        id: "acct-e2e".to_string(),
        name: "E2E".to_string(),
        email_address: "e2e@nuncio.mx".to_string(),
        keyring_secret_key: "nuncio/acct-e2e".to_string(),
        sync_interval_secs: 60,
        transport: nuncio_core::Transport::Jmap(nuncio_core::JmapTransport {
            endpoint_host: "jmap.nuncio.mx".to_string(),
        }),
    };
    db.save_account(&account).await.expect("save account");

    db.save_pending_mutation(&nuncio_filter::PendingRemoteMutation {
        id: "mut-e2e".to_string(),
        account_id: "acct-e2e".to_string(),
        rule_id: "rule-1".to_string(),
        message_id: "msg-1".to_string(),
        mutation_type: "MOVE".to_string(),
        payload: "{}".to_string(),
        status: "pending".to_string(),
        retry_count: 0,
        created_at: 0,
    })
    .await
    .expect("save mutation");

    let event_bus = Arc::new(EventBus::new());
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    tokio::spawn(nunciod::grpc::serve_on_listener(
        listener,
        event_bus,
        db,
        filter_engine,
        secrets,
        token.clone(),
    ));

    let mut client = nuncio_proto::client::connect_system(&format!("http://{addr}"), &token)
        .await
        .expect("connect");

    let health = client
        .get_health(GetHealthRequest {})
        .await
        .expect("get_health")
        .into_inner();

    let queue = health
        .account_queues
        .iter()
        .find(|q| q.account_id == "acct-e2e")
        .expect("seeded account must be reported");
    assert_eq!(queue.pending, 1);
    assert_eq!(queue.failed, 0);
}

#[tokio::test]
async fn unauthenticated_get_health_is_rejected() {
    let secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );
    let (db, _dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let event_bus = Arc::new(EventBus::new());
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    tokio::spawn(nunciod::grpc::serve_on_listener(
        listener,
        event_bus,
        Arc::new(db),
        filter_engine,
        secrets,
        token,
    ));

    let mut client =
        nuncio_proto::v1::system_client::SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("connect");

    let status = client
        .get_health(GetHealthRequest {})
        .await
        .expect_err("must reject unauthenticated GetHealth");

    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}
```

If `serve_on_listener`'s argument list differs after Task 2's controller threading, match whatever signature exists then; `graceful_shutdown_test.rs` is the live reference.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p nunciod --test health_e2e_test`
Expected: FAIL.

- [ ] **Step 3: Add the CLI command**

Mirror Task 3 Step 3: a `Health` variant plus a runner arm reusing `connect_system`.

- [ ] **Step 4: Run the gate and commit**

```bash
cargo fmt --all -- --check && cargo check-all && cargo test-all
git add crates/nuncio-cli crates/nunciod/tests/health_e2e_test.rs
git commit -m "feat(cli): add system health command

Closes #296"
```

---

## Phase 4 — The monitor crate

Tasks 6 through 8 depend on nothing in Phases 1 to 3 and may be built in parallel with them.

### Task 6: Scaffold the crate and add dependencies

**Files:**
- Create: `crates/nuncio-monitor/Cargo.toml`, `crates/nuncio-monitor/src/main.rs`
- Modify: root `Cargo.toml` (members list and `[workspace.dependencies]`)

**Interfaces:**
- Produces: a buildable `nuncio-monitor` binary that the gate covers.

- [ ] **Step 1: Add the workspace member and let cargo resolve the GUI versions**

In the root `Cargo.toml`, add `"crates/nuncio-monitor"` to `members`.

**Do not hand-pin the GUI crate versions.** Any version numbers written here would be guesses. Let cargo resolve versions compatible with the pinned 1.97.1 toolchain:

```bash
cargo add --package nuncio-monitor eframe egui egui_extras tray-icon
```

Then lift the resolved versions into `[workspace.dependencies]` at the root and switch the crate's entries to `{ workspace = true }`, matching how every other shared dependency in this workspace is declared.

**Record the resolved versions in your report.** If cargo resolves a version requiring a newer rustc than 1.97.1, STOP and report it — do not bump the toolchain, which is a Global Constraint.

- [ ] **Step 2: Create the crate manifest**

```toml
[package]
name = "nuncio-monitor"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true

[lints]
workspace = true

[dependencies]
nuncio-proto = { path = "../nuncio-proto" }
nuncio-store = { path = "../nuncio-store" }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
eframe = { workspace = true }
egui = { workspace = true }
egui_extras = { workspace = true }
tray-icon = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 3: Create a minimal main that compiles**

```rust
//! Nuncio Monitor: a development instrument for watching a running `nunciod`.

fn main() {
    println!("nuncio-monitor");
}
```

- [ ] **Step 4: Verify the gate covers it**

Run: `cargo check-all`
Expected: `nuncio-monitor` appears in the checked crates and passes.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/nuncio-monitor
git commit -m "feat(monitor): scaffold the nuncio-monitor crate"
```

### Task 7: `LogTailer`

**Files:**
- Create: `crates/nuncio-monitor/src/log_tail.rs`
- Test: same file, inline `mod tests`

**Interfaces:**
- Consumes: `nunciod::logging::log_dir_for_db_path` semantics (logs live at `<data_dir>/logs/nunciod.log.<date>`). Do not depend on the `nunciod` crate; take the directory as a parameter.
- Produces: `LogRecord { timestamp: String, level: String, target: String, request_id: Option<String>, message: String, raw: String }`; `LogAvailability { Ok, DirectoryMissing, NotJson }`; `LogTailer::new(dir: PathBuf) -> Self`, `LogTailer::poll(&mut self) -> Vec<LogRecord>`, `LogTailer::availability(&self) -> LogAvailability`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn parses_a_json_line_into_a_record() {
    let line = r#"{"timestamp":"2026-08-07T12:00:00Z","level":"INFO","target":"nunciod","fields":{"message":"started","request_id":"abc"}}"#;
    let rec = LogRecord::parse(line);
    assert_eq!(rec.level, "INFO");
    assert_eq!(rec.request_id.as_deref(), Some("abc"));
    assert_eq!(rec.message, "started");
}

#[test]
fn a_non_json_line_becomes_an_opaque_record_not_an_error() {
    let rec = LogRecord::parse("2026-08-07 plain text log line");
    assert_eq!(rec.level, "RAW");
    assert_eq!(rec.raw, "2026-08-07 plain text log line");
}

#[test]
fn a_malformed_json_line_mid_stream_does_not_abort_the_tail() {
    let lines = ["{\"level\":\"INFO\"}", "{not json", "{\"level\":\"WARN\"}"];
    let recs: Vec<_> = lines.iter().map(|l| LogRecord::parse(l)).collect();
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[1].level, "RAW");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p nuncio-monitor log_tail`
Expected: FAIL — `LogRecord` not defined.

- [ ] **Step 3: Implement `LogRecord::parse` and the tailer**

Implement parsing with `serde_json::Value` so an unexpected shape degrades to `RAW` instead of erroring. Implement `LogTailer` holding the directory, the currently open file, its dated filename, and the last read offset. `poll()` reopens when the dated filename changes (midnight rotation) and reads only from the stored offset. Seek to end on first open so history is not replayed.

- [ ] **Step 4: Add a rotation test**

```rust
#[test]
fn rotating_to_a_new_dated_file_continues_the_tail() {
    use std::io::Write;

    let dir = tempfile::tempdir().expect("tempdir");

    let day_one = dir.path().join("nunciod.log.2026-08-07");
    let mut f1 = std::fs::File::create(&day_one).expect("create day one");
    writeln!(f1, r#"{{"level":"INFO","fields":{{"message":"first"}}}}"#).expect("write");
    f1.flush().expect("flush");

    let mut tailer = LogTailer::new(dir.path().to_path_buf());

    // First poll establishes the position; a tailer seeks to end on open, so
    // pre-existing history is deliberately NOT replayed.
    let _ = tailer.poll();

    writeln!(f1, r#"{{"level":"WARN","fields":{{"message":"second"}}}}"#).expect("write");
    f1.flush().expect("flush");
    let batch = tailer.poll();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].message, "second");

    // Rotation: a newer dated file appears. The tailer must switch to it and
    // read from its start, not stay pinned to the previous day.
    let day_two = dir.path().join("nunciod.log.2026-08-08");
    let mut f2 = std::fs::File::create(&day_two).expect("create day two");
    writeln!(f2, r#"{{"level":"ERROR","fields":{{"message":"third"}}}}"#).expect("write");
    f2.flush().expect("flush");

    let batch = tailer.poll();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].message, "third");
    assert_eq!(batch[0].level, "ERROR");
}
```

`LogTailer` must therefore select the lexicographically greatest `nunciod.log.*` in the directory on each poll rather than caching one handle forever. Dates are ISO-8601, so lexicographic order is chronological order.

- [ ] **Step 5: Handle a missing log directory honestly**

`docs/LOGGING.md` states the daemon degrades to stderr-only logging (with a `warn!`) when the log directory cannot be created, rather than failing to start. So an absent directory means "this daemon is not writing logs", **not** "no logs yet", and the UI must say so instead of rendering an empty pane as if healthy.

```rust
#[test]
fn a_missing_log_directory_is_reported_not_silently_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut tailer = LogTailer::new(dir.path().join("does-not-exist"));
    assert_eq!(tailer.poll(), Vec::new());
    assert_eq!(tailer.availability(), LogAvailability::DirectoryMissing);
}
```

Add `LogAvailability { Ok, DirectoryMissing, NotJson }` and a `LogTailer::availability(&self)` accessor. Task 11 renders it.

- [ ] **Step 6: Run the tests and commit**

```bash
cargo test -p nuncio-monitor
git add crates/nuncio-monitor/src/log_tail.rs
git commit -m "feat(monitor): tail and parse the daemon's rotating JSON log"
```

### Task 8: `EngineController`

**Files:**
- Create: `crates/nuncio-monitor/src/engine.rs`
- Test: same file, inline `mod tests`

**Interfaces:**
- Consumes: `System/Shutdown` from Task 2.
- Produces: `EngineState { Stopped, Starting, Running, NotResponding }` and `EngineController::liveness(&self) -> EngineState`, `EngineController::start(&self) -> Result<(), EngineError>`, `EngineController::stop(&self) -> Result<(), EngineError>`.

- [ ] **Step 1: Write the failing liveness tests**

```rust
#[test]
fn no_lock_file_means_stopped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("nuncio.db");
    let ctl = EngineController::new(db, Launcher::noop());
    assert_eq!(ctl.liveness(), EngineState::Stopped);
}

#[test]
fn a_held_lock_file_means_not_stopped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("nuncio.db");

    // Hold the lock exactly as the daemon does, via the same type, so this
    // test breaks if the daemon's locking strategy ever changes.
    let _held = nunciod::lock::InstanceLock::acquire(&db).expect("acquire lock");

    let ctl = EngineController::new(db, Launcher::noop());
    assert_ne!(ctl.liveness(), EngineState::Stopped);
}

#[test]
fn a_released_lock_returns_to_stopped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("nuncio.db");

    {
        let _held = nunciod::lock::InstanceLock::acquire(&db).expect("acquire lock");
    } // guard dropped: the OS releases the lock here

    let ctl = EngineController::new(db, Launcher::noop());
    assert_eq!(ctl.liveness(), EngineState::Stopped);
}
```

Using `nunciod::lock::InstanceLock` in the test means adding `nunciod` as a **dev-dependency** of `nuncio-monitor`, not a regular one. The monitor's production code must not depend on the daemon crate; it detects the lock by attempting its own advisory lock on `<db_path>.lock`. If pulling `nunciod` in as a dev-dependency creates a dependency cycle, STOP and report it rather than weakening the test to a file-existence check — a lock file can exist without being held, and the two states must not be conflated.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p nuncio-monitor engine`
Expected: FAIL — `EngineController` not defined.

- [ ] **Step 3: Implement liveness and an injected launcher**

Liveness reads `<db_path>.lock` and attempts a non-blocking shared lock: success means no daemon holds it (Stopped), failure means one does. Define `Launcher` as a trait with a `spawn(&self) -> Result<(), EngineError>` method so tests inject a no-op and production spawns `nunciod.exe` detached with `NUNCIO_LOG_FORMAT=json` set.

- [ ] **Step 4: Implement `stop` against the Shutdown RPC**

`stop()` calls `System/Shutdown`, then polls liveness until Stopped or a bounded timeout elapses. On timeout return an error; **do not** fall back to killing the process.

- [ ] **Step 5: Run the tests and commit**

```bash
cargo test -p nuncio-monitor
git add crates/nuncio-monitor/src/engine.rs
git commit -m "feat(monitor): detect engine liveness and control lifecycle"
```

### Task 9: `StatusPoller`

**Files:**
- Create: `crates/nuncio-monitor/src/status.rs`

**Interfaces:**
- Consumes: `nuncio_proto::client::connect_system`, `connect_accounts`; `System/GetStatus`, `System/GetHealth`, `System/Subscribe`, `Accounts/ListAccounts`; `nuncio_store::vault::GRPC_TOKEN_ACCOUNT` (`"grpc-bearer-token"`).
- Produces: `StatusPoller::spawn(addr, secrets) -> mpsc::Receiver<StatusUpdate>` where `StatusUpdate` carries `Option<GetStatusResponse>`, `Option<GetHealthResponse>`, `Vec<AccountConfig>`, and a `stream_stale: bool`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn a_missing_token_is_an_honest_error_not_an_empty_status() {
    let secrets = nuncio_store::vault::SecretManager::mock();
    let err = StatusPoller::connect("127.0.0.1:9420", &secrets)
        .await
        .expect_err("must fail without a minted token");
    assert!(matches!(err, StatusError::NoToken));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p nuncio-monitor status`
Expected: FAIL — `StatusPoller` not defined.

- [ ] **Step 3: Implement token resolution and dialing**

Two things here are easy to get wrong and both are silent failures.

**Use `get_secret`, never `get_or_create_key_bytes`.** `get_or_create_key_bytes` (`crates/nuncio-store/src/vault.rs:221`) MINTS a fresh key when none exists. The monitor is a read-only client: if no token exists, that means the daemon has never run on this machine, which is a state the UI must report — not paper over by writing key material into the user's OS keyring merely because they opened the app.

**Do not hex-encode the result.** `get_secret` returns the **already hex-encoded** string; `get_or_create_key_bytes` hex-*decodes* it. Encoding again yields a token that never authenticates.

```rust
let token = match secrets.get_secret(nuncio_store::vault::GRPC_TOKEN_ACCOUNT) {
    Ok(hex_token) => hex_token,
    Err(nuncio_store::vault::VaultError::NotFound(_)) => return Err(StatusError::NoToken),
    Err(e) => return Err(StatusError::Vault(e.to_string())),
};
nuncio_proto::client::connect_system(addr, &token).await
```

(For contrast, `nuncio-cli` at `runner.rs:1152` does `hex::encode(get_or_create_key_bytes(..))` — a decode-then-re-encode round trip, which is why it works there and why copying it here would be wrong.)

- [ ] **Step 4: Implement the poll and stream loop**

Spawn a task running a timed `GetStatus` + `GetHealth` poll alongside a `Subscribe` stream. On stream error, mark `stream_stale` and reconnect with capped exponential backoff. Never let a failed poll emit zeros; emit `None` so the UI renders unknown.

- [ ] **Step 5: Make `EngineState::NotResponding` actually reachable**

`EngineController::liveness()` deliberately does **not** probe over gRPC: doing so would force every liveness call, including its unit tests, to read the real OS keyring and dial the real port, breaking the no-live-network and mock-keyring rules. It therefore reports only what the advisory lock can tell it, and `NotResponding` is currently unreachable in practice from that component alone.

**This task owns the composite state**, because it is where the authenticated channel already lives. Derive the state the UI displays by combining the two signals:

| Lock | RPC reachable | Displayed state |
| :--- | :--- | :--- |
| free | — | `Stopped` |
| held | yes, `ready` | `Running` |
| held | yes, not `ready` | `Starting` |
| held | no | `NotResponding` |

Expose that composite on `StatusUpdate` so `ui.rs` renders one state rather than re-deriving it. A wedged daemon must not display as either cleanly running or cleanly stopped.

**Test this with an injected reachability result**, not a live dial — the mapping is pure logic and must be unit-testable offline. If you find yourself needing a real socket to test the table above, the seam is in the wrong place.

- [ ] **Step 5: Run the tests and commit**

```bash
cargo test -p nuncio-monitor
git add crates/nuncio-monitor/src/status.rs
git commit -m "feat(monitor): poll daemon status and follow the event stream"
```

### Task 10: `AppState`

**Files:**
- Create: `crates/nuncio-monitor/src/state.rs`

**Interfaces:**
- Consumes: `LogRecord` (Task 7), `EngineState` (Task 8), `StatusUpdate` (Task 9).
- Produces: `AppState` with `engine: EngineState`, `status: Option<GetStatusResponse>`, `health: Option<GetHealthResponse>`, `accounts: Vec<AccountConfig>`, `logs: VecDeque<LogRecord>`, `log_filter: LogFilter`, `stream_stale: bool`; and `AppState::visible_logs(&self) -> Vec<&LogRecord>`.

- [ ] **Step 1: Write the failing filter tests**

```rust
#[test]
fn filtering_by_request_id_returns_only_that_requests_lines() {
    let mut st = AppState::default();
    st.push_log(rec_with_request_id("a"));
    st.push_log(rec_with_request_id("b"));
    st.log_filter.request_id = Some("a".into());
    assert_eq!(st.visible_logs().len(), 1);
}

#[test]
fn the_log_buffer_is_bounded_and_drops_oldest_first() {
    let mut st = AppState::default();
    for i in 0..(AppState::MAX_LOG_LINES + 10) {
        st.push_log(rec_with_message(&i.to_string()));
    }
    assert_eq!(st.logs.len(), AppState::MAX_LOG_LINES);
    assert_eq!(st.logs.front().map(|r| r.message.as_str()), Some("10"));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p nuncio-monitor state`
Expected: FAIL — `AppState` not defined.

- [ ] **Step 3: Implement `AppState`**

Bound `logs` with a `VecDeque` and a `MAX_LOG_LINES` constant (start at 5000) so a debug-level sync cannot exhaust memory. Implement `visible_logs` applying level, text, and `request_id` filters in that order.

- [ ] **Step 4: Run the tests and commit**

```bash
cargo test -p nuncio-monitor
git add crates/nuncio-monitor/src/state.rs
git commit -m "feat(monitor): add bounded, filterable application state"
```

### Task 11: UI and tray, wired together

**Files:**
- Create: `crates/nuncio-monitor/src/ui.rs`, `crates/nuncio-monitor/src/tray.rs`
- Modify: `crates/nuncio-monitor/src/main.rs`

**Interfaces:**
- Consumes: everything from Tasks 6 to 10.
- Produces: the running application.

- [ ] **Step 1: Implement the tray**

Icon plus menu: Show, Start Engine, Stop Engine, Quit. Icon colour encodes `EngineState`. Start and Stop are disabled when not applicable rather than failing when clicked.

**Event-loop hazard, read before writing code.** On Windows, `tray-icon` needs a Win32 message pump on the thread that created the icon, and `eframe`/`winit` owns the event loop. Create the tray **after** the event loop is running, on the event-loop thread, and route menu activations through winit's user-event proxy. Do **not** spawn a second event loop on a side thread: it appears to work, then drops menu events or deadlocks under load, presenting as "the window is fine but the tray menu is dead". If the resolved `tray-icon` version cannot cohabit with the resolved `eframe`, STOP and report rather than working around it with a second loop.

- [ ] **Step 2: Implement the UI panes**

Status header rendering `EngineState`, version, uptime, readiness, and a stale-stream indicator. Accounts table with one row per account showing sync state, last sync, last error, and `pending`/`failed` **as separate columns**. Log table via `egui_extras::TableBuilder` with a level dropdown, a search box, and a clickable `request_id` cell that sets the filter.

- [ ] **Step 3: Render unknown distinctly from zero**

Every numeric cell renders an em dash when its source is `None`. Add a manual check: stop the daemon and confirm no pane shows `0`.

- [ ] **Step 4: Wire main.rs**

Start the Tokio runtime on a worker thread, spawn `StatusPoller` and `LogTailer`, run `eframe` on the main thread, and drain channels into `AppState` each frame.

- [ ] **Step 5: Run the gate**

```bash
cargo fmt --all -- --check && cargo check-all && cargo test-all
```

- [ ] **Step 6: Manual verification**

Start the daemon, confirm the tray icon goes green, logs stream, accounts populate. Stop it from the tray and confirm a graceful exit (the log shows the shutdown path, not an abrupt end).

- [ ] **Step 7: Commit**

```bash
git add crates/nuncio-monitor
git commit -m "feat(monitor): add tray icon and status, accounts, and log panes"
```

---

## Documentation

- [ ] Add a Nuncio Monitor section to `docs/RUNNING.md` covering how to launch it, that it needs `NUNCIO_LOG_FORMAT=json` for filtering, and that it attaches to an already-running daemon.
- [ ] Update `docs/ROADMAP.md` and `docs/roadmap/index.html` **in the same commit** if this changes milestone status. #296 moving to closed changes the WS-C counts.
