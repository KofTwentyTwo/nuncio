# PLAN: Nuncio Mail & Calendar Infrastructure

## Goal
Build Nuncio (nuncio.mx), a high performance cross-platform mail and calendar application written in Rust, following a library-first architecture with minimal UI shells for Terminal (TUI) and Desktop (GUI).

## Architecture Strategy
- Core Libs First: All protocol handling, offline synchronization, storage, indexing, and cryptography reside in decoupled Rust crates.
- Skinny UI Shells: Terminal and Desktop shells consume core APIs via async channels and state streams.
- Target Platforms: Windows, macOS, and Linux.

## Crate Layout
- `crates/nuncio-core`: Workspace management, account orchestration, async event bus.
- `crates/nuncio-mail`: IMAP4rev1, JMAP (RFC 8620/8621), SMTP client engines.
- `crates/nuncio-cal`: CalDAV, iCalendar (RFC 5545) synchronization and event primitives.
- `crates/nuncio-store`: Encrypted local persistence and SQLite FTS5 full-text indexing.
- `crates/nuncio-tui`: Ratatui based terminal user interface binary.
- `crates/nuncio-gui`: Desktop GUI binary shell.

## High Level Roadmap Phases
1. Phase 1: Engine Foundation & Storage (Core, SQLite FTS5 schema, basic store)
2. Phase 2: Mail & Calendar Protocols (IMAP IDLE, JMAP, CalDAV sync engines)
3. Phase 3: Terminal UI (Ratatui interface for mail triage and calendar viewing)
4. Phase 4: Desktop GUI Shell & Cross-Platform Packaging (Windows, macOS, Linux targets)

## Open Questions
- Default GUI engine choice: Native GPU framework (Slint/Iced/GPUI) vs Tauri v2.
- Local encryption default: SQLCipher vs custom age/AES-GCM at rest payload encryption.
