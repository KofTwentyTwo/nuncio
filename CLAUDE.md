# Nuncio Codebase Guidelines (CLAUDE.md)

Nuncio ([nuncio.mx](https://nuncio.mx)) is a cross-platform mail and calendar solution written in Rust.

## Semantic Versioning 2.0.0 & Release Policy

- **SemVer Standard**: All releases follow **Semantic Versioning 2.0.0** (`MAJOR.MINOR.PATCH`).
- **UNIFIED V3 RELEASE MANDATE**: **No public release tag or binary distribution will be cut until Phase V3 (Platform & AI Automation) is 100% feature complete, verified, and working across all 4 presentation UIs on Linux, macOS, and Windows.**
- **Git Tag Format**: Release tags MUST follow `vMAJOR.MINOR.PATCH` format (e.g. `v3.0.0` for GA launch).
- **Automated GitHub Releases Workflow**: Pushing a `v*.*.*` tag triggers [.github/workflows/release.yml](file:///R:/Git.Local/KofTwentyTwo/nuncio/.github/workflows/release.yml) to compile, package, compute SHA256 checksums, and publish release binaries for Windows (`.zip` / `.msi`), macOS (`.tar.gz` / `.dmg`), and Linux (`.tar.gz` / `.AppImage`).

## JetBrains RustRover IDE Integration

Nuncio is pre-configured for **JetBrains RustRover** out of the box:
- **Workspace Cargo Resolution**: RustRover automatically recognizes all 7 workspace member crates (`crates/*`).
- **Pre-Configured Shared Run Configurations**: Located in `.idea/runConfigurations/`:
  - `Cargo Check All`: Executes `cargo check-all` with warnings treated as errors.
  - `Cargo Test All`: Executes `cargo test-all` for the full workspace.
  - `Cargo Coverage Gate`: Executes `cargo cov` measuring 100% unit test line coverage.
  - `Run nuncio-cli (status)`: Executes `cargo run -p nuncio-cli -- status`.
  - `Run nuncio-tui`: Executes `cargo run -p nuncio-tui`.
  - `Run nuncio-gui`: Executes `cargo run -p nuncio-gui`.
- **Code Style & Formatting**: `.idea/codeStyles/Project.xml` configures RustRover to format files automatically via `rustfmt` on save.

## Multi-Agent Execution Framework

- **Master Orchestrator Agent (`agy` / Antigravity)**: Manages high-level task decomposition, roadmap tracking ([docs/PLAN-nuncio-roadmap.md](file:///R:/Git.Local/KofTwentyTwo/nuncio/docs/PLAN-nuncio-roadmap.md), [docs/TODO.md](file:///R:/Git.Local/KofTwentyTwo/nuncio/docs/TODO.md)), subagent dispatching, quality gate validation (`cargo verify`, `cargo cov`), git branch workflows, and wiki documentation.
- **Claude Code Subagent (`claude`)**: Assigned to headless engine crates (`crates/nuncio-core`, `crates/nuncio-mail`, `crates/nuncio-cal`, `crates/nuncio-store`), SQLite FTS5 migrations, protocol parsers, `rrule` recurrence math, `age` encryption, and 100% unit test coverage.
- **OpenAI Codex Subagent (`codex`)**: Assigned to CLI subcommands and shell pipeline automation (`crates/nuncio-cli`).
- **Antigravity Worker Subagent (`agy-worker`)**: Assigned to TUI (`crates/nuncio-tui`), GUI (`crates/nuncio-gui`), webview sandboxing, and CI/CD cross-platform release matrix.

## Architecture Guidelines

1. **Library-First ("Ghost" Decoupled Model)**:
   - Business logic, protocol clients, offline state synchronization, SQLite FTS5 search indexing, and cryptographic key management MUST reside strictly inside headless Rust crates: `crates/nuncio-core`, `crates/nuncio-mail`, `crates/nuncio-cal`, and `crates/nuncio-store`.
   - Presentation shells (`crates/nuncio-cli`, `crates/nuncio-tui`, `crates/nuncio-gui`, `crates/nuncio-mcp`) MUST remain thin UI layers interacting with `nuncio-core` via `IpcClient` and async Tokio channels.

2. **Standing Rule: 100% Multi-Shell Feature Parity Equivalence**:
   - **Mandatory 1:1 Parity**: Any feature, action, search query, configuration setting, or workflow exposed in one presentation shell MUST be fully implemented and accessible across ALL FOUR presentation shells (`nuncio-cli`, `nuncio-tui`, `nuncio-gui`, `nuncio-mcp`).
   - Adding a new feature to V1, V2, or V3 automatically requires implementing its CLI subcommand (`--json`), TUI keyboard shortcut/modal, GUI React component, and MCP LLM tool handler.

3. **Language & Code Quality Standards**:
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
