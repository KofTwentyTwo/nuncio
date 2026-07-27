# ADR 0001 — Engine-first architecture with a published gRPC API

- **Status:** Accepted
- **Date:** 2026-07-26
- **Deciders:** James Maes
- **Supersedes:** the Antigravity-authored "sovereign architecture" direction and
  the four-shell monorepo model (see the 2026-07-26 forensic assessment).

## Context

The original Nuncio POC was built as a Cargo workspace containing a core/daemon
plus four in-repo presentation shells (CLI, TUI, GUI/Tauri, MCP). A forensic
assessment on 2026-07-26 found that the mail spine (fetch → store → read → send)
worked in **none** of the four shells: each shell ran on its own disjoint,
throwaway, or hardcoded data source and fabricated success. Building UI shells
before the engine existed caused the shells to fake what the engine could not
provide, and the "100% four-shell parity" mandate was unenforceable by discipline.

We want a foundation where the engine can become genuinely solid and complete,
where feature parity across UIs is structural rather than aspirational, and where
each UI can eventually use the best-fit native technology per platform.

## Decision

1. **`nunciod` is the product.** It owns all state, credentials, protocol logic,
   and business logic. The other engine crates are libraries it composes.
2. **The daemon publishes a versioned gRPC API** defined by Protocol Buffers in
   `proto/nuncio/v1/`, served over **loopback TCP (`127.0.0.1`)** with a
   **bearer-token handshake**; the token is minted into the OS keyring on first
   run and passed as gRPC metadata. Push (new mail, sync progress, events) uses
   **gRPC server-streaming**.
3. **The `.proto` files are the single source of truth** for the contract, are
   versioned in-repo, and are what every client codegens against. Parity is a
   property of the contract.
4. **`nuncio-cli` remains in-repo as the reference gRPC client** and E2E driver.
5. **All other UIs become separate projects/repos** consuming the published API:
   `nuncio-tui` (Rust), `nuncio-gui-macos` (Swift), `nuncio-gui-windows`
   (WinUI/C#), `nuncio-mcp` (MCP↔gRPC bridge). Native per-platform, not one
   cross-platform shell.

## Consequences

**Positive**
- Parity is guaranteed by the shared contract; a client cannot offer what the API
  does not expose.
- The workspace shrinks to engine + CLI, making the warnings-as-errors and E2E
  gates achievable — a precondition for "100% solid."
- gRPC gives strong typing and mature codegen into Swift, C#, TS, and Rust, and
  first-class server-streaming for mail's push model.
- gRPC's multiplexing eliminates the confirmed response/notification demux race in
  the old hand-rolled JSON-RPC IPC.
- Native per-platform clients (matching the gclo WinUI + gclo-macos Swift pattern)
  instead of one compromise Tauri app.

**Negative / costs**
- A published API is a versioned contract: it needs SDK/stub distribution and
  contract tests so external clients don't silently break.
- Native per-platform clients mean more client repos to maintain than one shell.
- Loopback + token is adequate for local sovereign use but is **not** a networked
  auth model; a future networked mode (see Alternatives) would require real
  authN/Z, TLS, and a multi-tenant data model.

## Alternatives considered

- **JSON-RPC 2.0 + OpenRPC** (evolve the existing IPC): lowest migration cost and
  human-readable, but weaker cross-language typing/codegen than gRPC. Rejected in
  favor of stronger contracts for polyglot native clients.
- **REST + OpenAPI + WebSocket/SSE**: browser-native and familiar, but two
  mechanisms to maintain and an awkward fit for RPC-shaped mail operations.
- **Networked server from the start**: enables mobile/web/multi-device immediately
  but forces full authN/Z, TLS, and multi-tenancy into V1 and reshapes the
  sovereign story. Deferred; may return as an explicit future ADR.
- **One cross-platform GUI (Tauri/Flutter)**: fewer client repos, but back to one
  compromise GUI technology. Rejected in favor of native per-platform.
