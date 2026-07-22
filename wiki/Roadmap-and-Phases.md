# Nuncio Roadmap and Project Phases

Nuncio ([nuncio.mx](https://nuncio.mx)) development is organized into 4 primary roadmap phases.

---

## Phase 1: Core Engine & SQLite FTS5 Storage
- `nuncio-core` workspace engine setup, configuration schema, and Tokio async event bus.
- `nuncio-store` SQLite database persistence with WAL mode.
- SQLite FTS5 virtual table schemas (`unicode61` + `trigram`) for mail envelopes and calendar events.
- `keyring` integration for OS Keychain / Credential Manager secrets.
- `age` payload encryption for disk attachment blobs.

## Phase 2: Protocol Engine Libraries
- `nuncio-mail`:
  - `mail-parser` integration for zero-copy MIME parsing.
  - `jmap-client` + `jmap-proto` JMAP (RFC 8620/8621) sync engine over HTTP/2 and WebSockets.
  - `async-imap` IMAP4rev1 dual-socket connection manager (IDLE push stream + fetch pool).
  - `lettre` async Tokio SMTP transport.
- `nuncio-cal`:
  - `calcard` iCalendar (RFC 5545) and vCard (RFC 6350) parsing to JSCalendar (RFC 8984).
  - `rrule` recurrence expansion engine.
  - WebDAV / CalDAV (RFC 4791) `sync-collection` and `calendar-query` REPORT client via `reqwest` and `quick-xml`.

## Phase 3: Terminal User Interface (TUI Shell)
- `nuncio-tui` built on `ratatui` v0.28+ and `crossterm` v0.28+.
- Keyboard-first mail thread navigation, composer, and folder trees.
- `html2text` HTML email rendering into formatted terminal text with link footers.
- Calendar day, week, and month grid layouts.

## Phase 4: Desktop GUI Shell & Cross-Platform Packaging
- `nuncio-gui` built on `Tauri v2` wrapper calling `nuncio-core` via IPC.
- Untrusted HTML email sandboxing (`<iframe sandbox>` with custom `nuncio-mail://` URI scheme).
- Native OS accessibility tree integration (VoiceOver, NVDA/JAWS, Orca).
- Windows (`.msi` / `.exe`), macOS (`.dmg`), and Linux (`.AppImage` / `.deb`) CI automated release pipelines.

---

## Project Tracking Links
- GitHub Issues: [KofTwentyTwo/nuncio/issues](https://github.com/KofTwentyTwo/nuncio/issues)
- GitHub Project Board: [Nuncio Roadmap Project #5](https://github.com/users/KofTwentyTwo/projects/5)
