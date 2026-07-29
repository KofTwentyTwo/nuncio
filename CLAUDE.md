# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

Nuncio is a **local-first mail, calendar, and contacts engine** written in Rust
(edition 2021, Cargo workspace). It is **pre-alpha and under active
reconstruction** after a 2026-07-26 forensic assessment found the original
AI-generated proof of concept was largely non-functional and its documentation
substantially fabricated. **Trust the code and this file over any older prose.**

Read these before substantive work:
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — the authoritative plan and target architecture (rendered page: <https://koftwentytwo.github.io/nuncio/roadmap/>)
- [`docs/STORY-WORKFLOW.md`](docs/STORY-WORKFLOW.md) — how to execute a roadmap story (gates, patterns to mirror, branch/PR/review flow)
- [`docs/HANDOFF.md`](docs/HANDOFF.md) — current state, branches, and the M1–M7 story index for picking up on a new machine
- [`docs/adr/0001-engine-first-grpc-architecture.md`](docs/adr/0001-engine-first-grpc-architecture.md) — the governing architecture decision
- [`docs/BACKLOG.md`](docs/BACKLOG.md) — engineering-ready Phase 0 / Phase 1 stories (Phase 2+ is in `docs/ROADMAP.md`)

## Architecture (target)

The daemon is the product; UIs are thin clients over a published contract.

- **`nunciod`** owns all state (SQLite/WAL), credentials (OS keyring), protocol
  sync logic, the filter engine, and all business logic. The `nuncio-*` library
  crates are composed by the daemon.
- The daemon **publishes a versioned gRPC API** (`proto/nuncio/v1/*.proto`, being
  introduced in roadmap Phase 1) over **loopback TCP with a bearer-token
  handshake** (token in the OS keyring). **Server-streaming RPCs carry push**
  (new mail, sync progress, events).
- The `.proto` files are the single source of truth for the contract. **Feature
  parity across clients is guaranteed by the contract, not by discipline.**
- **`nuncio-cli` is the in-repo reference client** and E2E driver. All other UIs
  (native macOS/Windows GUIs, TUI, MCP bridge) become separate repositories.

### Current vs target state

The presentation shells (`nuncio-tui`, `nuncio-gui`, `nuncio-mcp`) have been moved
to `_reference/` (out of the workspace) and will be rebuilt as separate native
client repos. The daemon serves a single transport — the **gRPC API on loopback
`127.0.0.1:9420`**; the old hand-rolled JSON-RPC IPC has been removed.

### Library-first boundary

Business logic, protocol clients, storage, search, and key management live in the
headless crates (`nuncio-core`, `nuncio-store`, `nuncio-mail`, `nuncio-cal`,
`nuncio-contacts`, `nuncio-filter`). Presentation code must remain a thin client
of the daemon's API and hold no business logic or private data store.

## Build, test, and quality gates

Warnings are hard errors and `unsafe_code` is forbidden (see
[`.cargo/config.toml`](.cargo/config.toml) and workspace lints in
[`Cargo.toml`](Cargo.toml): `unwrap_used`, `expect_used`, `panic`, `todo` are all
`deny`).

Cargo aliases (defined in `.cargo/config.toml`):

```bash
cargo check-all   # = clippy --all-targets --workspace -- -D warnings   (the lint gate)
cargo test-all    # = test --workspace
cargo cov         # = llvm-cov --workspace --ignore-filename-regex main.rs --summary-only  (informational; needs cargo-llvm-cov)
```

There is no `cargo verify` alias — Cargo aliases are a single command's argv and
cannot chain subcommands. Run the full gate as three separate commands (this is
exactly what the pre-commit hook runs):

```bash
cargo fmt --all -- --check
cargo check-all
cargo test-all
```

Common commands:

```bash
cargo build --workspace
cargo test -p nuncio-store                       # one crate
cargo test -p nuncio-filter parser::             # one module's tests
cargo test -p nunciod --test daemon_e2e_test     # one integration suite
cargo run -p nunciod                             # start the daemon (gRPC on 127.0.0.1:9420)
cargo run -p nuncio-cli -- system status         # drive it via the CLI (daemon must be running)
```

> **Gate status:** the workspace is green under the pinned toolchain
> (`rust-toolchain.toml` = 1.97.1), so the local pre-commit hook runs the exact
> `fmt`/`clippy --all-targets`/tests that CI does — "green locally" is CI-equivalent.
> CI itself is paused until the GitHub Actions minutes reset (releases are tag-only).
> Coverage is informational, not a 100% gate.

## RustRover / IDE

A shared RustRover setup lives in `.idea/` (the useful parts are tracked;
`workspace.xml`/`*.iml` and other local churn are gitignored):
- **Cargo project** auto-resolves from the root `Cargo.toml`; after pulling changes
  that add or move crates, run **Reload Cargo Project** (RustRover usually prompts).
- **Toolchain** comes from `rust-toolchain.toml` (1.97.1) automatically.
- **rustfmt on save** is enabled (`.idea/codeStyles/Project.xml`).
- **Shared run configurations** (`.idea/runConfigurations/`):
  - *Run nunciod (daemon)* — start the daemon.
  - *Run nuncio-cli (system status)* — drive it (start the daemon first).
  - *Cargo Build Release (daemon + CLI)* — `cargo build-release`.
  - *Cargo Check All* · *Cargo Test All* — the gates (run `cargo fmt --all -- --check`
    separately; there is no combined run configuration, since Cargo aliases and
    RustRover run configurations can't chain subcommands).
  - *Cargo Coverage* — informational `llvm-cov` (needs `cargo-llvm-cov` installed).
- Tip: set Clippy as the external linter (Settings → Rust → External Linters) so the
  editor mirrors the `-D warnings` gate.
- **Full walkthrough:** [`docs/DEVELOPING-IN-RUSTROVER.md`](docs/DEVELOPING-IN-RUSTROVER.md)
  — setup, running the daemon + CLI, running/debugging tests, gates, and the dev loop.

## Conventions

- **Rust edition 2021**; `cargo fmt` enforced (no unformatted code).
- **Error handling:** `thiserror` in library crates; `unwrap()`/`expect()` are
  forbidden in production code (tests only).
- **Security:** never store plaintext credentials/keys in source or SQLite; route
  secrets through the OS keyring; encrypt attachments at rest; sandbox untrusted
  HTML email in an `<iframe sandbox>` with JS disabled. (Several of these are
  aspirational in the current tree — see the assessment; Phase 0.E hardens them.)
- **Commits:** Conventional Commits (`feat(scope): …`, `fix(scope): …`), imperative,
  <72-char subjects, no AI attribution.
- **Comments:** explain the code's *intent, constraints, and rationale*. Do NOT
  put backlog-story numbers (`1.C.2`), GH issue references (`#156`, `GH-156`,
  `Refs #`), or `Phase N` breadcrumbs in source comments (`.rs`/`.proto`) — that
  traceability belongs in git history and `docs/`, and it rots in the code.
  Keep genuine technical rationale (RFC citations, algorithm notes, why-not-X).
  (Commit messages may still cite `Refs #NNN`.)
- **Branches:** feature branches; never commit directly to `main`. Branch flow is
  `dev` → `rc` → `main`.
- **A capability is "done" only when engine + proto + CLI command + offline E2E
  test all exist and CI is green.** No fabricated success output — an unimplemented
  path returns an honest error, never fake data.
- **Roadmap is dual-format — keep it in sync.** `docs/ROADMAP.md` is the source of
  truth; `docs/roadmap/index.html` is its rendered GitHub Pages page
  (<https://koftwentytwo.github.io/nuncio/roadmap/>, linked from `README.md`).
  Any change to milestones, status, or scope MUST update **both** in the same
  commit — never let the rendered page drift from the markdown, and keep the
  `README.md` link valid.

## Testing standards

- 100% of `cargo test-all` must pass before committing.
- Integration tests use ephemeral SQLite (`tempfile` / `:memory:`); no state
  leakage between tests.
- **All external protocols (IMAP/JMAP/SMTP/CalDAV/CardDAV) must be mocked**
  (`wiremock` or the `Mock*Backend` traits); the OS keyring is mocked via
  `MockKeyring`. **Live network calls in tests are forbidden.**

## Release policy

SemVer 2.0.0; release tags are `vMAJOR.MINOR.PATCH`. **No release should be
presented as functional until the engine and its API are solid (roadmap Phase 4).**
Existing public pre-1.0 releases are non-functional and are being marked pre-alpha.
