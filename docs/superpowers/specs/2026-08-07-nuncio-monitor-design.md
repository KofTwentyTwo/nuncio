# Nuncio Monitor — design

Status: approved 2026-08-07. Implementation plan not yet written.

## Purpose

A Windows system-tray instrument for watching a running `nunciod` during
development: start and stop the engine, follow its logs live with structured
filtering, and see configured accounts with their per-account sync state and
queue depth.

It is a **development and operations tool**, not a product client. The native
macOS and Windows GUIs remain separate repositories per
[`docs/adr/0001-engine-first-grpc-architecture.md`](../../adr/0001-engine-first-grpc-architecture.md);
this crate does not replace or preempt them. It holds no business logic and no
data store, so the library-first boundary in `CLAUDE.md` is preserved.

## Decisions

| Decision | Choice | Rationale |
| :--- | :--- | :--- |
| What it is | In-repo dev instrument | Fastest to something useful; free to read the log file and control the process, which a contract-only client could not do. |
| Where it lives | Workspace member `crates/nuncio-monitor` | The gate (`cargo check-all`, `cargo test-all`, `-D warnings`, `unwrap_used`/`panic` denies) covers it automatically. A tool outside the gate rots. |
| Engine lifetime | Independent | The daemon outlives the monitor. Closing the tray never stops mail sync; a monitor crash cannot take the engine down. `start` spawns detached. |
| Stop mechanism | New `System.Shutdown` RPC | Windows has no `SIGTERM`. An authenticated RPC is cross-platform, testable offline, serves future clients, and avoids OS-specific daemon code. |
| Log view | Structured JSON viewer | Level filter, text search, and click-to-follow `request_id` across an RPC. The `request_id` correlation is the payoff of the OBS epic. |
| Build order | All pieces in one pass | Monitor plus both engine changes designed and planned together. One design and one plan, sequenced per "Build order" below, not one commit. |

## Scope

In scope: engine start/stop/liveness, structured live log viewing, account list
with per-account sync state and queue depth, tray icon encoding coarse state.

Out of scope: adding or editing accounts, triggering syncs, retrying the outbox,
reading mail, autostart at login. The monitor observes and controls lifecycle;
it does not operate the mail engine.

## Architecture

Three data sources, deliberately separate. The monitor never touches SQLite.

| Source | Provides | Why this source |
| :--- | :--- | :--- |
| gRPC on `127.0.0.1:9420` | Engine status, per-account state, queue depths, live events, account list | The contract. |
| `%USERPROFILE%\.nuncio\logs\nunciod.log.<date>` | The log viewer | Logs are not in the contract, and a log-streaming RPC is a far larger design. Tailing the file the daemon already writes is the honest cheap path for a dev tool. |
| `<db_path>.lock` and OS process control | Liveness and starting the engine | A stopped daemon cannot answer an RPC about itself. The single-instance advisory lock is already the authoritative "is one running" signal. |

### Engine changes

All three serialize through the PO's proto queue, since two touch `nuncio.v1`
and `HANDOFF.md` allows one proto-touching story in flight at a time.

1. **`System.Shutdown`** — new RPC on the `System` service, behind the existing
   bearer-auth interceptor, triggering the shutdown signal in
   `crates/nunciod/src/lifecycle.rs`. Any client holding the token can stop the
   daemon; the token already grants full mail access, so this adds little real
   privilege, but it is a genuine new capability in the contract and is recorded
   as such.
2. **`pending_remote_mutations.account_id`** — additive column plus a backfill
   joining `message_id` to `messages.account_id`, following the existing
   `migrate_backfills_smtp_columns_for_a_pre_existing_accounts_table` precedent.
   The table today carries `id, rule_id, message_id, mutation_type, payload,
   status, retry_count, created_at` and has no account column, so per-account
   outbox counts are not currently derivable. This is a prerequisite, not a
   nicety.
3. **`System.GetHealth`** — implements existing open story **#296 (WS-C8)**
   rather than inventing a parallel surface: outbox pending and failed per
   account, per-account last-sync outcome, WAL size. #296 is claimed for this
   work and annotated with the `account_id` prerequisite. Its own note to
   de-duplicate with WS-F10 still applies.

   "Queue depth" throughout this document means **two counts per account,
   reported separately**: `pending` and `failed`, matching the `status` column
   on `pending_remote_mutations` and #296's own wording. A single merged number
   would hide a stuck outbox behind a healthy-looking total, which the
   three-state display rule below exists to prevent.

### Build order

"One pass" means one design and one plan, not one commit. `HANDOFF.md` allows
only one proto-touching story in flight at a time, so the work has a required
sequence:

1. `pending_remote_mutations.account_id` plus backfill. Store-only, touches no
   proto, so it can land first and unblocks the rest.
2. `System.Shutdown`. Proto change one. Small and self-contained.
3. `System.GetHealth` (#296). Proto change two, depends on step 1 for its
   per-account counts.
4. `crates/nuncio-monitor`. Depends on steps 2 and 3 for stop and queue depth,
   but its `LogTailer`, `EngineController` liveness, and `ui` shell depend on
   none of them and can be built in parallel with the earlier steps.

`System.GetStatus` already supplies `engine_status`, `version`, `uptime`,
`accounts_loaded`, `unread_count`, `last_error`, a global `outbox_depth`,
`ready`, `db_healthy`, and repeated `account_sync_states` carrying `account_id`,
`state`, `last_synced`, and `last_error`. `System.Subscribe` already provides a
live server-streaming event feed. Neither needs changing.

### Consuming an unfrozen contract

M7 has not started and WS-A is still reshaping `nuncio.v1`, so anything the
monitor consumes may move under it. For a dev instrument that cost is
acceptable, but it argues for keeping the client surface small: four RPCs
(`GetStatus`, `GetHealth`, `Subscribe`, `Shutdown`) plus `ListAccounts`, not
thirty.

## Components

Five, each independently testable.

- **`EngineController`** — owns lifecycle. `is_running()` reads the lock file.
  `start()` spawns `nunciod.exe` detached with `NUNCIO_LOG_FORMAT=json`, then
  polls `GetStatus` until `ready`. `stop()` calls `System.Shutdown` and waits for
  the lock to release. Parses no logs and renders nothing.
- **`StatusPoller`** — owns the authenticated channel. Reads the bearer token
  from the keyring under `nuncio_store::vault::GRPC_TOKEN_ACCOUNT`
  (`"grpc-bearer-token"`), hex-encodes it, and dials through the existing
  `nuncio_proto::client::connect_system` / `connect_accounts` helpers, the same
  path `nuncio-cli` uses. Runs a timed `GetStatus`/`GetHealth` poll for current
  values alongside a `Subscribe` stream for push. Both are needed: the stream
  says when something happened, the poll gives current depth and readiness.
- **`LogTailer`** — follows the current day's file, seeking to the end rather
  than replaying history. Handles daily rotation by reopening when the dated
  filename changes. Parses one JSON object per line into a structured record
  (timestamp, level, target, `request_id`, fields, message) and falls back to
  opaque text when the daemon was not started in JSON mode.
- **`ui`** — status header, accounts table, and log table with level filter,
  text search, and click-a-`request_id`-to-filter. Pure rendering.
- **`tray`** — icon and menu; the icon encodes coarse state (stopped, running,
  error) so a glance at the taskbar is meaningful without opening the window.

### Data flow

```
lock file ──────────► EngineController ──┐
                                         ├──► AppState ──► ui / tray
gRPC (poll + stream) ► StatusPoller ─────┤
                                         │
log file (JSON) ─────► LogTailer ────────┘
```

`egui` runs on the main thread, a Tokio runtime on a worker, communicating over
channels. `AppState` is the single thing the UI reads. No component calls
another directly, which is what makes them testable without a running daemon.

## Error handling

The governing rule is the repo's anti-theater guardrail: no fabricated success.
Applied here, **every value has three states, not one** — a value, unknown, or
error. A monitor rendering `0 queued` when it actually failed to reach the
daemon is exactly the class of lie this codebase was rebuilt to eliminate.
Unknown must be visually distinct from zero.

| Condition | Behaviour |
| :--- | :--- |
| No lock file | Stopped. Not an error. Start enabled; panes show unknown, not zeros. |
| Lock held, gRPC refuses | Starting or wedged. Distinct from stopped: show starting, with a timeout escalating to "not responding" rather than spinning forever. |
| No token in keyring | The daemon has never run on this machine, so no token was minted. Expected fresh-machine state; say so actionably. |
| `Unauthenticated` | Token rotated. Re-read the keyring once and redial; if it fails again, report plainly. |
| Log directory missing | The daemon degrades to stderr-only rather than failing to start. Surface that; do not render an empty pane as healthy. |
| Log not JSON | Fall back to opaque text and state why filtering is unavailable. |
| `Subscribe` drops | Reconnect with backoff; mark the stream stale so the UI does not imply live data. |
| `Shutdown` returns, process persists | Wait, then report honestly. Force-kill exists only as a separate, explicitly labelled action, never a silent fallback, since that would reintroduce the ungraceful exit `lifecycle.rs` removed. |
| Start blocked by instance lock | `InstanceLock` already returns a typed `AlreadyHeld`; surface it rather than a generic failure. |

## Security

The monitor renders log content, making it a surface where a redaction failure
becomes visible rather than merely present. It inherits the guarantee in
[`docs/LOGGING.md`](../../LOGGING.md) and `no_secrets_canary_test`: message
bodies, addresses, credentials, and tokens must never reach a log line at all.

The monitor deliberately adds **no redaction of its own**, because a second
redaction layer would mask exactly the canary regressions worth catching. A
secret appearing in the monitor is a real engine bug and should look like one.

## Testing

All logic lives outside `ui`, because `egui` is impractical to assert against.
If something needs a test, it does not belong in the render layer. The crate is
a workspace member, so `-D warnings` and the `unwrap_used` / `expect_used` /
`panic` denies apply, which matters more than usual since GUI code reaches for
`unwrap` constantly.

- **`LogTailer`** — fixture files: well-formed JSON, malformed lines mid-stream,
  non-JSON fallback, midnight rotation handoff. No daemon, no network.
- **`EngineController`** — liveness against temporary lock files in `tempfile`
  directories; the spawn path tested through an injected launcher rather than by
  starting a real `nunciod.exe`.
- **`StatusPoller`** — against a test-local gRPC server with `MockKeyring`,
  mirroring the seam `nuncio-cli`'s runner already uses.
- **`System.Shutdown`** — offline E2E asserting the daemon actually exits and
  that an unauthenticated call is rejected.
- **`account_id` migration** — backfill test following the existing SMTP-column
  precedent.
- **`System.GetHealth`** — per #296's own definition of done.

Live network calls remain forbidden throughout, per `docs/STORY-WORKFLOW.md`.

## Accepted costs

- **Dependency surface.** `eframe`, `egui`, and `tray-icon` enter every
  `cargo test-all` and every CI run, on a project that currently has no
  dependency-advisory scanning: no `cargo-audit`, no `cargo-deny`, no
  `deny.toml`, no supply-chain CI job, and CodeQL scanning JS and actions only
  (#250). The monitor roughly doubles the third-party surface such a scanner
  would watch, which strengthens the case for landing `cargo-deny` soon.
- **CI time.** GUI dependencies slow the shared build. If that bites, moving the
  crate to a nested workspace under `tools/` is a directory move plus a CI job.
- **Contract churn.** Consuming `nuncio.v1` before the M7 freeze means the
  monitor may need updating as WS-A reshapes it.

## Definition of done

Per `CLAUDE.md`: engine, proto, CLI command, and offline E2E all exist together,
with the local gate green. For the two engine changes that means proto plus
daemon plus a `nuncio-cli` command plus an offline E2E each. The monitor itself
is a GUI and has no CLI surface; its equivalent bar is that every component
outside `ui` is unit-tested and the crate passes the workspace gate.
