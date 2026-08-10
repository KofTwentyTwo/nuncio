# Handoff — `story/monitor-and-health`

Written 2026-08-08. Branch is complete, reviewed, and pushed; it has **not**
been merged and its PR has **not** been opened.

This document exists because the working notes that produced this branch lived
in a git-ignored scratch directory (`.superpowers/sdd/`) which does not travel
between machines. Everything below is the durable part of those notes.

## State

| | |
| :--- | :--- |
| Branch | `story/monitor-and-health`, 26 commits off `dev` (base `a997220`) |
| Gate | green: `fmt` clean, `check-all` clean, `test-all` **664 passed / 0 failed / 40 suites** (baseline before the branch was 596) |
| Merged | no |
| PR | not opened |
| Closes | **#296** (WS-C8 `System.GetHealth`) via `Closes #296` in `fcc12ce` |

Two documentation commits also sit on `dev` ahead of this branch's base:
`1e5f19c` (design spec) and `a997220` (implementation plan).

## What it contains

1. **A trustworthy gate.** Two racy log-assertion tests moved to `tracing-test`.
   `tracing` caches callsite `Interest` globally, so a test installing a
   capturing subscriber only sees callsites first evaluated while it was
   active — they passed locally and failed on CI depending on thread
   scheduling. Scoped to the two tests in files this branch edits; issue #375
   owns the wider cleanup.
2. **Outbox account attribution.** `pending_remote_mutations` gained
   `account_id` with an additive migration, a backfill joining
   `message_id → messages.account_id`, and the write path
   (`PendingRemoteMutation`, `OutboxManager::create_mutation`,
   `save_pending_mutation`, `list_pending_mutations`). Migrating alone was not
   enough: without the write path every *new* mutation would have written
   `NULL` and per-account counts would have worked once, then silently stopped.
3. **`System.Shutdown`.** Windows has no `SIGTERM`, so there was no way to stop
   the daemon gracefully from another process. Authenticated RPC triggering the
   existing `lifecycle.rs` path, plus `nuncio system shutdown` and an offline
   E2E asserting the daemon actually *exits* (not merely that the RPC returned
   `Ok` — it returns as soon as shutdown is *requested*).
4. **`System.GetHealth`.** Per-account outbox `pending`/`failed`, WAL size,
   `db_healthy`. Plus `nuncio system health` and an offline E2E. Closes #296.
5. **`crates/nuncio-monitor`.** A Windows tray dev instrument: `log_tail.rs`,
   `engine.rs`, `status.rs`, `state.rs`, `fmt.rs`, `ui.rs`, `tray.rs`,
   `main.rs`. First GUI crate in a previously headless workspace.

## Running it

```
cargo build -p nuncio-monitor -p nunciod
target\debug\nuncio-monitor.exe
```

`nunciod.exe` must sit beside the monitor binary — `daemon_exe_path()` resolves
it as a sibling of `current_exe()`. The monitor sets `NUNCIO_LOG_FORMAT=json`
on the daemon it spawns, which is what gives the log pane structured lines; a
daemon started by other means logs human-readable text and the viewer degrades
to opaque lines with filtering unavailable.

**A running `nunciod.exe` or `nuncio-monitor.exe` holds its own binary on
Windows**, so a build will fail with `Access is denied. (os error 5)` until it
is stopped. This bites repeatedly; stop them before building.

## Manual verification — NOT COMPLETE

The GUI was never signed off. Two bugs were found by looking at it and fixed
(`9e70fbb`), but nobody has confirmed the fixes visually. Outstanding:

1. **Tray menu responsiveness.** The highest-value unknown. Nothing automated
   covers whether `egui`'s `request_repaint()` genuinely forwards through the
   `EventLoopProxy` that `tray-icon`'s docs require. If it does not, menu
   clicks land only when the loop happens to wake for a redraw — the menu feels
   *intermittently* unresponsive rather than broken, which is far harder to
   diagnose later. Right-click the tray repeatedly, including after idling.
2. **Red egui overlays gone**, and the Accounts and Logs tables scroll
   independently. Both symptoms of the same id collision fixed in `9e70fbb`.
3. **`STREAM STALE` clears within seconds** of clicking Start Engine, not ~30s.
4. **Em dash renders as a dash, not tofu.** `fmt::UNKNOWN` is U+2014. If egui's
   bundled font boxes it, the "unknown" signal is worse than useless.
5. **Tray colours distinguishable at 16px**, especially yellow `(230,200,40)`
   on a light taskbar. The unit test only proves the colours differ from each
   other.
6. **Start Engine with `nunciod.exe` absent** should show a red
   `monitor error:` line. Before the final fix wave it showed nothing at all.

Left-clicking the tray icon does nothing by design — only `MenuEvent` is
forwarded, not `TrayIconEvent`. Confirm that is wanted.

## Design decisions worth not relitigating

- **The monitor is a dev instrument, not a product client.** The native
  macOS/Windows GUIs remain separate repos per
  [`adr/0001`](../adr/0001-engine-first-grpc-architecture.md). It lives in the
  workspace so the gate covers it; a tool outside the gate rots.
- **It reads the log file and the lock file directly.** Sanctioned: logs are
  not in the contract and a log-streaming RPC is a far larger design, and a
  stopped daemon cannot answer an RPC about itself. It reaches around gRPC for
  nothing else and never touches SQLite.
- **`stop()` has no force-kill fallback.** It calls `Shutdown`, polls to a 15s
  deadline, and errors. A kill fallback would silently reintroduce the
  ungraceful exit `lifecycle.rs` exists to eliminate — on a mail engine
  mid-sync that risks a torn outbox.
- **The monitor adds no redaction.** Secrets are excluded at the source by
  `Redacted<T>` plus `no_secrets_canary_test`. A second masking layer here
  would hide exactly the regressions that canary exists to catch. A secret
  visible in the monitor is an engine bug and should look like one.
- **Engine state is composed, not read raw.** `EngineController::liveness()`
  knows only what the advisory lock knows and can never yield `NotResponding`
  for a daemon that is up but not answering. `status.rs` combines it with RPC
  reachability; `state.rs` and `ui.rs` render that composite. Reading
  `liveness()` from the UI would make a wedged daemon display as cleanly
  `Running`.
- **Unknown must never render as zero.** Single choke point,
  `fmt::u64_or_unknown`. See the known gap below.

## Known gaps, carried deliberately

**1. The status header can show `outbox: 0` beside a table showing `—`.**
`GetStatus` soft-degrades `unread_count`, `outbox_depth`, and `accounts_loaded`
to `0` on a query error; `GetHealth` fails closed. So the same window can show
"unknown" and "verified empty" for the same underlying failure.
`docs/RUNNING.md` documents this explicitly rather than claiming otherwise.

The monitor **cannot** fix this itself — on the wire a degraded `0` and a real
`0` are byte-identical `uint64`s. The right fix is additive: make those fields
`optional uint64` (proto3 field presence) so the daemon can express absence and
old clients keep reading `0`. **Do not make `GetStatus` fail-closed** — that
would destroy `db_healthy`/`ready` reporting precisely when the daemon is
degraded, which is when `nuncio system status` matters most.

**2. `EngineState::Unknown` is absorbing, not transient.** If the monitor's
background thread dies, the only writer of engine state is a channel that never
opens, so it stays `Unknown` forever. `liveness()` is a synchronous local
lock-file probe touching no network or keyring, and `MonitorApp` already holds
an `Arc<EngineController>` — re-probing on the GUI thread would let it recover.
Also, disabling **Start** in `Unknown` is over-conservative: `nunciod`'s own
single-instance lock already prevents the race the code comment cites.
Disabling **Stop** is correct, since `stop()` builds its own runtime and the
background thread just failed to.

**3. No dependency-advisory scanning, now over a much larger surface.** This
branch added ~281 transitive packages (the first GUI tree in a headless
workspace). There is no `cargo-audit`, no `cargo-deny`, no `deny.toml`, and no
supply-chain CI job; CodeQL scans JS and actions only (#250). Zero open
security alerts therefore means *nothing was scanned*. Landing `cargo-deny` is
materially more urgent than it was before this branch.

**4. Three path conventions are hand-mirrored from `nunciod` with nothing
pinning them.** `default_db_path`, `log_dir_for` (`main.rs`) and
`lock_path_for` (`engine.rs`). All three currently agree with the daemon's, and
`nunciod` is already a dev-dependency used for the real `InstanceLock` in
tests — so an assertion that they agree is a few lines. Drift would silently
make the monitor watch the wrong lock file and log directory, reporting a
confident "Stopped" for a running daemon.

## CI baseline — read before judging a red PR

**`dev`'s CI was already red before this branch existed**, from flaky tests with
three distinct root causes. This branch fixed one of them (the tracing-callsite
race) for the two tests in files it touched. Still failing intermittently and
**out of scope here**:

- `a_timed_out_item_does_not_block_a_later_item_in_a_subsequent_pass` —
  timing-sensitive, `crates/nunciod/tests/outbox_executor_e2e_test.rs`
- `mail_and_folder_report_honest_errors_when_daemon_unreachable` — port-reuse
  race, issue **#347**
- the remaining hand-rolled tracing captures — issue **#375**

If this branch's CI comes back red, check it against that baseline rather than
assuming it is this work.

## Other pre-existing findings surfaced along the way

Recorded here because they were found while working and would otherwise be
lost; none belong to this branch.

- `nuncio-cli` mints a gRPC bearer token as a side effect of any command that
  dials the daemon (`get_or_create_key_bytes` in `runner.rs` and siblings).
  Harmless in practice, but a *client* creating key material rather than
  reporting that the daemon has never run is backwards.
- `ratatui` and `crossterm` are declared in `[workspace.dependencies]` but used
  by no workspace crate — leftovers from the TUI's move to `_reference/`.
- **#370** is real: `evaluate_with_timeout` has no production caller, so the
  live sync path still calls the unbounded `evaluate`. A pathological
  user-authored filter regex can hang sync.
- `get_status` has the same soft-degrade-to-zero pattern as the `GetHealth` bug
  fixed here, on its own metrics. Worth resolving before the M7 API freeze
  rather than shipping it into a frozen contract.
