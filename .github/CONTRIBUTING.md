# Contributing to Nuncio

Thank you for your interest in contributing to Nuncio ([nuncio.mx](https://nuncio.mx)).

## Development Workflow

1. **Branch Naming**: All work must be conducted on dedicated feature or fix branches branching off `main`:
   - `feature/GH-123-short-description`
   - `fix/GH-45-short-description`
2. **Library-First Decoupling**: Business logic, protocol parsing, offline caching, and search indexing must be implemented in headless engine crates (`crates/nuncio-core`, `crates/nuncio-mail`, `crates/nuncio-cal`, `crates/nuncio-store`). Presentation shells (`nuncio-cli`, `nuncio-tui`, `nuncio-gui`) must remain thin UI layers.
3. **Commit Messages**: All git commits must follow Conventional Commits format:
   - `feat(mail): implement JMAP Email/changes sync`
   - `fix(store): resolve FTS5 trigram query escaping`
   - Subject lines must be under 72 characters, written in imperative mood.

## Quality Gates & Verification

Before submitting a Pull Request, verify that all local quality gates pass:

```bash
# Code formatting check
cargo fmt --all -- --check

# Linter checks with warnings as errors
cargo clippy --all-targets --workspace -- -D warnings

# Unit & Integration tests
cargo test --workspace
```

## Pull Request Process

1. Open a Pull Request targeting `main`.
2. Ensure the PR description references the relevant GitHub Issue (`Closes #123`).
3. Complete the PR template checklist confirming local test suite execution and linter verification.
4. All CI matrix checks (Windows, macOS, Linux) must pass 100% prior to merging.
