# Nuncio Roadmap (authoritative)

> **This document supersedes** `PLAN-nuncio-roadmap.md`, `PLAN-production-roadmap-100-plus.md`,
> `PLAN-enhanced-roadmap-v1-v2-v3.md`, `PLAN-multi-shell-parity-v1-v3.md`,
> `PLAN-testing-matrix-and-v3-release-policy.md`, `TODO.md`, `TICKET-*.md`,
> `EXECUTIVE-REVIEW.md`, and `EXECUTIVE-AUDIT-REVIEW.md`. Those were authored by the
> Antigravity agent and contained systematic fabrication (self-awarded scores,
> nonexistent benchmarks, "completed" epics that were never wired) and have been
> **removed** (2026-07-26); they remain in git history only. **Do not trust them.**
>
> Ground truth for the starting state: the 2026-07-26 forensic assessment
> (`knowledge/nuncio-poc-ground-truth-2026-07-26` in the second-brain vault).
> Architecture decision: [`docs/adr/0001-engine-first-grpc-architecture.md`](adr/0001-engine-first-grpc-architecture.md).

## North star

Nuncio is a **local-first (sovereign) mail, calendar, and contacts engine**: a
background daemon (`nunciod`) that owns all state, credentials, and protocol
logic, and **publishes a versioned gRPC API over loopback**. User interfaces are
**separate native client projects** that consume that API — they hold no business
logic and no data of their own.

This inverts the failure mode of the original POC, which built four UI shells on
top of a spine that did not exist, so each shell faked what the spine could not
provide and drifted into four disjoint fake data stores. With one engine and one
published contract, **feature parity across clients becomes a property of the
API, not a discipline anyone has to police.**

## Target architecture

- **`nunciod` is the product.** It owns the SQLite store, credentials (real OS
  keyring), the protocol sync loops (IMAP/SMTP first; then JMAP, CalDAV, CardDAV),
  the NSQL filter engine, and all business logic. The `nuncio-{core,mail,cal,
  contacts,store,filter}` crates are libraries the daemon composes.
- **The published API** is a set of versioned Protocol Buffers definitions
  (`proto/nuncio/v1/*.proto`) served by the daemon via **gRPC over loopback TCP
  (`127.0.0.1`)** with a **bearer-token handshake** — the token minted into the OS
  keyring on first run and sent as gRPC metadata. Unary RPCs for commands and
  queries; **server-streaming RPCs for push** (new mail, sync progress, events).
- **The `.proto` files are the contract** — versioned, in-repo, the single source
  of truth every client codegens against.
- **One reference client stays in-repo: `nuncio-cli`**, a tonic gRPC client. It
  dogfoods the API and drives the end-to-end tests.
- **Clients live in separate repos**, each consuming the published proto/SDK:
  `nuncio-tui` (Rust), `nuncio-gui-macos` (Swift + grpc-swift),
  `nuncio-gui-windows` (WinUI / C# + Grpc.Net), `nuncio-mcp` (MCP ↔ gRPC bridge).
  Linux is supported at the daemon + CLI level; a native Linux GUI is deferred.

## Principles (anti-theater guardrails)

1. **A capability is "done" only when engine + proto + CLI command + offline E2E
   test all exist together.** Never before.
2. **No fabricated success.** A code path that cannot do the real thing returns an
   honest `Unimplemented`/error — never a fake "Message sent" or canned data.
3. **Green CI is a gate, not a decoration.** `fmt`, `clippy` (all targets,
   warnings-as-errors), tests, and mock-server E2E must pass before merge.
4. **"Done" is re-derived from tested behavior**, never inherited from a doc label.
5. **Scope is earned.** The four never-real features (OpenPGP/S-MIME E2EE, WASM
   plugins, local-LLM summarization, NLP scheduler) are **deferred and must be
   re-justified as genuine engine capabilities** before re-entering scope.

## Phases

Phases 0–4 deliver "the engine is 100% solid and publishing its full API." Phase 5
is "clients consume it."

### Phase 0 — Honesty & foundation
**Goal:** stop lying, shrink to what matters, and get the tree green.
**Exit criteria:** workspace is engine crates + CLI only; `cargo fmt`,
`cargo check-all`, `cargo test-all`, and mock-server E2E all pass in CI; no crypto
key literal exists in source; credentials come from the real OS keyring; fabricated
docs archived; `CLAUDE.md` and `README.md` reflect verified reality; public
releases are marked pre-alpha and the fail-open auto-updater is disabled.

### Phase 1 — gRPC skeleton + the spine
**Goal:** a real account that fetches, stores, reads, and sends — end-to-end,
persisted, over gRPC, driven by the CLI.
**Exit criteria:** daemon serves gRPC over loopback with token auth; CLI is a
gRPC client; `IMAP fetch → store → read → SMTP send` works against mock servers
in an offline E2E test; the "simulate remote mutation" outbox worker and the
status-flag "sync" are gone; the store correctness bugs (FTS-vs-encryption, FTS
backfill, salvage schema, transient-error≠corruption) are fixed.

### Phase 2 — API completeness for the spine
**Goal:** the full proto surface for everything the spine can do, published.
**Exit criteria:** `proto/nuncio/v1` covers accounts, folders, mail
(sync/list/read/send/flags), search, filters, and the events stream; generated
SDK stubs published; contract tests in place; the legacy JSON-RPC IPC removed.

### Phase 3 — Feature completeness (behind the API)
**Goal:** calendar, contacts, filters, and JMAP made real.
**Exit criteria:** real CalDAV transport + TZID fix + rrule; real CardDAV sync;
filter-engine correctness fixes (account scoping, `ON ACCOUNT`, non-ASCII offsets,
`NOT IN`); real JMAP HTTP client + camelCase fix. Each ships only with engine +
proto + CLI + E2E together.

### Phase 4 — Security & release hardening
**Goal:** the engine is production-solid.
**Exit criteria:** fail-*closed* updater with signature verification; auth
hardening (optional UDS/named-pipe, mTLS); real WORM keys; HTML sanitization
pipeline; an honest release pipeline and version policy. **This is the "engine
100% solid + full API published" milestone.**

### Phase 5 — Client projects
**Goal:** the native clients, in separate repos, consuming the published API.
**Exit criteria:** `nuncio-tui`, `nuncio-gui-macos`, `nuncio-gui-windows`, and
`nuncio-mcp` exist as independent projects built against the published proto/SDK;
parity is guaranteed by the contract. Re-justified V2/V3 features are planned here
or folded back into Phase 3 as they earn their place.

## Detailed backlog

Phase 0 and Phase 1 are decomposed into engineering-ready stories in
[`docs/BACKLOG.md`](BACKLOG.md).
