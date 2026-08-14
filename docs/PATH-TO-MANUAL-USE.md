# Path to manual use

> **Purpose:** the shortest honest route from where the engine is today to a
> daemon a human can drive by hand and test end to end. Written 2026-08-14,
> after the ADR 0002 sync model landed. Complements [`ROADMAP.md`](ROADMAP.md)
> (the full plan) and [`HANDOFF.md`](HANDOFF.md) (machine-to-machine state).

## Where we actually are

`dev` is green and carries the whole of [ADR 0002](adr/0002-multi-engine-sync-model.md):
message/placement identity split, four-rung IMAP change enumeration, three-state
verified mutations, per-account filter ownership, JMAP `Email/changes`.

**The engine is ahead of its contract.** That is the one fact that matters here.

```
$ grep -c 'rpc \(MoveMessage\|DeleteMessage\|FlagMessage\)' \
      crates/nuncio-proto/proto/nuncio/v1/nuncio.proto
0
```

The daemon can move, delete and flag mail — the outbox executes verified
mutations with three honest outcomes — but **nothing can ask it to**. By this
repo's own definition of done (*engine + proto + CLI command + offline E2E*),
the placement work is not finished: it has an engine and nothing else.

## What that means if you boot it today

Works: add an account, sync, list folders, list and read messages, search,
calendar and contacts sync, export, audit.

Does not: **any mutation**. You cannot move, delete or flag a message from the
CLI, because the RPC does not exist.

Two consequences of the same gap:

- **Filters cannot be switched on.** `filters_enabled` is per-account and
  **default off** (deliberately — one engine owns side-effecting actions, see
  ADR 0002 §2). There is no proto field and no CLI command, so the only way to
  enable it is editing the `accounts` row in SQLite by hand.
- **Placements are invisible.** `Message.folder_id` names the one occupancy the
  message was reached through. A client cannot see that a message sits in three
  folders, which is the thing the split exists to model.

### Before you run it

```bash
git checkout dev && git pull
rm -f ~/.nuncio/nuncio_main.db*     # see below
cargo run -p nunciod                 # gRPC on 127.0.0.1:9420
cargo run -p nuncio-cli -- system status
```

Schema migrations were removed: databases are **created, never migrated**. A
store from before that change errors on read rather than upgrading. Deleting it
is correct — accounts and credentials live in the OS keyring and survive; cached
mail re-syncs. Expect this again whenever `IDENTITY_SCHEMA_VERSION` bumps.

## The critical path

Three steps, in order. Everything else in the roadmap is behind them.

### 1. #407 — cross-engine identity *(in flight)*

IMAP requests `EMAILID` (RFC 8474, where the server advertises `OBJECTID`) and
`X-GM-MSGID` (where it advertises `X-GM-EXT-1`), so it reaches identity tier 1
instead of only tiers 3–4. Until this lands, an IMAP engine and a JMAP engine
mint **disjoint key spaces** for the same mailbox and ADR 0002's convergence
claim is not true across protocols.

Bumps `IDENTITY_SCHEMA_VERSION` (2 → 3): the store rebuilds and re-syncs rather
than re-keying every row incrementally through the placement repoint arm.

### 2. #395 — the keystone

The single thing standing between "engine works" and "a human can drive it":

- **Mutation RPCs on `Mail`** — Move / Delete / Flag / Unflag, addressed by
  message key **plus the placement to act on**, since a message can occupy
  several mailboxes and `MOVE`/`DELETE` against the wrong one is destructive.
- **`Conflict` as a typed error**, not a response field, so clients that predate
  it take the error path instead of silently misreading success. Conflicts are
  durable and queryable (ADR 0002 §5), so this needs list/resolve RPCs too.
- **`Message` carries its placement set** — the field the split has been waiting
  for. Today's `folder_id` becomes one view of it.
- **`filters_enabled` on the wire** so filter ownership is settable without
  hand-editing SQLite.
- DAV conditional write-back.

This takes the repo's one proto-touching-story slot (currently free) and gates
the M7 API freeze.

### 3. Manual-readiness pass

Drive every RPC from `nuncio-cli` against a live daemon, fix what is missing or
broken, and write the exercise guide. The open WS-F CLI issues (#327, #328,
#329, #332) largely fall out of this.

## Scope, honestly

**81 open issues** as of 2026-08-14 — that is the roadmap through M7, not a task
list:

| open | milestone |
| ---: | --- |
| 15 | WS-B Mail model & mutations |
| 12 | WS-E Calendar/Contacts write & Search |
| 10 | WS-F Ops, usability & maintainability |
|  9 | WS-C Sync, push & lifecycle |
|  8 | WS-D Security to 9/10 · M6.5 de-fabrication · (no milestone) |
|  5 | M7 API freeze |
|  3 | M6 Security hardening · WS-A Contract hardening |

The three steps above are what buy manual usability. The rest is the product.

## Known-open, deliberately

Recorded so nobody re-derives them:

- **`recovery::tests::test_stage_2_backup_creation_and_stage_3_table_salvage`**
  failed once in CI and has never reproduced (0/40 locally under artificial CPU
  load, quiet across every run since). Genuinely undiagnosed — not fixed, not
  understood. If it recurs, get a reproduction before attempting a fix.
- **#339** — `parse_email_get_response` keeps a fixed-folder signature. The
  *production* path was corrected while implementing JMAP `Email/changes`
  (removals key on folder, so the hardcoded `"inbox"` made every JMAP removal
  inert); the public helper still reads as the issue describes.
- **`export_mbox` no longer emits `X-Nuncio-Folder`**, permanently. A message can
  hold several placements; one header would name one arbitrarily and MBOX cannot
  express the set.
- **A filter fire claim outlives its message.** `filter_fired` is never reaped, so
  a deleted-and-re-arriving message never re-fires. Deliberate: reaping would
  silently re-enable duplicate `FORWARD` / `CALL WEBHOOK`.
- **`db.rs` has one pre-existing `set_default`** log-capture test. Everything else
  uses the process-global subscriber. Harmless today, worth folding into a
  cleanup — see below.

## Two traps that cost real time here

Both are written up because they are invisible locally and expensive to
rediscover.

1. **`tracing` caches callsite `Interest` process-globally; `set_default` is
   per-thread.** A callsite first executed by a thread with no subscriber caches
   `never` and is skipped forever, before any thread-local subscriber is
   consulted. Reproduces at `--test-threads=2` (CI's core count), invisible at
   higher parallelism. Every log-capture helper must install **one global**
   subscriber and route per thread. Never reintroduce `set_default` /
   `with_default`.
2. **`tokio::time::pause()` cannot wrap real blocking I/O.** Auto-advance fires
   whenever the runtime *looks* idle — including while a `sqlx` query sits on the
   blocking pool — and it also trips sqlx's pool-acquire timer (default 30s,
   frequently the same duration as the timeout under test), turning a healthy
   query into `PoolTimedOut` and the result into misleading zeros. Make timeouts
   injectable and wait them out in real time instead.

## Picking this up cold

1. Read [ADR 0002](adr/0002-multi-engine-sync-model.md) — it is the design, and
   its **Pre-release posture** section is load-bearing: nothing is released,
   stores are disposable, behaviour may change without a deprecation path.
2. `cargo fmt --all -- --check`, `cargo check-all`, `cargo test-all` — the gate.
   Also run `RUST_TEST_THREADS=2 cargo test-all` before trusting a green run;
   default parallelism hides the flake class above.
3. `gh pr list` — PRs stack (`A → B → C`), and merging into `dev` does **not**
   auto-close issues, because GitHub only honours `Closes #N` on merge into the
   default branch. Close them by hand.
