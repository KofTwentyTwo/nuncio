# Nuncio Codebase Guidelines (CLAUDE.md)

Nuncio ([nuncio.mx](https://nuncio.mx)) is a cross-platform mail and calendar solution written in Rust.

## Architecture Guidelines

1. **Library-First ("Ghost" Decoupled Model)**:
   - Business logic, protocol clients, offline state synchronization, SQLite FTS5 search indexing, and cryptographic key management MUST reside strictly inside headless Rust crates: `crates/nuncio-core`, `crates/nuncio-mail`, `crates/nuncio-cal`, and `crates/nuncio-store`.
   - Presentation shells (`crates/nuncio-cli`, `crates/nuncio-tui`, `crates/nuncio-gui`) MUST remain thin UI layers interacting with `nuncio-core` via async Tokio state streams (`watch`) and command channels (`mpsc`).

2. **Language & Code Quality Standards**:
   - **Rust Edition**: 2021 edition across all workspace crates.
   - **Formatting**: `cargo fmt` enforced. No unformatted code.
   - **Linter Gates**: `cargo clippy --all-targets --workspace -- -D warnings` enforced. Warnings MUST be treated as errors.
   - **Error Handling**: Use `thiserror` for library crates (`nuncio-core`, `nuncio-mail`, `nuncio-cal`, `nuncio-store`). Avoid `unwrap()` or `expect()` outside unit tests and prototypes.
   - **Security**: Never store plain-text passwords, tokens, or encryption keys in source files or SQLite tables. Route all credentials to OS native vaults via `keyring`.

3. **Commit & Branch Conventions**:
   - Commit messages MUST follow Conventional Commits format (`feat(scope): description`, `fix(scope): description`).
   - All work MUST be performed on feature branches (`feature/GH-123-description`), never directly on `main`.

## Local Build & Quality Verification Commands

Before pushing or creating a Pull Request, run the full verification suite locally:

```bash
# 1. Format Check
cargo fmt --all -- --check

# 2. Clippy Linter Check (warnings as errors)
cargo clippy --all-targets --workspace -- -D warnings

# 3. Complete Test Suite Execution (100% passing required)
cargo test --workspace
```
