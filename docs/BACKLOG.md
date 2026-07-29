# Backlog — Phase 0 & Phase 1 (engineering-ready)

> **Status:** Phase 0 and Phase 1 are both complete and on `dev` — this document
> is now the historical story-level record of that work. For where the project
> stands today (Phase 3 in progress, milestones M1–M7), see
> [`ROADMAP.md`](ROADMAP.md).

Ordered, executable stories for the first two phases of [`ROADMAP.md`](ROADMAP.md).
Each story has acceptance criteria (AC) and a rough size (S ≤1d, M ≈2–4d, L ≈1wk).
Do them roughly top-to-bottom; within an epic, stories are mostly independent
unless noted. **Definition of done for every story:** engine + (where applicable)
proto + CLI + offline test, and `cargo verify` green.

---

## Phase 0 — Honesty & foundation

### Epic 0.A — Documentation reset
- **0.A.1 Archive fabricated docs** (S). Move the superseded `PLAN-*`, `TODO.md`,
  `TICKET-*`, and `EXECUTIVE-*` files into `docs/_archive/` with a README stating
  they are historical and unverified. **AC:** `docs/` top level contains only
  `ROADMAP.md`, `BACKLOG.md`, `adr/`, a rewritten `ARCHITECTURE.md`, and `_archive/`.
- **0.A.2 Rewrite `CLAUDE.md` to reality** (M). Correct crate count (12), define
  the cargo aliases (`check-all`, `test-all`, `cov`, `verify`), document the
  gRPC/engine-first direction, and replace the "100% four-shell parity" mandate
  with "parity via the published contract." **AC:** every command and claim in
  `CLAUDE.md` is locally verifiable.
- **0.A.3 Rewrite `README.md`** (S). Remove fabricated claims and benchmarks;
  state honest pre-alpha status. **AC:** no unverifiable claim remains.

### Epic 0.B — Release hygiene
- **0.B.1 Mark public releases pre-alpha** (S). Convert v1.0.0 and rolling
  releases to prerelease with a NOTICE that they are non-functional and the
  updater is unsafe. **AC:** no GitHub release is presented as GA.
- **0.B.2 Disable the fail-open auto-updater** (S). Guard the 24h update loop off
  until Phase 4. **AC:** the daemon never downloads/replaces a binary.

### Epic 0.C — Green CI
- **0.C.1 Clear the clippy backlog** (M). Commit or revert the ~1,600 lines of
  abandoned edits so `cargo check-all` passes (76 current denials in
  `nuncio-core`). **AC:** `cargo check-all` exits 0 locally and in CI.
- **0.C.2 Align CI to the local gate** (S). CI runs `cargo clippy --workspace`
  without `--all-targets`; make CI run `cargo check-all` so CI and local are
  identical. **AC:** CI and `cargo verify` enforce the same lints.
- **0.C.3 Replace the coverage gate** (M). Drop the gamed 100%-line gate (it
  excludes all `main.rs` and incentivized stub+matching-test theater) for a
  realistic threshold on engine crates plus a **required** mock-server E2E job.
  **AC:** coverage gate reflects real confidence; full CI green on `dev`/`main`.

### Epic 0.D — Shrink to engine + CLI
- **0.D.1 Extract the API-surface draft** (S, do before 0.D.2). Mine the MCP
  shell's ~20 tool→engine mappings into `docs/api-surface-draft.md` as candidate
  RPCs. **AC:** every operation the MCP shell exposed is listed as a candidate RPC.
- **0.D.2 Move UIs to `_reference/`** (M). Remove `nuncio-tui`, `nuncio-gui`
  (+ `src-tauri`, `ui`), and `nuncio-mcp` from the workspace members into
  `_reference/`. **AC:** workspace = `core, mail, cal, contacts, store, filter,
  cli, nunciod`; `cargo build`/CI green.

### Epic 0.E — Kill hardcoded keys / real vault
- **0.E.1 Purge key literals** (M). Remove `DEFAULT_STORAGE_KEY`,
  `DEFAULT_WORM_KEY`, `'secret_ledger_key'`, and `b'nuncio_ledger_secret'`; fail
  closed when no key is present. **AC:** `grep` finds no crypto key literal in
  `src/`; missing-key paths error rather than silently use a default.
- **0.E.2 Real OS keyring vault** (L). Replace `MockKeyring` in production with the
  `keyring` crate; keys minted and read from the OS keyring; `MockKeyring` remains
  test-only. **AC:** an integration test (with a mock keyring provider) proves the
  daemon reads keys from the vault, not from source.

---

## Phase 1 — gRPC skeleton + the spine

### Epic 1.A — gRPC transport
- **1.A.1 Proto skeleton + build** (M). Create `proto/nuncio/v1` with a `Status`
  RPC; wire tonic/prost codegen. **AC:** proto compiles; Rust server+client stubs
  generate in the build.
- **1.A.2 Authenticated daemon server** (M). Daemon serves gRPC over loopback;
  bearer token from the keyring; unauthenticated calls rejected. **AC:** an
  unauthenticated `Status` call is refused; an authenticated one returns real
  daemon state.
- **1.A.3 CLI becomes a gRPC client** (M). Rewrite `nuncio status` as a tonic
  client; fence/retire the legacy JSON-RPC IPC. **AC:** `nuncio status`
  round-trips daemon↔CLI over gRPC in an E2E test.
- **1.A.4 Events stream skeleton** (M). `Subscribe` server-streaming RPC emitting
  `CoreEvent`s. **AC:** the CLI can subscribe and receive a test event; no
  response/notification interleaving bug (regression test).

### Epic 1.B — Store correctness (prerequisite for real data)
- **1.B.1 FTS-vs-encryption strategy** (L). Decide and implement: index plaintext
  before encryption, or drop body encryption for searchable fields — document the
  choice. **AC:** body search returns real hits on the production store path
  (not via raw-SQL plaintext injection); E2E proves it.
- **1.B.2 FTS backfill at migration** (M). Backfill the FTS index so messages
  saved before first search are searchable. **AC:** a message saved before any
  search call is found by a later search.
- **1.B.3 Fix salvage schema mismatch** (M). Recovery restores filter rules against
  the real schema. **AC:** a recovery test restores a nonzero rule count.
- **1.B.4 Transient error ≠ corruption** (M). Distinguish `SQLITE_BUSY`/pool
  timeouts from genuine corruption; never salvage/delete the live DB on a
  transient error. **AC:** an injected transient error does not trigger
  salvage/deletion; a genuinely corrupt DB still does.

### Epic 1.C — The spine (one account, real mail)
- **1.C.1 Persistent DB path** (M). Daemon opens a real store (e.g.
  `~/.nuncio/…`); remove `HeadlessRunner::ephemeral()` from the CLI/production
  path. **AC:** `account add` → restart daemon → `account list` shows the account.
- **1.C.2 Account setup + credentials** (M). Collect IMAP/SMTP config; store the
  password in the keyring only. **AC:** account config persists; the password is
  never written to SQLite or source; only a keyring reference is stored.
- **1.C.3 Real IMAP fetch → store loop** (L). Replace the status-flag "sync" with a
  real fetch loop in the daemon using `ImapEngine`. **AC:** against a mock IMAP
  server, messages land in the store and sync status reflects reality (no stuck
  `Syncing`).
- **1.C.4 Read path over gRPC** (M). List folders/messages, read a body, mark
  read — via CLI over gRPC. **AC:** offline E2E does list → read → mark-read
  against the mock server.
- **1.C.5 Real SMTP send via outbox** (L). Delete the "Simulate remote IMAP/JMAP
  mutation execution" worker; real send with retry/backoff. **AC:** against a mock
  SMTP server a message actually sends; a failure retries; there is no fabricated
  "Message sent" when it did not.
- **1.C.6 Mock protocol server harness** (M, enables 1.C.3–1.C.5 E2E). Stand up
  offline mock IMAP/SMTP (and wiremock scaffolding for later JMAP/CalDAV). **AC:**
  the full `fetch → store → read → send` E2E runs offline in CI.

**Phase 1 exit:** a real account syncs and sends end-to-end through the CLI over
gRPC, persisted across restarts, tested entirely offline, with CI green.
