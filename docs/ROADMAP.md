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
   test all exist together**, and `cargo verify` (fmt + clippy `-D warnings`
   all-targets + `test-all`) is green.
2. **No fabricated success.** A path that cannot do the real thing returns an
   honest `Unimplemented`/error — never canned data, never a fake "sent".
3. **The `.proto` files are the contract.** The `nuncio-proto` contract-stability
   test fails on any accidental wire change; breaking changes go to a new
   package version, never a silent mutation of `nuncio.v1`.
4. **All external protocols are mocked in tests** (`wiremock` / `Mock*Backend`);
   live network calls in the test suite are forbidden.
5. **"Done" is re-derived from tested behavior**, never inherited from a label.

---

## Where we are today (2026-07-28)

**Phases 0–2 are complete and on `dev`.** The daemon boots gRPC-only on loopback
`127.0.0.1:9420` behind a keyring-minted bearer token, and serves **six services,
each mounted behind the auth interceptor**: `System`, `Accounts`, `Mail`,
`Filters`, `Export`, `Audit`. The IMAP→store→read→SMTP-send spine works
end-to-end over gRPC, driven by the `nuncio-cli` reference client, tested
offline. The legacy JSON-RPC IPC is removed. The `nuncio.v1` contract is
published with a byte-deterministic `FileDescriptorSet` golden guarding it, plus
external-codegen docs for Swift/C#/TypeScript.

- **Phase 0 — Honesty & foundation:** done. Fabricated docs archived; workspace
  shrunk to engine crates + CLI (`_reference/` holds the old shells); real OS
  keyring; fail-open auto-updater disabled; releases marked pre-alpha.
- **Phase 1 — gRPC skeleton + the spine:** done. Authenticated loopback gRPC;
  CLI as gRPC client; real `IMAP fetch → store → read → SMTP send` against mock
  servers; store correctness fixes (FTS-vs-encryption, backfill, salvage schema,
  transient-error ≠ corruption).
- **Phase 2 — API completeness for the spine:** done. Full proto surface for the
  spine; contract-stability test; codegen docs; IPC removed.

**Phase 3 — Feature completeness — is in progress:**

- **3.A Filter correctness** — **done** (account scoping, `ON ACCOUNT`, non-ASCII
  offsets, `NOT IN`).
- **3.B Calendar** — engine layer **done** (real CalDAV `REPORT` transport,
  `TZID` fix, `CalendarBackend` trait, `calendar_events` store CRUD); the API
  vertical + recurrence wiring remain (see M1).
- **3.C Contacts (CardDAV)** and **3.D JMAP** — not started.

Dogfooding against a real mailbox and a fabrication audit surfaced additional
**real gaps** now folded into the milestones below: the filter match-and-act
engine is not yet wired into the live sync path; sync is not incremental;
account edit/delete/test-connection and TLS-mode selection are incomplete; and a
set of correctness/security items (see M5/M6).

**Explicitly deferred** (out of scope until the backend is client-ready and each
is re-justified as a genuine engine capability): OpenPGP/S-MIME E2EE (#79),
WASM/QuickJS plugins (#80), local-LLM summarization (#81).

---

## The path to client-ready

The remaining work is organized into seven milestones. M1–M5 complete the
**feature surface**; M6 hardens **security**; M7 is the **API-freeze gate** that
declares the backend ready for front-end repos. Milestones are ordered but
M1–M4 are largely independent (they touch different engine crates); the only
hard serialization is the shared `nuncio.proto` (and its golden), so proto
additions land one at a time.

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

Each milestone maps to GitHub issues (numbers above). Every capability is built
via one implementer + a spec/quality review gate + an offline E2E, committed only
when `cargo verify` is green and the proto golden is consistent. Issues close
only after the review step — never on a green build alone.
