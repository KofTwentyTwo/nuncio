# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Nuncio ([nuncio.mx](https://nuncio.mx)) is a cross-platform (Linux/macOS/Windows) mail, calendar, and contacts suite written in Rust: a central daemon plus four presentation shells (CLI, TUI, GUI, MCP).

## Commands

Cargo aliases are defined in `.cargo/config.toml`:

```bash
cargo check-all   # clippy --all-targets --workspace -- -D warnings
cargo test-all    # cargo test --workspace
cargo cov         # cargo llvm-cov --workspace --fail-under-lines 100 (requires cargo-llvm-cov)
cargo verify      # fmt --check + check-all + test-all (run before every push/PR)
```

- Run a single test: `cargo test -p nuncio-store test_name` (or `cargo test --test integration_matrix` for root-level integration tests in `tests/`).
- CI's coverage gate excludes binary entry points: `cargo llvm-cov --workspace --ignore-filename-regex "main\.rs" --fail-under-lines 100`.
- The pre-commit hook (`.githooks/pre-commit`) runs fmt check, clippy, and the full test suite; a commit that hasn't passed `cargo verify` will fail it.
- GUI frontend (React 18 + Vite + TypeScript) lives in `crates/nuncio-gui/ui/`: `npm run dev` / `npm run build` from that directory. The Tauri shell (`crates/nuncio-gui/src-tauri/`) builds the frontend automatically via its `beforeBuildCommand`.
- Run shells: `cargo run -p nuncio-cli -- status`, `cargo run -p nuncio-tui`, `cargo run -p nuncio-gui`, `cargo run -p nunciod` (daemon).

Warnings are hard errors everywhere: `.cargo/config.toml` sets `rustflags = ["-D", "warnings", "-F", "unsafe_code", "-D", "unused_must_use"]`, and workspace lints in `Cargo.toml` deny `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, `clippy::todo`, `clippy::unimplemented`, and `clippy::unreachable`. `unwrap()`/`expect()` are permitted only in tests. Use `thiserror` error enums in library crates.

## Architecture

**Hybrid daemon-first model.** All state, storage, credentials, and protocol sync live in a standalone background daemon (`nunciod`). The four UIs are thin clients that talk to it over OS-native IPC — a UNIX domain socket (`~/.nuncio/nuncio.sock`) on POSIX, a named pipe (`\\.\pipe\nuncio-ipc`) on Windows — using JSON-RPC 2.0 framing. The client side (`IpcClient`, in `crates/nuncio-core/src/ipc/`) auto-spawns the daemon and retries; the server side (`IpcDaemonServer`) fans events out to all connected shells via the `nuncio-core` `EventBus` (`CoreCommand` in, `CoreEvent`/`AppState` snapshots out).

**Headless engine crates** (all business logic MUST live here, never in shells):

- `nuncio-core` — domain models (`Email`, `CalendarEvent`, `Contact`), `EventBus`, IPC client/server + framing, config, E2EE (OpenPGP/S-MIME), WASM plugins, `McpAgentPolicy` RBAC, WORM audit, self-update.
- `nuncio-mail` — IMAP4rev1, JMAP (RFC 8620/8621), SMTP (`lettre`), MIME parsing.
- `nuncio-cal` — CalDAV (RFC 4791), iCalendar (RFC 5545), `rrule` recurrence, NLP scheduler.
- `nuncio-contacts` — CardDAV sync (RFC 6352), vCard 4.0, email contact harvester.
- `nuncio-store` — SQLite WAL (`sqlx`), FTS5 trigram search, AES-256-GCM + `age` encryption at rest, OS keyring vault, WORM audit ledger.
- `nuncio-filter` — NSQL declarative filter language (parsed with `sqlparser`), AST actions, validator, dry-run, HMAC webhooks. Spec: `docs/NSQL-Filter-Language-Specification.md`.

**Presentation shells** (thin UI layers over `IpcClient` only): `nuncio-cli` (Noun+Verb subcommands, `--json` output), `nuncio-tui` (Ratatui, Vim keys), `nuncio-gui` (Rust view/sandbox layer + `src-tauri` Tauri v2 shell + `ui/` React frontend — two workspace crates plus an npm package), `nuncio-mcp` (MCP stdio server exposing tools/resources/prompts to LLM agents), `nunciod` (the daemon binary itself).

**Standing rule — 100% four-shell feature parity.** Any feature exposed in one shell MUST be implemented in all four: CLI subcommand (with `--json`), TUI keybinding/modal, GUI React component, and MCP tool handler.

## Testing

- 100% line coverage is enforced (`cargo cov`); all workspace tests must pass locally before committing.
- Live network calls in tests are forbidden. Mock all protocols (IMAP/JMAP/SMTP/CalDAV/CardDAV) with `wiremock` or mock traits (`MockMailBackend`, `MockCalendarBackend`), and the OS vault with the in-memory `MockKeyring`.
- Integration/E2E tests use isolated ephemeral SQLite databases (`tempfile` or `:memory:`, e.g. `DatabaseEngine::connect_ephemeral()`); no state leakage between tests. Root-level `tests/` holds the cross-crate E2E matrix (`integration_matrix.rs`, `ephemeral_harness.rs`).

## Security

- Never store plain-text passwords, tokens, or keys in source or SQLite — credentials go to OS vaults via `keyring`; attachments encrypted at rest via `age`; credential fields use `ZeroizeOnDrop`.
- Untrusted HTML email renders only inside `<iframe sandbox>` with JS disabled.

## Commits, Branches & Releases

- Conventional Commits (`feat(scope): description`), subject under 72 chars, imperative mood, zero AI attribution.
- All work on feature branches (`feature/GH-123-description`); never commit directly to `main`.
- SemVer 2.0.0; tags `vMAJOR.MINOR.PATCH`. **Unified V3 release mandate:** no public release tag or binary until Phase V3 is 100% complete and verified across all four shells on Linux, macOS, and Windows. Pushing a `v*.*.*` tag triggers `.github/workflows/release.yml` (packages + SHA256 checksums for `.zip`/`.msi`, `.tar.gz`/`.dmg`, `.tar.gz`/`.AppImage`).

## Planning & Task Tracking

Roadmap and issue state live on GitHub (Project board #5, milestones, issues) and in `docs/` (`PLAN-production-roadmap-100-plus.md`, `TODO.md`, `SESSION-STATE.md`). In the multi-agent split, Claude Code owns the headless engine crates (`nuncio-core`, `nuncio-mail`, `nuncio-cal`, `nuncio-store`, and by extension `nuncio-contacts`/`nuncio-filter`): protocol parsers, FTS5 migrations, `rrule` math, encryption, and coverage. CLI work is assigned to Codex; TUI/GUI/CI to Antigravity workers.
