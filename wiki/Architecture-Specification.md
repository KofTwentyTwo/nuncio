# Nuncio Architecture Specification

Nuncio ([nuncio.mx](https://nuncio.mx)) is a high-performance cross-platform mail and calendar client for macOS, Windows, and Linux. This document defines the system architecture, crate layout, data models, network protocol engines, storage strategy, presentation shell integrations, and quality gates.

---

## 1. Architectural Philosophy: Library-First ("Ghost" Model)

Nuncio decouples application state management, network protocol engines, cryptography, and storage persistence from presentation interfaces.

```
┌───────────────────────────────────────────────────────────────────────────────────┐
│                                Presentation Shells                                │
│                                                                                   │
│ ┌────────────────────────┐   ┌─────────────────────────┐   ┌────────────────────┐ │
│ │   nuncio-cli (Clap)    │   │  nuncio-tui (Ratatui)   │   │nuncio-gui (Tauri v2│ │
│ └───────────┬────────────┘   └────────────┬────────────┘   └─────────┬──────────┘ │
└─────────────┼─────────────────────────────┼──────────────────────────┼────────────┘
              │ CoreCommand (mpsc)          │ CoreCommand (mpsc)       │ CoreCommand (mpsc)
              ▼ CoreState (watch)           ▼ CoreState (watch)        ▼ CoreState (watch)
┌───────────────────────────────────────────────────────────────────────────────────┐
│                                    nuncio-core                                    │
│                           Async Event Bus & Orchestrator                          │
└──────────────┬────────────────────────────┬───────────────────────┬───────────────┘
               │                            │                       │
               ▼                            ▼                       ▼
┌──────────────────────────┐  ┌──────────────────────────┐  ┌──────────────────────────┐
│       nuncio-mail        │  │        nuncio-cal        │  │       nuncio-store       │
│    JMAP / IMAP / SMTP    │  │      CalDAV / iCal       │  │     SQLite FTS5 / Age    │
└──────────────────────────┘  └──────────────────────────┘  └──────────────────────────┘
```

- **Headless Engine Core**: `nuncio-core`, `nuncio-mail`, `nuncio-cal`, and `nuncio-store` contain 100% of domain rules, protocol parsing, offline caching, search indexing, and secret key encryption.
- **Thin Presentation Shells**: `nuncio-cli` (command line), `nuncio-tui` (terminal), and `nuncio-gui` (desktop) are stateless interfaces consuming unidirectional state streams (`tokio::sync::watch`) and dispatching command enums (`tokio::sync::mpsc`).

---

## 2. Workspace Crate Layout & Crate Selections

### Crate Ecosystem Selections

| Domain | Primary Crates | Technical Justification |
| :--- | :--- | :--- |
| **Mail Parsing** | `mail-parser` (Stalwart Labs) | Zero-copy MIME parser using SIMD Base64 decoding and perfect hashing; outputs RFC 8621 compliant structures. |
| **Mail Protocols** | `jmap-client`, `jmap-proto`, `async-imap`, `lettre` | Dual protocol support: JMAP (RFC 8620/8621) via HTTP/2 and SSE streams; legacy IMAP4rev1 IDLE via `async-imap`; SMTP via `lettre`. |
| **Calendar & Contacts** | `calcard` (Stalwart Labs), `rrule`, `chrono-tz` | `calcard` parses RFC 5545 iCal & RFC 6350 vCard to JSCalendar; `rrule` expands complex recurrences; `chrono-tz` maps IANA timezones. |
| **DAV Protocols** | `reqwest`, `quick-xml` | Custom HTTP method execution (`PROPFIND`, `REPORT`) with `quick-xml` Serde deserialization of `<multistatus>` payloads. |
| **Database & Storage** | `sqlx` (SQLite WAL), `age` | `sqlx` in Write-Ahead Logging mode for async metadata; `age` chunked streaming cipher for encrypted attachments. |
| **Full-Text Search** | SQLite FTS5 (`unicode61` + `trigram`) | Baseline ACID-compliant trigram search. Optional `tantivy` feature flag for high-volume mailboxes (>50k emails). |
| **OS Credentials** | `keyring` | Binds to macOS Keychain Services, Windows Credential Manager, and Linux Secret Service / D-Bus. |
| **Command Line Interface** | `clap` v4 | Fast subcommands parser supporting JSON output formatting for shell scripts and Unix piping. |
| **Terminal Shell** | `ratatui`, `crossterm`, `html2text` | Double-buffered terminal renderer; `html2text` converts HTML emails into formatted text with link references. |
| **Desktop Shell** | `Tauri v2` | Leverages OS native webviews (`WKWebView`, `WebView2`, `WebKitGTK`) for sandboxed, secure HTML email rendering and accessibility. |

---

## 3. Quality & Coverage Enforcement Gates

- **Compiler & Linter Enforcement**: `.cargo/config.toml` sets `rustflags = ["-D", "warnings"]`. All compiler and clippy warnings are treated as hard errors during normal local development.
- **100% Line Coverage Requirement**: **100% line coverage** is required for all unit tests across the workspace (`cargo llvm-cov --workspace --fail-under-lines 100`).
- **Automated CI Pipeline**: GitHub Actions [.github/workflows/ci.yml](file:///R:/Git.Local/KofTwentyTwo/nuncio/.github/workflows/ci.yml) enforces `fmt`, `clippy`, `test`, 100% line coverage, and cross-platform matrix compilation on Linux, macOS, and Windows.
