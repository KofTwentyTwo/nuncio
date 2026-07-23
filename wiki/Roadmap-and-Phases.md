# Nuncio Roadmap and Project Phases

Nuncio ([nuncio.mx](https://nuncio.mx)) development is organized into 4 primary roadmap phases executed via a **Multi-Agent Architecture**:
- **Master Orchestrator**: Antigravity (`agy`)
- **Engine & Core Subagent**: Claude Code (`claude`)
- **CLI & Automation Subagent**: OpenAI Codex (`codex`)
- **UI Shells & CI/CD Subagent**: Antigravity Worker (`agy-worker`)

---

## Phase 1: Core Engine & SQLite FTS5 Storage
- `nuncio-core` workspace engine setup, configuration schema, and Tokio async event bus -> **Claude Code Subagent**
- `nuncio-store` SQLite database persistence with WAL mode -> **Claude Code Subagent**
- SQLite FTS5 virtual table schemas (`unicode61` + `trigram`) for mail envelopes and calendar events -> **Claude Code Subagent**
- `keyring` integration for OS Keychain / Credential Manager secrets -> **Claude Code Subagent**
- `age` payload encryption for disk attachment blobs -> **Claude Code Subagent**

## Phase 2: Protocol Engine Libraries
- `nuncio-mail`:
  - `mail-parser` integration for zero-copy MIME parsing -> **Claude Code Subagent**
  - `jmap-client` + `jmap-proto` JMAP (RFC 8620/8621) sync engine over HTTP/2 and WebSockets -> **Claude Code Subagent**
  - `async-imap` IMAP4rev1 dual-socket connection manager (IDLE push stream + fetch pool) -> **Claude Code Subagent**
  - `lettre` async Tokio SMTP transport -> **Claude Code Subagent**
- `nuncio-cal`:
  - `calcard` iCalendar (RFC 5545) and vCard (RFC 6350) parsing to JSCalendar (RFC 8984) -> **Claude Code Subagent**
  - `rrule` recurrence expansion engine -> **Claude Code Subagent**
  - WebDAV / CalDAV (RFC 4791) `sync-collection` and `calendar-query` REPORT client via `reqwest` and `quick-xml` -> **Claude Code Subagent**
- Protocol server mocks (`wiremock`, `MockMailBackend`, `MockKeyring`) -> **Claude Code Subagent**

## Phase 3: Command Line & Terminal User Interfaces (CLI & TUI Shells)
- `nuncio-cli` built on `clap` v4 for subcommand parsing (`mail`, `cal`, `sync`, `status`) and JSON output formatting -> **OpenAI Codex Subagent**
- `nuncio-tui` built on `ratatui` v0.28+ and `crossterm` v0.28+ -> **Antigravity Worker Subagent**
- Keyboard-first mail thread navigation, composer, and folder trees -> **Antigravity Worker Subagent**
- `html2text` HTML email rendering into formatted terminal text with link footers -> **Antigravity Worker Subagent**
- Calendar day, week, and month grid layouts -> **Antigravity Worker Subagent**

## Phase 4: Desktop GUI Shell & Cross-Platform Packaging
- `nuncio-gui` built on `Tauri v2` wrapper calling `nuncio-core` via IPC -> **Antigravity Worker Subagent**
- Untrusted HTML email sandboxing (`<iframe sandbox>` with custom `nuncio-mail://` URI scheme) -> **Antigravity Worker Subagent**
- Native OS accessibility tree integration (VoiceOver, NVDA/JAWS, Orca) -> **Antigravity Worker Subagent**
- Windows (`.msi` / `.exe`), macOS (`.dmg`), and Linux (`.AppImage` / `.deb`) CI automated release pipelines -> **Antigravity Master Agent (agy)**

---

## Project Tracking Links
- GitHub Issues: [KofTwentyTwo/nuncio/issues](https://github.com/KofTwentyTwo/nuncio/issues)
- GitHub Project Board: [Nuncio Roadmap Project #5](https://github.com/users/KofTwentyTwo/projects/5)
