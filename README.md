# Nuncio

Cross-platform mail and calendar solution designed for Windows, macOS, and Linux, providing CLI, TUI, and GUI interfaces.

Official site: [nuncio.mx](https://nuncio.mx)

> **Etymology**: Derived from the Latin verb ***nūntiō*** ("I announce", "I declare", "I deliver a message") and noun ***nūntius*** ("messenger", "courier", "bearer of tidings"). Nuncio is built as the ultimate cross-platform messenger and calendar courier.

---

## Single Source of Truth for Project Planning

Project planning, issue tracking, atomic micro-milestones, and subagent assignments are authoritatively managed directly on GitHub:

- **GitHub Project Board**: [Nuncio Roadmap Project #5](https://github.com/users/KofTwentyTwo/projects/5)
- **GitHub Milestones**: [KofTwentyTwo/nuncio/milestones](https://github.com/KofTwentyTwo/nuncio/milestones) (`v0.1.0` through `v1.0.0`)
- **GitHub Issues**: [KofTwentyTwo/nuncio/issues](https://github.com/KofTwentyTwo/nuncio/issues)

---

## Overview

Nuncio follows a library-first architecture. All core business logic, protocol synchronization, offline storage, indexing, and state management are isolated within headless Rust libraries. Presentation layers remain lightweight shells driving a command line interface (CLI), terminal UI (TUI), or native desktop GUI.

## Workspace Architecture

- `crates/nuncio-core`: Workspace management, account orchestration, async event routing.
- `crates/nuncio-mail`: IMAP4rev1, JMAP (RFC 8620/8621), and SMTP protocol engines.
- `crates/nuncio-cal`: CalDAV and iCalendar (RFC 5545) client libraries.
- `crates/nuncio-store`: Encrypted local persistence and SQLite FTS5 search indexing.
- `crates/nuncio-cli`: Scriptable command-line interface binary for Unix pipes and automation.
- `crates/nuncio-tui`: Terminal user interface powered by Ratatui.
- `crates/nuncio-gui`: Native desktop graphical user interface shell.

## Local Build & Verification Commands

- Rust 1.75+ toolchain

```bash
# Compiler & Linter Verification (warnings as errors)
cargo check-all

# Run Full Test Suite
cargo test-all

# 100% Line Coverage Gate Verification
cargo cov
```

## License

MIT OR Apache-2.0
