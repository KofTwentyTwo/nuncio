# Nuncio

Cross-platform mail and calendar solution designed for Windows, macOS, and Linux, providing CLI, TUI, and GUI interfaces.

Official site: [nuncio.mx](https://nuncio.mx)

> **Etymology**: Derived from the Latin verb ***nūntiō*** ("I announce", "I declare", "I deliver a message") and noun ***nūntius*** ("messenger", "courier", "bearer of tidings"). Nuncio is built as the ultimate cross-platform messenger and calendar courier.

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

## Requirements & Building

- Rust 1.75+ toolchain

```bash
cargo build --workspace
```

To run the command-line interface:

```bash
cargo run -p nuncio-cli -- status
```

To run the terminal client:

```bash
cargo run -p nuncio-tui
```

To run the desktop GUI client:

```bash
cargo run -p nuncio-gui
```

## License

MIT OR Apache-2.0
