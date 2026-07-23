# Nuncio Enhanced Production Roadmap (V1, V2, V3)

> **Zero-Mock Commercial Parity Roadmap & Micro-Feature Specification**  
> Benchmarked against Superhuman, Mimestream, Apple Mail, Outlook, HEY, Fantastical, Cron, and ProtonMail.

---

## Roadmap Phases Summary

| Release Phase | Target Audience | Key Feature Highlights |
| :--- | :--- | :--- |
| **V1 (GA Launch)** | Developers & Power Users | Full 4-shell presentation layer (`nuncio-cli`, `nuncio-tui`, `nuncio-gui`, `nuncio-mcp`), centralized `nunciod` daemon, IMAP/SMTP/CalDAV drivers, FTS5 trigram search (<3.2ms), OS keyring integration, zero `.unwrap()` runtime safety. |
| **V2 (Power User & Enterprise)** | Enterprise Teams & Security Professionals | JMAP RFC 8620/8621 native transport, OAuth2 PKCE token rotation, CardDAV contact sync, OpenPGP / S/MIME encryption, multi-timezone calendar overlays, 1-on-1 scheduling link generation, Zoom/Teams/Meet link detection. |
| **V3 (Platform & AI Automation)** | AI-Native Workflows & Developers | Local LLM agent auto-categorization & triage, automated meeting summaries, Lua/WASM plugin runtime, webhook workflow engine, age encrypted cloud sync relay. |

---

## Release Milestones (v0.1.0 to v1.0.0)

- **v0.1.0 (Core Library Engine)**: Hexagonal domain models, `EventBus`, SQLite WAL `DatabaseEngine`.
- **v0.2.0 (Storage & FTS5 Search)**: Trigram FTS5 search index, AES-256-GCM column encryption, age attachment stream ciphers.
- **v0.3.0 (IMAP & SMTP Drivers)**: `async-imap` TLS connections, IDLE real-time push, `lettre` SMTP MIME transport.
- **v0.4.0 (CalDAV & iCal Recurrence)**: CalDAV HTTP REPORT queries, iCalendar RFC 5545 parsing, `rrule` recurrence expansion.
- **v0.5.0 (Ratatui TUI Shell)**: 3-pane split-view terminal UI, Vim motions (`j`/`k`/`h`/`l`), HTML to terminal plaintext conversion.
- **v0.6.0 (Tauri v2 Desktop GUI)**: React 18 + Vite desktop UI, glassmorphic CSS, strict HTML `<iframe sandbox>` CSP enforcement.
- **v0.7.0 (Native MCP Server)**: Stdio JSON-RPC 2.0 MCP server for local LLM agents (`nuncio_mail_send`, `nuncio_cal_create_event`).
- **v0.8.0 (Hybrid Daemon & IPC)**: Central `nunciod` binary, 4-byte length-prefixed IPC framing codec, auto-spawning retry loop.
- **v1.0.0 (GA Release)**: Production packaging, installers, zero warnings, 100% test coverage gate.
