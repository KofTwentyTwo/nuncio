# Contributing to Nuncio

Thank you for contributing to Nuncio!

## Semantic Versioning 2.0.0 & Tagging

Nuncio follows **Semantic Versioning 2.0.0** (`vMAJOR.MINOR.PATCH`):
- `MAJOR`: Incremented for incompatible API or structural changes.
- `MINOR`: Incremented for new functionality added in a backward-compatible manner.
- `PATCH`: Incremented for backward-compatible bug fixes.

Tagging a commit with `vX.Y.Z` is intended to drive the release workflow under
`.github/workflows/`. **Pre-alpha note:** the release pipeline is being rebuilt and
**no functional release exists yet** — see roadmap milestone M7 (issue #209). Do not
tag releases until the backend is declared client-ready.

## Architecture Decoupling Rules

2. **Thin Presentation Shells**: Presentation shells (`crates/nuncio-cli`, `crates/nuncio-tui`, `crates/nuncio-gui`, `crates/nuncio-mcp`) MUST remain thin UI layers interacting with `nunciod` over length-prefixed IPC streams.
3. **100% Multi-Shell Feature Parity Rule**: Any new feature or command added to one presentation shell MUST be implemented simultaneously across ALL FOUR presentation shells.

## Code Quality Gates

Before submitting a Pull Request, run the local quality suite:

```bash
cargo fmt --all -- --check
cargo check-all
cargo test-all
cargo cov
```

All 4 checks MUST pass cleanly. Compiler warnings are treated as hard errors (`-D warnings`). 100% unit test line coverage is required.

## Commit Message Format

Follow Conventional Commits:

```
feat(mail): add JMAP WebSocket push state engine
fix(store): resolve SQLite FTS5 trigram deadlock on concurrent insert
ci: update GitHub Actions release matrix workflow
```
