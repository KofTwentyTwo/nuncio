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
  event bus, the API/IPC layer, and cross-cutting services (export, RBAC, audit,
  config). Note: the `e2ee`, `ai`, and `plugin` modules are non-functional
  placeholders in the current tree and are deferred until re-justified.
- **`nuncio-store`** — SQLite persistence in WAL mode, migrations, FTS search,
  payload ciphers (AES-GCM / age), corruption detection & recovery, and the OS
  keyring vault abstraction. Genuinely engineered; see the correctness fixes in
  [`BACKLOG.md`](BACKLOG.md) Phase 1.B.
- **`nuncio-mail`** — IMAP, JMAP, and SMTP engines and MIME handling. SMTP/MIME
  are solid; IMAP session code exists; JMAP needs a real HTTP client. None are yet
  driven by the daemon (Phase 1–3 wire them in).
- **`nuncio-cal`** — iCalendar parsing, CalDAV/CardDAV, recurrence (`rrule`), and
  scheduling. Parsing utilities exist but lack real transport and have TZID bugs
  (Phase 3).
- **`nuncio-contacts`** — contacts store, CardDAV sync, vCard generation.
- **`nuncio-filter`** — the NSQL declarative filter language: parser
  (`sqlparser`-based), multi-pass validator, and evaluation engine. The strongest
  subsystem; correctness fixes and action execution are Phase 3. See
  [`NSQL-Filter-Language-Specification.md`](NSQL-Filter-Language-Specification.md).

### The daemon

- **`nunciod`** — boots the event bus, opens/recovers the store, loads filter
  rules, runs background workers, and serves the API. Its central missing piece is
  real protocol sync: today "sync" only flips a status flag and the outbox worker
  *simulates* remote mutations. Phase 1 replaces this with a real
  `IMAP fetch → store → SMTP send` loop.

### The published API

- A versioned **gRPC** service defined by Protocol Buffers in `proto/nuncio/v1/`
  (introduced in Phase 1), served over **loopback TCP (`127.0.0.1`)** with a
  **bearer-token handshake** — the token minted into the OS keyring on first run
  and sent as gRPC metadata.
- **Unary RPCs** for commands and queries; **server-streaming RPCs** for push
  (new mail, sync progress, events). gRPC's multiplexing removes the
  response/notification demux race present in the current hand-rolled IPC.
- The `.proto` files are the versioned contract every client codegens against.

> **Current transport (to be replaced):** a hand-rolled length-prefixed
> (4-byte big-endian, 16 MB cap) JSON-RPC 2.0 protocol over **loopback TCP
> `127.0.0.1:9422`** (`NUNCIO_IPC_ADDR` override). It has a confirmed
> response/notification interleaving bug. Do not extend it; build on gRPC.

### Clients

- **`nuncio-cli`** stays in-repo as the reference gRPC client and E2E driver.
- **Native GUIs** (`nuncio-gui-macos` in Swift, `nuncio-gui-windows` in WinUI/C#),
  a **Rust TUI** (`nuncio-tui`), and an **MCP↔gRPC bridge** (`nuncio-mcp`) become
  separate repositories consuming the published API (Phase 5). Linux is supported
  at the daemon + CLI level; a native Linux GUI is deferred.

## Data & security model

- **Single local SQLite database per profile** (e.g. `~/.nuncio/`), WAL mode,
  single-writer through the daemon.
- **Credentials and keys belong in the OS keyring.** The current tree hardcodes
  crypto keys in source and never calls the keyring — a critical defect fixed in
  Phase 0.E before any at-rest-encryption or tamper-evidence claim is valid.
- **Full-text search** via SQLite FTS5 (trigram). The indexing-vs-encryption
  strategy is being corrected in Phase 1.B.
- **HTML email** must render sandboxed (`<iframe sandbox>`, JS disabled, strict
  CSP) in GUI clients.

## Diagram

A current, accurate topology diagram will be added once the gRPC layer lands
(Phase 1). The previous `architecture.png` depicted the superseded socket/4-shell
model and was removed.
