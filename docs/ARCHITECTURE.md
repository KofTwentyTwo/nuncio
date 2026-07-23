# Nuncio Architecture Specification

Nuncio ([nuncio.mx](https://nuncio.mx)) is a high-performance cross-platform mail and calendar client for macOS, Windows, and Linux. This document defines the system architecture, crate layout, data models, network protocol engines, storage strategy, presentation shell integrations, quality gates, and domain encapsulation boundaries.

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

## 2. Domain Encapsulation & Anti-Corruption Boundary (Zero Library Leakage)

To ensure third-party crates (e.g. `mail-parser`, `calcard`, `rrule`, `sqlx`, `keyring`, `age`, `jmap-client`) can be swapped or upgraded at any time without breaking Nuncio's data models or presentation shells, Nuncio enforces a **Hexagonal Ports & Adapters Architecture**:

```
┌───────────────────────────────────────────────────────────────────────────────────┐
│                          nuncio-core Domain Entities                              │
│       (Email, Folder, CalendarEvent, Contact, ExpandedOccurrence, SyncDelta)      │
└─────────────────────────────────────────▲─────────────────────────────────────────┘
                                          │ Implements Ports / Adapters
┌─────────────────────────────────────────┴─────────────────────────────────────────┐
│                       Anti-Corruption Layer (Adapters)                            │
│                                                                                   │
│  ┌─────────────────────────┐ ┌─────────────────────────┐ ┌─────────────────────┐  │
│  │ MimeParserAdapter       │ │ IcalParserAdapter       │ │ SqliteStorageAdapter│  │
│  │ (Wraps mail-parser)     │ │ (Wraps calcard & rrule) │ │ (Wraps sqlx & age)  │  │
│  └────────────┬────────────┘ └────────────┬────────────┘ └──────────┬──────────┘  │
└───────────────┼───────────────────────────┼─────────────────────────┼─────────────┘
                ▼                           ▼                         ▼
┌──────────────────────────┐ ┌──────────────────────────┐ ┌──────────────────────────┐
│    mail-parser Crate     │ │   calcard & rrule Crates │ │    sqlx & age Crates     │
│   (Implementation Detail)│ │   (Implementation Detail)│ │   (Implementation Detail)│
└──────────────────────────┘ └──────────────────────────┘ └──────────────────────────┘
```

### Strict Isolation Rules

1. **Zero External Types in `nuncio-core`**: No third-party struct, enum, or trait types (e.g., `mail_parser::Message`, `calcard::Calendar`, `rrule::RRuleSet`, `sqlx::SqlitePool`, `keyring::Entry`, `lettre::Message`) are ever exposed in `nuncio-core` struct fields or public API signatures.
2. **Domain Models Owned by Nuncio**: `nuncio-core` defines Nuncio's own pure Rust domain types (`Email`, `CalendarEvent`, `Contact`, `Folder`).
3. **Adapter Boundary**:
   - **MIME Parsing**: `mail-parser` is hidden behind an internal `MimeParserAdapter` function converting raw RFC 822/5322 byte slices (`&[u8]`) into `nuncio_core::model::Email`. Swapping `mail-parser` for another parser touches **only** the adapter implementation inside `nuncio-mail`.
   - **Calendar Parsing & Recurrences**: `calcard` and `rrule` are hidden behind `IcalParserAdapter` and `RruleRecurrenceAdapter`. Swapping either crate touches **only** `nuncio-cal`.
   - **Persistence & Vaults**: `sqlx`, `keyring`, and `age` are hidden behind `StorageEngine` and `SecretManager` adapter traits. Swapping SQLite or encryption algorithms touches **only** `nuncio-store`.

---

## 3. Workspace Crate Layout & Crate Selections

### Crate Ecosystem Selections

| Domain | Primary Crates | Technical Justification | Encapsulation Adapter |
| :--- | :--- | :--- | :--- |
| **Mail Parsing** | `mail-parser` (Stalwart Labs) | Zero-copy MIME parser using SIMD Base64 decoding and perfect hashing. | `MimeParserAdapter` inside `nuncio-mail`. |
| **Mail Protocols** | `jmap-client`, `jmap-proto`, `async-imap`, `lettre` | Dual JMAP / IMAP / SMTP engines. | `MailBackend` trait inside `nuncio-mail`. |
| **Calendar & Contacts** | `calcard` (Stalwart Labs), `rrule`, `chrono-tz` | `calcard` parses iCal/vCard to JSCalendar; `rrule` expands recurrences. | `IcalParserAdapter` & `RecurrenceAdapter` inside `nuncio-cal`. |
| **DAV Protocols** | `reqwest`, `quick-xml` | Custom WebDAV `PROPFIND` & CalDAV `sync-collection` REPORT queries. | `CalDavClient` trait inside `nuncio-cal`. |
| **Database & Storage** | `sqlx` (SQLite WAL), `age` | Async SQLite metadata persistence and `age` chunked attachment cipher. | `StorageRepository` trait inside `nuncio-store`. |
| **Full-Text Search** | SQLite FTS5 (`unicode61` + `trigram`) | Baseline ACID-compliant trigram search. Optional `tantivy` feature flag. | `SearchEngine` trait inside `nuncio-store`. |
| **OS Credentials** | `keyring` | Binds to macOS Keychain, Windows Credential Manager, Linux Secret Service. | `SecretVault` trait inside `nuncio-store`. |
| **Command Line Interface** | `clap` v4 | Fast subcommands parser supporting JSON output formatting. | Presentation Shell (`nuncio-cli`). |
| **Terminal Shell** | `ratatui`, `crossterm`, `html2text` | Double-buffered terminal renderer & `html2text` hyperlink parser. | Presentation Shell (`nuncio-tui`). |
| **Desktop Shell** | `Tauri v2` | Native OS webview sandboxed HTML email renderer and accessibility. | Presentation Shell (`nuncio-gui`). |
| **Mocking & Test Infra** | `wiremock`, `tempfile` | HTTP/JMAP/CalDAV mock servers and ephemeral test databases. | Test Infrastructure. |

---

## 4. Subsystem Architecture Specifications

### 4.1 `crates/nuncio-mail` Engine

`nuncio-mail` exposes a protocol-agnostic async trait (`MailBackend`) implemented by two engines: `JmapBackend` and `ImapBackend`.

```rust
use async_trait::async_trait;
use nuncio_core::model::{Email, Folder, SyncDelta};

#[async_trait]
pub trait MailBackend: Send + Sync {
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError>;
    async fn sync_messages(&self, folder_id: &str, since_state: Option<&str>) -> Result<SyncDelta<Email>, MailError>;
    async fn fetch_body(&self, email_id: &str) -> Result<Email, MailError>;
    async fn send_mail(&self, email: Email) -> Result<(), MailError>;
}
```

- **JMAP Engine**: Executes single-roundtrip differential updates using `Email/changes(sinceState)` and receives WebSocket push state changes.
- **IMAP Engine**: Uses a dual-socket connection manager. Connection A remains locked in `IDLE` listening for socket events. Connection B handles on-demand `FETCH`, `STORE`, and `SEARCH` requests.
- **MIME Parser Adapter**: Encapsulates `mail-parser` inside worker tasks, returning pure `nuncio_core::model::Email` entities.

### 4.2 `crates/nuncio-cal` Engine

`nuncio-cal` handles calendar synchronization and contact management across CalDAV (RFC 4791), CardDAV (RFC 6352), and JMAP Calendars.

- **Data Normalization**: `calcard` converts native `.ics` components into `JSCalendar` (RFC 8984) and `JSContact` (RFC 9553) models.
- **Recurrence Engine Adapter**: `rrule` processes master `RRULE` strings alongside `EXDATE` exclusions and detached `RECURRENCE-ID` override components over a finite window `[start_date, end_date]`.
- **Sync Protocol**: Uses WebDAV `sync-collection` (RFC 6578) with `sync-token` parameters to receive minimal deltas, falling back to `calendar-query` time-range REPORT requests.

### 4.3 `crates/nuncio-store` Storage & Security

`nuncio-store` manages local caching, full-text search indexing, and secret key storage.

- **Relational Metadata**: `sqlx` manages SQLite tables for mail envelopes, calendar events, contacts, and sync tokens in WAL mode (`PRAGMA journal_mode=WAL;`).
- **Full-Text Indexing**: SQLite FTS5 with `unicode61 remove_diacritics 2 porter` and `trigram` tokenizers provides transactional full-text search.
- **Payload & Attachment Encryption**: Raw MIME message bodies and binary attachments are encrypted on disk using `age` (X25519 / ChaCha20-Poly1305) with keys fetched from `keyring`.

---

## 5. Presentation Shell Specifications

### 5.1 Command Line Interface (`nuncio-cli`)

- **Execution Engine**: `clap` v4 derive macro.
- **Noun + Verb Command Standard**: Standardized `<Noun> <Verb> [Flags]` command hierarchy:
  - `nuncio account list | add | show`
  - `nuncio mail sync | list | read | send | search`
  - `nuncio folder list`
  - `nuncio cal list | sync`
  - `nuncio system status`
- **Pipeline & Scripting Support**: Accepts `--json` flag to stream machine-readable JSON payloads to stdout for processing with `jq`, `grep`, or automation pipelines. Global `--account <id>` flag allows targeting any specific account context.

### 5.2 Terminal UI Shell (`nuncio-tui`)

- **Rendering Engine**: `ratatui` v0.28+ with `crossterm` v0.28+.
- **HTML Email Strategy**: HTML email bodies are parsed with `html2text` and transformed into `ratatui::text::Text` structures. Links are assigned numeric footers (e.g. `o 1`) to trigger the OS default browser via `open::that`. Pressing `o` exports the message to an external browser or terminal pager (`w3m`).

### 5.3 Desktop GUI Shell (`nuncio-gui`)

- **Rendering Engine**: **Tauri v2** shell calling `nuncio-core` via IPC.
- **HTML Email Sandboxing**: Displays untrusted HTML emails inside isolated `<iframe sandbox="allow-same-origin" srcdoc="...">` tags with JavaScript execution disabled. Custom URI schemes (`nuncio-mail://`) proxy local attachments while blocking remote tracking pixels by default.
- **Accessibility & Native Integration**: Utilizes OS native browser accessibility engines (VoiceOver on macOS, NVDA/JAWS on Windows, Orca on Linux), OS system tray integration, and native notification APIs.

---

## 6. Testing, E2E & Protocol Mocking Standards

1. **100% Unit Test Line Coverage**: All engine domain logic, parsers, and recurrence algorithms require 100% line coverage (`cargo llvm-cov --workspace --fail-under-lines 100`).
2. **Integration Test Isolation**: Integration tests in `tests/` MUST use ephemeral databases (`tempfile` or `:memory:`) with zero state leakage.
3. **Headless E2E Test Suite**: E2E tests validate complete user workflows from `nuncio-cli` subcommands down through core engine streams and storage persistence.
4. **100% Offline Mocks for External Systems**:
   - Network protocols (JMAP, IMAP, CalDAV, CardDAV, SMTP) are 100% mocked via `wiremock` and async mock traits.
   - OS native vaults are mocked via `MockKeyring` in-memory provider.
   - Live network connections during testing are strictly forbidden.
