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

## JetBrains RustRover Setup

Nuncio is fully pre-configured for **JetBrains RustRover**:

- **Cargo Workspace Auto-Detection**: Open `R:\Git.Local\KofTwentyTwo\nuncio` in RustRover. The IDE will automatically index all 7 workspace member crates (`crates/*`).
- **Pre-Configured Shared Run Configurations** (accessible in RustRover's top-right run menu):
  - `Cargo Check All`: Runs `cargo check-all` (compiler/clippy warnings as errors).
  - `Cargo Test All`: Runs full workspace test suite.
  - `Cargo Coverage Gate`: Runs `cargo cov` (100% unit test line coverage verification).
  - `Run nuncio-cli (status)`: Launches CLI status subcommand.
  - `Run nuncio-tui`: Launches Terminal UI client.
  - `Run nuncio-gui`: Launches Native Desktop GUI shell.
- **Automatic Code Formatting**: `rustfmt` format-on-save is enabled by default via `.idea/codeStyles/Project.xml`.

---

## Overview

Nuncio follows a library-first architecture. All core business logic, protocol synchronization, offline storage, indexing, and state management are isolated within headless Rust libraries. Presentation layers remain lightweight shells driving a command line interface (CLI), terminal UI (TUI), or native desktop GUI.
## CLI Usage (Noun + Verb Syntax)

Nuncio CLI follows a standardized `<Noun> <Verb> [Flags]` command structure:

```bash
# Account Operations
nuncio account add --email james.maes@kof22.com --imap-host mail.kof22.com --imap-port 993
nuncio account list

# Email Operations
nuncio mail sync [--account <id>]
nuncio mail list --folder INBOX
nuncio mail read --id msg_123
nuncio mail search --query "roadmap"
nuncio mail send --to alice@nuncio.mx --subject "Update" --body "Message"

# Folder & Calendar Operations
nuncio folder list
nuncio cal list
nuncio system status
```

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
