# Contributing to Nuncio

Thank you for contributing to Nuncio ([nuncio.mx](https://nuncio.mx))!

## Semantic Versioning 2.0.0 & Tagging

Nuncio follows **Semantic Versioning 2.0.0** (`vMAJOR.MINOR.PATCH`):
- `MAJOR`: Incremented for incompatible API or structural changes.
- `MINOR`: Incremented for new functionality added in a backward-compatible manner.
- `PATCH`: Incremented for backward-compatible bug fixes.

Tagging a commit with `vX.Y.Z` triggers [.github/workflows/release.yml](file:///R:/Git.Local/KofTwentyTwo/nuncio/.github/workflows/release.yml) to automatically compile cross-platform release artifacts (Windows `.zip`/`.msi`, macOS `.tar.gz`/`.dmg`, Linux `.tar.gz`/`.AppImage`), generate release notes from Conventional Commits, compute SHA256 checksums, and publish an official GitHub Release.

## Architecture Decoupling Rules

1. **Library-First Model**: Business logic, protocol parsing, offline caching, search indexing, and cryptographic key management MUST reside strictly inside headless Rust crates: `crates/nuncio-core`, `crates/nuncio-mail`, `crates/nuncio-cal`, and `crates/nuncio-store`.
2. **Skinny Shells**: Presentation shells (`crates/nuncio-cli`, `crates/nuncio-tui`, `crates/nuncio-gui`) MUST remain thin UI layers interacting with `nuncio-core` via async Tokio channels.

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
