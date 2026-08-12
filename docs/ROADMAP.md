# Nuncio Roadmap (authoritative)

> **Purpose of this document:** the single source of truth for where Nuncio is
> and the path from here to a **frozen, client-ready backend** — a solid engine,
> a reference CLI, and a published, versioned gRPC API that front-end repos
> (TUI, native macOS/Windows, MCP bridge) can begin consuming with
> contract-guaranteed parity.
>
> It supersedes the Antigravity-era `PLAN-*`, `TODO.md`, `TICKET-*`,
> `EXECUTIVE-*` documents (systematic fabrication — removed 2026-07-26, in git
> history only; **do not trust them**). Ground truth for the starting state: the
> 2026-07-26 forensic assessment. Governing decision:
> [`docs/adr/0001-engine-first-grpc-architecture.md`](adr/0001-engine-first-grpc-architecture.md).
> Phase 0/1 story-level detail lives in [`docs/BACKLOG.md`](BACKLOG.md).

## North star

Nuncio is a **local-first (sovereign) mail, calendar, and contacts engine**: a
background daemon (`nunciod`) that owns all state, credentials, and protocol
logic, and **publishes a versioned gRPC API over loopback**. User interfaces are
**separate native client projects** that consume that API — they hold no
business logic and no data of their own. Feature parity across clients is a
property of the **contract**, not of anyone's discipline.

## Definition of "done" (anti-theater guardrails)

These are non-negotiable and apply to every item below:

1. **A capability is done only when engine + proto + CLI command + offline E2E
   test all exist together**, and the local gate (`cargo fmt --all -- --check`,
   `cargo check-all` (clippy `-D warnings` all-targets), `cargo test-all`) is
   green.
2. **No fabricated success.** A path that cannot do the real thing returns an
   honest `Unimplemented`/error — never canned data, never a fake "sent".
3. **The `.proto` files are the contract.** The `nuncio-proto` contract-stability
   test fails on any accidental wire change; breaking changes go to a new
   package version, never a silent mutation of `nuncio.v1`.
4. **All external protocols are mocked in tests** (`wiremock` / `Mock*Backend`);
   live network calls in the test suite are forbidden.
5. **"Done" is re-derived from tested behavior**, never inherited from a label.

---

## Where we are today (2026-08-07)

**Phases 0–2 are complete and on `dev`.** The daemon boots gRPC-only on loopback
`127.0.0.1:9420` behind a keyring-minted bearer token, and serves **eight
services, each mounted behind the auth interceptor**: `System`, `Accounts`,
`Mail`, `Filters`, `Calendar`, `Contacts`, `Export`, `Audit` (the `Calendar`
and `Contacts` verticals landed in M1/M2). The IMAP→store→read→SMTP-send spine works
end-to-end over gRPC, driven by the `nuncio-cli` reference client, tested
offline. The legacy JSON-RPC IPC is removed. The `nuncio.v1` contract is
published with a byte-deterministic `FileDescriptorSet` golden guarding it, plus
external-codegen docs for Swift/C#/TypeScript.

**The feature surface — Phase 3 through M5 — is now COMPLETE**, via milestones
M1–M5, all merged to `dev` (M1–M4 are already promoted to `main`; M5 promotes
next):

- **3.A Filter correctness** — **done** (account scoping, `ON ACCOUNT`, non-ASCII
  offsets, `NOT IN`).
- **3.B Calendar** — **done** (M1). Real CalDAV `REPORT` transport, `TZID` fix,
  `CalendarBackend` trait, `calendar_events` store CRUD, the `Calendar` gRPC
  service + CLI commands, and RRULE recurrence wired into the event-query path.
- **3.C Contacts (CardDAV)** — **done** (M2). A real, wiremock-tested CardDAV
  client replaced the fabricated one; contacts persist through the daemon's
  store and are exposed over a `Contacts` gRPC service + CLI.
- **3.D JMAP** — **done** (M3). A real JMAP HTTP client is wired into the same
  `MailBackend` fetch→store path as IMAP.
- **Filters made live** — **done** (M4). `FilterEngine::evaluate` is wired into
  the live sync path with a fire-once-per-new-message guard, the `account`
  condition-field bug is fixed, unsupported `header[...]` conditions are
  honestly rejected, filter edit/export/import/logs moved onto the `Filters`
  gRPC service, and a `Filters.Triage` server-streaming RPC does retroactive
  bulk rescan.
- **Mail lifecycle & incremental sync** — **done** (M5). Account lifecycle
  (`UpdateAccount`/`RemoveAccount`/`TestAccountConnection`) is real over gRPC,
  with `imap_tls_mode`/`smtp_tls_mode` persisted and actually driving the real
  IMAP (implicit/STARTTLS/plain) and SMTP connection. Sync is genuinely
  incremental: a real per-folder `{uidvalidity}:{uidnext}` checkpoint narrows
  the FETCH instead of re-fetching everything, a UIDVALIDITY change forces a
  safe full re-fetch (no silent mail loss), a per-item FETCH timeout replaces
  the indefinite hang, and a `SyncProgress` event is emitted. `SendMessage`
  takes an explicit `account_id` (honest `AccountNotFound`, no silent
  wrong-account fallback), and `AddAccount` rolls back an orphaned keyring
  secret if persistence fails.

**M6 — Security & release hardening is nearly complete** (13 of 16 stories
closed). Landed: fail-closed encryption, zeroized key material, WORM/ledger
key-length validation, non-loopback gRPC bind rejection plus the auth-coverage
canary, the fail-closed updater on missing `SHA256SUMS`, and real CIDR/DNS SSRF
checks on webhook egress. Still open: inbound HTML email sanitization (#204),
Rust static analysis in CodeQL (#250 — CodeQL currently scans JS and actions
only), and removal of assertion-free test theater (#251).

**Work reorganized after M5 into three tracks that now carry the bulk of the
remaining effort.** These are GitHub milestones in their own right; they are not
part of the original M1–M7 numbering:

- **M6.5 — Feature completeness & de-fabrication** (3 closed / 8 open).
  Eliminates the remaining fabricated or silently-wrong paths: production-wiring
  Contacts sync with per-account CardDAV config (#255), namespace-aware DAV XML
  parsing to replace substring splitting and silent drops (#257), the
  `tls_mode_from_db` silent `ImplicitTls` fallback (#271), the
  `NaturalLanguageScheduler` that returns wrong times (#258), plus CLI and audit
  cleanups.
- **WS-A … WS-F — the pre-freeze reshape** (22 closed / 56 open). Six tracks
  that must land before `nuncio.v1` can be frozen: **WS-A** contract hardening
  (typed errors, `Timestamp`/`Duration` unification, keyset pagination, id
  conventions, enum hygiene, account transport `oneof`); **WS-B** mail model and
  mutations; **WS-C** sync, push and lifecycle; **WS-D** security to 9/10
  (whole-DB SQLCipher encryption, OS-permissioned UDS / named-pipe IPC replacing
  loopback TCP, signed updates, HTML sanitization); **WS-E** calendar and
  contacts write-back plus structured search; **WS-F** ops, usability and
  maintainability, including the behavior-preserving `grpc.rs` (#333) and
  `DatabaseEngine` (#334) decompositions.
- **OBS-1 … OBS-9 — observability — COMPLETE.** All nine stories merged
  (#357–#365; no milestone was assigned). Delivered: a layered `tracing`
  subscriber with `EnvFilter`, rotating file and JSON output; per-RPC spans with
  request-id correlation and auth accept/reject logging; domain and lifecycle
  coverage across handlers, syncs, outbox and shutdown; protocol-sync
  instrumentation for mail, calendar and contacts; a real `System.GetStatus`
  health and readiness surface; CLI `-v`/`-vv` with typed `ErrorInfo` rendering;
  and a redaction policy with a `Redacted<T>` newtype plus a no-secrets-in-logs
  canary. Reference: [`docs/LOGGING.md`](LOGGING.md).

**M7 (API freeze) has not started** — 5 stories open, 0 closed. It gates on WS-A
finishing the contract reshape.

A handful of follow-ups were filed during M1–M5 and remain open, tracked as
their own backlog items rather than blocking completion: calendar
windowing/predicate refinement (#216), contacts notes handling (#219), a
`filter edit` bug where re-saving a rule silently re-enables it and resets
`created_at` (#225), and further mail sync/SMTP robustness polish identified
during M5 (#233). Also open and worth early attention: the ReDoS guard
`evaluate_with_timeout` exists but no production caller uses it, so the live
sync path still calls the unbounded `evaluate` (#370).

**Explicitly deferred** (out of scope until the backend is client-ready and each
is re-justified as a genuine engine capability): OpenPGP/S-MIME E2EE (#79),
WASM/QuickJS plugins (#80), local-LLM summarization (#81).

---

## The path to client-ready

The original plan was seven milestones: M1–M5 complete the **feature surface**;
M6 hardens **security**; M7 is the **API-freeze gate** that declares the backend
ready for front-end repos. Milestones are ordered but M1–M4 are largely
independent (they touch different engine crates); the only hard serialization is
the shared `nuncio.proto` (and its golden), so proto additions land one at a
time.

That structure still holds, with one amendment made after M5: the gap between
"the feature surface exists" and "the contract can be frozen" turned out to be
larger than M6 alone, so **M6.5**, the **WS-A … WS-F** workstreams, and the
**OBS** observability epic were opened between M6 and M7. They are described
under [M6.5, WS and OBS](#m65-ws-and-obs--the-work-between-m6-and-m7) below.

### M1 — Finish Calendar (closes #178)
Complete the Calendar vertical behind the API.
- Proto `Calendar` service (`Sync`/`ListEvents`/`GetEvent` — honest surface only,
  no fabricated create/delete), mounted behind the bearer-auth interceptor;
  regenerate the contract golden.
- Daemon `CalendarGrpcService` + injection seam + `calendar_sync` module; wire
  the (currently orphaned) `nuncio-cal` crate into `nunciod`.
- **Wire `RecurrenceEngine` (rrule) into the fetch/store/query path** so recurring
  events actually materialize (currently dead code).
- CLI `calendar` commands replacing the hardcoded stub; offline `sync → list →
  get` E2E incl. an unauthenticated-rejection assertion.
- **Exit:** create a rule-free Calendar view over gRPC end-to-end, offline-tested,
  recurrence expanded.

### M2 — Contacts / CardDAV (closes #179)
Bring Contacts to the same bar as Calendar.
- Replace the fake `CardDavClient` (returns a hardcoded contact) with a real
  `wiremock`-tested CardDAV client; vCard parse → model → regenerate round-trip.
- Persist contacts through the daemon's store (today the CLI writes a fresh
  in-memory DB per invocation — nothing persists); add a `Contacts` proto service
  + daemon wiring + CLI + offline `sync → list` E2E.
- **Exit:** contacts sync and persist across restarts, driven over gRPC.

### M3 — JMAP (closes #180)
Make JMAP a real second mail backend alongside IMAP/SMTP.
- Real JMAP HTTP client (session, `Email/query`/`get`/`set`), camelCase wire
  fix, wired into the same `MailBackend` fetch→store path; `wiremock`-tested.
- **Exit:** a JMAP account fetches → stores → lists offline, via the existing
  Mail service.

### M4 — Filters made live + complete (closes #175, #176, #181)
Today rules can be created, validated, and previewed as "MATCH" but **never fire**.
- **Wire `FilterEngine::evaluate` into the real sync/orchestrator path** so a
  synced message that matches a rule actually triggers its action (move/flag/
  delete/forward/webhook — the outbox is built but never invoked end-to-end).
- Fix `FilterField::Header` (silently hardcoded `false`) and the `eval_leaf`
  `account`-field bug (#181, reads `folder_id`).
- Re-expose filter edit/export/import/logs (#175) and bulk triage (#176) over the
  `Filters` service (lost with the IPC).
- **Exit:** an offline E2E proves a matching rule fires an action against synced
  mail; no silent no-match, no fabricated preview.

### M5 — Mail spine completeness & correctness (closes #170, #169, #182, #183, #184)
Round out real account and sync management.
- Account lifecycle over gRPC: edit / delete / test-connection, with TLS-mode
  (implicit / STARTTLS / plain) actually persisted and wired (#182).
- `Mail.SendMessage` explicit account selection (#170); avoid orphaned keyring
  entries on partial account add (#169).
- **Incremental sync**: honor `since_state` so sync isn't a full historical
  re-fetch every time (#184); sync progress reporting + bounded per-item stall
  timeout (#183).
- **Exit:** full account lifecycle + incremental sync, offline-E2E-proven, no
  fabricated fallbacks in the IMAP path.

### M6 — Security & release hardening (Phase 4; closes #162, #163, #164, #165)
The engine becomes production-solid.
- **Fail-closed everywhere:** fix `encrypt_text_at_rest` silent empty-ciphertext
  data loss (#162); a fail-*closed*, signature-verifying updater (no apply when
  `SHA256SUMS` is absent).
- Key hygiene: zeroize engine key material on drop (#163); validate WORM/ledger
  key length from the vault (#164).
- Transport/egress: gRPC bind hardening + a guard that every service is wrapped
  in the interceptor (#165); real CIDR-based SSRF checks on webhook egress
  (replace the substring blocklist); sandboxed HTML email rendering pipeline for
  clients.
- **Exit:** a security review passes; no fail-open crypto, update, or egress
  path remains.

### M6.5, WS and OBS — the work between M6 and M7

Opened after M5, when the reshape needed before an honest `v1` freeze proved
larger than M6. Status as of 2026-08-12:

| Track | Closed / Open | Scope |
| :--- | :---: | :--- |
| **M6.5** Feature completeness & de-fabrication | 3 / 8 | Remove the remaining fabricated or silently-wrong paths (Contacts sync wiring #255, DAV XML namespaces #257, TLS-mode fallback #271, NL scheduler #258) |
| **WS-A** Contract hardening (pre-freeze reshape) | 6 / 3 | Typed errors, `Timestamp`/`Duration`, keyset pagination, id conventions, enum hygiene, account transport `oneof` — **gates M7** |
| **WS-B** Mail model & mutations | 1 / 14 | Message identity, drafts, folder management, attachments on receive |
| **WS-C** Sync, push & lifecycle | 4 / 9 | Autonomous scheduler, IMAP IDLE, resumable event stream, graceful shutdown |
| **WS-D** Security to 9/10 | 4 / 8 | Whole-DB SQLCipher (#299), UDS / named-pipe IPC (#305), signed updates (#300), HTML sanitization (#302) |
| **WS-E** Calendar/Contacts write & Search | 2 / 12 | DAV write-back with conditional PUT/DELETE, recurrence editing, structured Search service |
| **WS-F** Ops, usability & maintainability | 5 / 10 | CLI polish, `System.Backup`, and the `grpc.rs` (#333) / `DatabaseEngine` (#334) decompositions |
| **OBS-1 … OBS-9** Observability | 9 / 0 | **COMPLETE.** Tracing subscriber, RPC spans + request-id, domain/lifecycle coverage, protocol instrumentation, `GetStatus`, CLI verbosity, redaction canary |

WS-B and WS-C grew when **ADR 0002** decided
the convergent multi-engine sync model and broke it into #386–#397. Those are
in flight, not closed: the message/placement identity split, Message-ID and
content-hash capture, `user_version` gating, the raw IMAP command layer, the
four-rung change enumeration ladder, and three-state verified mutations are all
implemented and green on a branch, and the counts above move when it merges.
Still open behind them: the mutation RPCs and durable conflict surface (#395),
DAV href/ETag identity and `sync-collection` (#392, #393), the scheduler and
rate limiting (#396), and the ≥3-engine convergence harness (#397).

The OBS stories (#357–#365) were merged without a milestone assignment; the
epic is complete regardless, and `docs/LOGGING.md` is its reference.

### M7 — API freeze & client-readiness gate  ← **the goal line**
Declare the backend ready for front-end repos.
- **Freeze `nuncio.v1`:** every backend capability is represented; the
  contract-stability golden covers the full surface; the versioning policy
  (additive-only in v1) is enforced by test.
- Generated SDK / codegen docs verified for Swift, C#, and TypeScript; a minimal
  "hello, daemon" example per language.
- **The CLI reference client exercises every RPC** in offline E2E — the CLI is
  the proof the contract is complete and usable.
- Replace the gamed 100%-line coverage gate with a realistic engine-crate
  threshold + a required mock-server E2E job (#143); CI green on `dev`/`main`.
- Tag a pre-alpha backend release whose notes state exactly what is real.
- **Exit / definition of client-ready:** the daemon publishes a **frozen,
  documented, contract-tested gRPC API** that the CLI fully exercises offline.
  **At this point Phase 5 front-end repos may begin.**

### Phase 5 — Client projects (after M7)
Separate repositories consuming the frozen API — `nuncio-tui` (Rust),
`nuncio-gui-macos` (Swift + grpc-swift), `nuncio-gui-windows` (WinUI/C# +
Grpc.Net), `nuncio-mcp` (MCP↔gRPC bridge). Parity is guaranteed by the contract.
The deferred V2/V3 features (E2EE, plugins, local AI) are re-justified and
scheduled here or folded back into the engine as they earn their place.

---

## Tracking

Each milestone maps to GitHub issues (numbers above); M6.5 and WS-A … WS-F are
GitHub milestones too, so `gh issue list --milestone "<title>"` is the live
status for any track in the table above. Every capability is built
via one implementer + a spec/quality review gate + an offline E2E, committed only
when the local gate (fmt + clippy `-D warnings` + tests) is green and the proto
golden is consistent. Issues close only after the review step — never on a green
build alone.
