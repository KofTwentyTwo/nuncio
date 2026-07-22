# Nuncio Architecture Specification

Nuncio ([nuncio.mx](https://nuncio.mx)) is a high-performance cross-platform mail and calendar client for macOS, Windows, and Linux. This document defines the system architecture, crate layout, data models, network protocol engines, storage strategy, and presentation shell integrations.

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

## 3. Subsystem Architecture Specifications

### 3.1 `crates/nuncio-mail` Engine

`nuncio-mail` exposes a protocol-agnostic async trait (`MailBackend`) implemented by two engines: `JmapBackend` and `ImapBackend`.

```rust
use async_trait::async_trait;

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
- **MIME Parser**: `mail-parser` parses incoming RFC 8322 byte slices in worker threads, returning structured envelope payloads without AST allocations.

### 3.2 `crates/nuncio-cal` Engine

`nuncio-cal` handles calendar synchronization and contact management across CalDAV (RFC 4791), CardDAV (RFC 6352), and JMAP Calendars.

- **Data Normalization**: `calcard` converts native `.ics` components into `JSCalendar` (RFC 8984) and `JSContact` (RFC 9553) models.
- **Recurrence Engine**: `rrule` processes master `RRULE` strings alongside `EXDATE` exclusions and detached `RECURRENCE-ID` override components over a finite window `[start_date, end_date]`.
- **Sync Protocol**: Uses WebDAV `sync-collection` (RFC 6578) with `sync-token` parameters to receive minimal deltas, falling back to `calendar-query` time-range REPORT requests.

### 3.3 `crates/nuncio-store` Storage & Security

`nuncio-store` manages local caching, full-text search indexing, and secret key storage.

```
+--------------------------------------------------------------------+
|                         OS Native Vault                            |
|    (macOS Keychain / Windows Credential Manager / Linux D-Bus)     |
+--------------------------------------------------------------------+
                                  │
                       Keyring API (`keyring`)
                                  ▼
┌────────────────────────────────────────────────────────────────────┐
|                     nuncio-store Key Hierarchy                     |
|                                                                    |
|  1. Account Credentials: "nuncio/imap/user@domain.com"             |
|  2. Master Encryption Key: "nuncio/db-dek"                         |
└────────────────────────────────────────────────────────────────────┘
```

- **Relational Metadata**: `sqlx` manages SQLite tables for mail envelopes, calendar events, contacts, and sync tokens in WAL mode (`PRAGMA journal_mode=WAL;`).
- **Full-Text Indexing**: SQLite FTS5 with `unicode61 remove_diacritics 2 porter` and `trigram` tokenizers provides transactional full-text search.
- **Payload & Attachment Encryption**: Raw MIME message bodies and binary attachments are encrypted on disk using `age` (X25519 / ChaCha20-Poly1305) with keys fetched from `keyring`.

---

## 4. Presentation Shell Specifications

### 4.1 Command Line Interface (`nuncio-cli`)

- **Execution Engine**: `clap` v4 derive macro.
- **Subcommands**: Supports `nuncio mail list|read|send`, `nuncio cal list|add`, `nuncio sync`, and `nuncio status`.
- **Pipeline Support**: Accepts `--json` flag to stream structured JSON outputs to stdout for processing with `jq`, `grep`, or shell automation scripts.

### 4.2 Terminal UI Shell (`nuncio-tui`)

- **Rendering Engine**: `ratatui` v0.28+ with `crossterm` v0.28+.
- **HTML Email Strategy**: HTML email bodies are parsed with `html2text` and transformed into `ratatui::text::Text` structures. Links are assigned numeric footers (e.g. `o 1`) to trigger the OS default browser via `open::that`. Pressing `o` exports the message to an external browser or terminal pager (`w3m`).

### 4.3 Desktop GUI Shell (`nuncio-gui`)

- **Rendering Engine**: **Tauri v2** shell calling `nuncio-core` via IPC.
- **HTML Email Sandboxing**: Displays untrusted HTML emails inside isolated `<iframe sandbox="allow-same-origin" srcdoc="...">` tags with JavaScript execution disabled. Custom URI schemes (`nuncio-mail://`) proxy local attachments while blocking remote tracking pixels by default.
- **Accessibility & Native Integration**: Utilizes OS native browser accessibility engines (VoiceOver on macOS, NVDA/JAWS on Windows, Orca on Linux), OS system tray integration, and native notification APIs.

---

## 5. Event Loop & State Binding

Communication between presentation shells and `nuncio-core` relies on thread-safe async channels:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CoreCommand {
    SelectAccount(String),
    SelectFolder { account_id: String, folder_id: String },
    FetchMessageBody { message_id: String },
    SendMail(OutgoingMessage),
    TriggerSync,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CoreEvent {
    AccountUpdated(AccountState),
    MailReceived { folder_id: String, message_id: String },
    SyncProgress { total: u32, current: u32, message: String },
    Error { code: String, message: String },
}

pub struct CoreEngine {
    command_tx: tokio::sync::mpsc::UnboundedSender<CoreCommand>,
    state_rx: tokio::sync::watch::Receiver<AppState>,
    event_tx: tokio::sync::broadcast::Sender<CoreEvent>,
}
```

Presentation shells send commands via `command_tx` and receive reactive state updates through `state_rx`.
