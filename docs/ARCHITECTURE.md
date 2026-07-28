# Nuncio Architecture

> **Status (2026-07-26):** this describes the **target** architecture the project
> is being rebuilt toward, and flags where the current tree differs. It replaces
> the previous `Architecture-Specification.md` (removed — it contained fabricated
> benchmarks and claims). Authoritative plan: [`ROADMAP.md`](ROADMAP.md); decision:
> [`adr/0001-engine-first-grpc-architecture.md`](adr/0001-engine-first-grpc-architecture.md).

## Principle: the daemon is the product

`nunciod` owns everything that matters — persistent state, credentials, protocol
synchronization, filtering, and all business logic. Every user interface is a thin
client that talks to the daemon over a published API and holds no logic or data of
its own. This is the language-server / database-server model.

The prior POC inverted this: four UI shells each carried their own (fake or
throwaway) state and business logic, so nothing worked end-to-end and the shells
drifted apart. Centralizing on the daemon makes correctness and cross-client
parity structural.

## Components

### Engine libraries (composed by `nunciod`)

- **`nuncio-core`** — domain models (`Email`, `CalendarEvent`, `Contact`), the
  event bus, and cross-cutting services (export, RBAC, audit, config). Note: the
  `e2ee`, `ai`, and `plugin` modules are non-functional placeholders in the
  current tree and are deferred until re-justified.
- **`nuncio-store`** — SQLite persistence in WAL mode, migrations, FTS search,
  payload ciphers (AES-GCM / age), corruption detection & recovery, and the OS
  keyring vault abstraction. Genuinely engineered; the FTS-vs-encryption,
  backfill, salvage-schema, and transient-error-vs-corruption fixes tracked in
  [`BACKLOG.md`](BACKLOG.md) Phase 1.B are done.
- **`nuncio-mail`** — IMAP, JMAP, and SMTP engines and MIME handling. IMAP and
  SMTP are wired into the daemon's real `fetch → store → read → send` spine
  (Phase 1–2); JMAP still needs a real HTTP client (roadmap M3).
- **`nuncio-cal`** — iCalendar parsing, CalDAV/CardDAV, recurrence (`rrule`), and
  scheduling. The engine layer is done (real CalDAV `REPORT` transport, the TZID
  fix, a `CalendarBackend` trait, `calendar_events` store CRUD); the API vertical
  and recurrence wiring are roadmap M1.
- **`nuncio-contacts`** — contacts store, CardDAV sync, vCard generation. Not yet
  wired into the daemon or persisted across restarts (roadmap M2).
- **`nuncio-filter`** — the NSQL declarative filter language: parser
  (`sqlparser`-based), multi-pass validator, and evaluation engine. The strongest
  subsystem; correctness fixes (account scoping, `ON ACCOUNT`, non-ASCII offsets,
  `NOT IN`) are done. Wiring the match-and-act engine into the live sync path so a
  matching rule actually fires its action is roadmap M4. See
  [`NSQL-Filter-Language-Specification.md`](NSQL-Filter-Language-Specification.md).

### The daemon

- **`nunciod`** — boots the event bus, opens/recovers the store, loads filter
  rules, runs background workers, and serves the gRPC API. The
  `IMAP fetch → store → read → SMTP send` spine is real and works end-to-end
  (Phase 1–2). The remaining gaps are wiring the filter engine's actions into
  that live sync path (roadmap M4) and incremental — rather than full-historical
  — sync (roadmap M5).

### The published API

- A versioned **gRPC** service defined by Protocol Buffers in `proto/nuncio/v1/`
  (delivered in Phase 1), served over **loopback TCP (`127.0.0.1:9420`)** with a
  **bearer-token handshake** — the token minted into the OS keyring on first run
  and sent as gRPC metadata.
- **Unary RPCs** for commands and queries; **server-streaming RPCs** for push
  (new mail, sync progress, events). gRPC's multiplexing removes the
  response/notification demux race that the old hand-rolled IPC suffered from.
- The `.proto` files are the versioned contract every client codegens against.

> **Removed:** the daemon previously also ran a hand-rolled length-prefixed
> JSON-RPC 2.0 transport on `127.0.0.1:9422`, which had a response/notification
> interleaving bug. It has been deleted — the daemon serves gRPC only.

### Clients

- **`nuncio-cli`** stays in-repo as the reference gRPC client and E2E driver.
- **Native GUIs** (`nuncio-gui-macos` in Swift, `nuncio-gui-windows` in WinUI/C#),
  a **Rust TUI** (`nuncio-tui`), and an **MCP↔gRPC bridge** (`nuncio-mcp`) become
  separate repositories consuming the published API (Phase 5). Linux is supported
  at the daemon + CLI level; a native Linux GUI is deferred.

## Data & security model

- **Single local SQLite database per profile** (e.g. `~/.nuncio/`), WAL mode,
  single-writer through the daemon.
- **Credentials and keys belong in the OS keyring.** Fixed in Phase 0.E:
  production code reads/writes real keys via `OsKeyring` (Windows Credential
  Manager / Keychain / Secret Service); only test code uses `MockKeyring`.
- **Full-text search** via SQLite FTS5 (trigram). The indexing-vs-encryption
  strategy was corrected in Phase 1.B.
- **HTML email** must render sandboxed (`<iframe sandbox>`, JS disabled, strict
  CSP) in GUI clients.

## Diagram

A current, accurate topology diagram covering the gRPC layer has not yet been
added. The previous `architecture.png` depicted the superseded socket/4-shell
model and was removed.
