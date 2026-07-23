# Nuncio Codebase Guidelines (CLAUDE.md)

Nuncio ([nuncio.mx](https://nuncio.mx)) is a cross-platform mail and calendar solution written in Rust.

## Architecture Guidelines

1. **Library-First ("Ghost" Decoupled Model)**:
   - Business logic, protocol clients, offline state synchronization, SQLite FTS5 search indexing, and cryptographic key management MUST reside strictly inside headless Rust crates: `crates/nuncio-core`, `crates/nuncio-mail`, `crates/nuncio-cal`, and `crates/nuncio-store`.
   - Presentation shells (`crates/nuncio-cli`, `crates/nuncio-tui`, `crates/nuncio-gui`) MUST remain thin UI layers interacting with `nuncio-core` via async Tokio state streams (`watch`) and command channels (`mpsc`).

2. **Language & Code Quality Standards**:
   - **Rust Edition**: 2021 edition across all workspace crates.
   - **Compiler & Linter Gates**: `rustflags = ["-D", "warnings"]` configured in `.cargo/config.toml`. All compiler and clippy warnings MUST be treated as hard build errors during normal development.
   - **Formatting**: `cargo fmt` enforced. No unformatted code allowed.
   - **Error Handling**: Use `thiserror` for library crates (`nuncio-core`, `nuncio-mail`, `nuncio-cal`, `nuncio-store`). `unwrap()` and `expect()` are **forbidden** in production library code (permitted only in tests and prototypes).
   - **Security**: Never store plain-text passwords, tokens, or encryption keys in source files or SQLite tables. Route all credentials to OS native vaults via `keyring`. Payload attachments encrypted at rest via `age`. Untrusted HTML email sandboxed inside `<iframe sandbox>` with JS disabled.

3. **Testing, E2E & Mocking Standards**:
   - **Test-First Commit Policy**: 100% of workspace tests (`cargo test --workspace`) MUST pass locally before committing or triggering CI pipelines.
   - **100% Line Coverage Requirement**: **100% line coverage** is required for unit tests across workspace engine crates (`cargo llvm-cov --workspace --fail-under-lines 100`).
   - **Integration Testing Standards**: `tests/` directories MUST test multi-crate interaction, SQLite WAL migrations, FTS5 trigram queries, and channel event loops using isolated ephemeral databases (`tempfile` or `:memory:`). State leakage between tests is forbidden.
   - **End-to-End (E2E) Testing Standards**: Headless E2E integration tests MUST validate complete user workflows from `nuncio-cli` subcommands and `nuncio-core` state streams down through storage and protocol layers.
   - **Full Mocks for External Systems**:
     - All external network protocols (JMAP, IMAP, CalDAV, CardDAV, SMTP) MUST be 100% mocked for offline test execution using `wiremock` or mock trait implementations (`MockMailBackend`, `MockCalDavClient`).
     - OS native vaults MUST be mocked via an in-memory `MockKeyring` provider during tests.
     - Live network calls during test runs are strictly **forbidden**.

4. **Commit & Branch Conventions**:
   - Commit messages MUST follow Conventional Commits format (`feat(scope): description`, `fix(scope): description`). Subject lines under 72 characters, imperative mood, zero AI attribution.
   - All work MUST be performed on feature branches (`feature/GH-123-description`), never committed directly to `main`.

## Local Build & Quality Verification Commands

Normal development automatically enforces compiler warnings as errors via `.cargo/config.toml`. Before pushing or creating a Pull Request, run the full verification suite locally:

```bash
# 1. Format Check
cargo fmt --all -- --check

# 2. Clippy Linter Check (warnings as errors)
cargo check-all

# 3. Complete Test Suite Execution (Unit, Integration & E2E - 100% passing required)
cargo test-all

# 4. Enforce 100% Code Coverage Threshold
cargo llvm-cov --workspace --fail-under-lines 100
```
