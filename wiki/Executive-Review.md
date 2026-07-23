# Nuncio Executive Architectural, Engineering & Product Review

Nuncio ([nuncio.mx](https://nuncio.mx)) is a high-performance cross-platform mail and calendar client for macOS, Windows, and Linux. This document synthesizes the formal evaluations conducted by a **Senior Principal Software Engineer**, **Senior Enterprise Systems Architect**, and **Senior Technical Product Manager**.

---

## Executive Summary & Verdict

| Panelist | Verdict | Core Finding / Key Recommendation |
| :--- | :--- | :--- |
| **Senior Principal Software Engineer** | **Approved with Engineering Remediation** | Mandate bounded `mpsc(100)` channels, zero-copy `bytes::Bytes` in domain models, explicit SQLite pragmas (`WAL`, `synchronous=NORMAL`, `busy_timeout=5000`), finite `rrule` window limits, and `tempfile` for WAL integration testing. |
| **Senior Enterprise Systems Architect** | **Approved with Structural Refinements** | Enforce CSP headers on Tauri v2 HTML webviews to block tracking pixels, SQLCipher / WAL index encryption at rest, Argon2id file vault fallback for headless SSH sessions, and channel split (`watch` for state vs `mpsc`/`broadcast` for events). |
| **Senior Technical Product Manager** | **Approved with Milestone Calibration** | Decompose roadmap into 10 atomic micro-milestones (v0.1.1 to v1.0.0), define a unified `AccountConfig` schema in `nuncio-core`, and implement an offline transactional Outbox queue in `nuncio-store`. |

---

## 1. Domain Encapsulation & Memory Optimization

- **Zero Third-Party Library Leakage**: The Hexagonal Ports & Adapters Architecture successfully isolates `mail-parser`, `calcard`, `rrule`, `sqlx`, `keyring`, `age`, and `jmap-client` behind internal adapters (`MimeParserAdapter`, `IcalParserAdapter`, `SqliteStorageAdapter`). No third-party structs appear in `nuncio-core` signatures.
- **Zero-Copy Memory Preservation**: To prevent adapter mapping from invalidating `mail-parser` zero-copy SIMD performance, `nuncio_core::model::Email` will use `bytes::Bytes` for raw MIME payloads and `Arc<str>` for body strings across async task bounds.

---

## 2. Concurrency, Tokio Channels & State Management

- **Channel Split (State vs Events)**:
  - `tokio::sync::watch` is lossy by design and MUST be used exclusively for continuous presentation state updates (`AppState`, `SyncStatus`, `FolderTree`).
  - Discrete transactional events (`CoreEvent::MessageReceived`, `CoreEvent::AuthFailed`) MUST use `tokio::sync::broadcast` or bounded `mpsc` channels to prevent lost notifications.
- **Command Channel Backpressure**: All `CoreCommand` channels MUST be bounded (`mpsc::channel(100)`). The presentation layer must drop redundant UI input floods or throttle keyboard events.

---

## 3. Database Pragmas, FTS5 Search & Encrypted Storage

- **SQLite Connection Pragmas**: `nuncio-store` MUST configure `sqlx::sqlite::SqliteConnectOptions` upon pool creation with:
  ```sql
  PRAGMA journal_mode = WAL;
  PRAGMA synchronous = NORMAL;
  PRAGMA busy_timeout = 5000;
  PRAGMA foreign_keys = ON;
  PRAGMA cache_size = -64000;
  ```
- **FTS5 Bulk Sync Optimization**: Initial sync of large mailboxes (50k+ emails) will bypass automatic FTS5 triggers, performing batch insertion into primary envelope tables first, followed by background batch indexing within single transactions.
- **Data-at-Rest Protection**: Disk encryption covers SQLite metadata files via SQLCipher / WAL encryption, sensitive tokens via `keyring`, and attachments via chunked `age` streaming ciphers.
- **Headless Secret Vault Fallback**: For headless Linux servers or SSH sessions where D-Bus SecretService is absent, `nuncio-store` provides an Argon2id + AES-256-GCM passphrase file vault fallback (`FileSecretVault`).

---

## 4. Security & Webview Sandboxing

- **HTML Email Privacy & Tracking Pixel Defense**:
  - In `nuncio-gui` (Tauri v2), HTML email renders inside isolated `<iframe sandbox="allow-same-origin" srcdoc="...">` tags with JavaScript disabled.
  - A strict Content Security Policy (`default-src 'none'; img-src nuncio-mail:; style-src 'unsafe-inline';`) blocks remote tracking pixels and external HTTP image beacons by default.
  - Custom protocol scheme handlers (`nuncio-mail://`) proxy local attachments.
- **TUI URL Sanitization**: In `nuncio-tui`, opening URLs via `open::that` enforces strict scheme whitelisting (`http://`, `https://`, `mailto:`), blocking malicious local URI executions (`file://`, `javascript:`).

---

## 5. Testing, E2E & Protocol Mocking Strategy

- **100% Unit Test Line Coverage**: Enforced via `cargo-llvm-cov --workspace --fail-under-lines 100`. Crate roots include `#![deny(clippy::unwrap_used, clippy::expect_used)]`.
- **Integration Test Ephemeral Storage**: Integration tests in `tests/` MUST use `tempfile::tempdir()` rather than `:memory:`. SQLite `:memory:` connections isolate data per connection, whereas `tempfile` accurately tests multi-connection WAL mode behaviors.
- **100% Offline Protocol Mocking**:
  - HTTP protocols (JMAP, CalDAV, CardDAV) are mocked using `wiremock`.
  - TCP protocols (IMAP, SMTP) are mocked using `MockMailBackend` trait implementations and Tokio socket harnesses. Live network calls during testing are strictly forbidden.

---

## 6. Atomic Micro-Milestone Roadmap (v0.1.1 to v1.0.0)

| Milestone | Version Tag | Granular Focus | Primary Deliverable | Assigned Agent |
| :--- | :--- | :--- | :--- | :--- |
| **M1.1** | `v0.1.1` | Event Bus Channels | Bounded `mpsc(100)` command channel & `watch` state stream in `nuncio-core`. | `claude` |
| **M1.2** | `v0.1.2` | Config & Account Schema | `AccountConfig` Serde models & validation rules in `nuncio-core`. | `claude` |
| **M1.3** | `v0.1.3` | SQLite Engine & WAL | `sqlx` SQLite pool with WAL pragmas & migrations in `nuncio-store`. | `claude` |
| **M1.4** | `v0.1.4` | SQLite FTS5 Indexing | FTS5 virtual table schemas (`unicode61` + `trigram`) & triggers in `nuncio-store`. | `claude` |
| **M1.5** | `v0.1.5` | OS Vault Integration | `SecretManager` connecting to `keyring` with Argon2id file fallback. | `claude` |
| **M1.6** | `v0.1.6` | Attachment Encryption | `PayloadCipher` (AES-256-GCM) & chunked `age` streaming cipher in `nuncio-store`. | `claude` |
| **M2.1** | `v0.2.1` | Zero-Copy MIME Parser | `mail-parser` integration with `bytes::Bytes` buffer sharing in `nuncio-mail`. | `claude` |
| **M2.2** | `v0.2.2` | Outbound SMTP Transport | `lettre` async Tokio SMTP client with connection pooling in `nuncio-mail`. | `claude` |
| **M2.3** | `v0.2.3` | JMAP Protocol Client | `jmap-client` `Email/changes` single-roundtrip differential sync in `nuncio-mail`. | `claude` |
| **M2.4** | `v0.2.4` | IMAP Dual-Socket Manager | `async-imap` dual-connection manager (IDLE push stream + fetch pool) in `nuncio-mail`. | `claude` |
| **M2.5** | `v0.2.5` | Protocol Server Mocks | `wiremock` server mocks & `MockMailBackend` for offline integration tests. | `claude` |
| **M3.1** | `v0.3.1` | iCal/vCard to JSCalendar | `calcard` normalization converting `.ics` and `.vcf` to JSCalendar in `nuncio-cal`. | `claude` |
| **M3.2** | `v0.3.2` | Recurrence Expansion | `rrule` master pattern expansion & `EXDATE` filtering over finite window `[start, end]`. | `claude` |
| **M3.3** | `v0.3.3` | CalDAV REPORT Client | `reqwest` + `quick-xml` WebDAV `sync-collection` & `calendar-query` REPORT client. | `claude` |
| **M3.4** | `v0.3.4` | CalDAV Server Mocks | `wiremock` CalDAV server mocks for offline calendar tests. | `claude` |
| **M4.1** | `v0.4.1` | CLI Subcommand Parser | `clap` v4 subcommands (`mail`, `cal`, `sync`, `status`) in `nuncio-cli`. | `codex` |
| **M4.2** | `v0.4.2` | JSON Pipeline Formatting | `--json` output formatting and stdout streaming for Unix piping in `nuncio-cli`. | `codex` |
| **M4.3** | `v0.4.3` | Headless CLI E2E Tests | Headless E2E integration test suite executing CLI commands against mocked engine. | `codex` |
| **M5.1** | `v0.5.1` | TUI Layout & Navigation | `ratatui` + `crossterm` frame layouts, navigation state, and folder sidebar in `nuncio-tui`. | `agy-worker` |
| **M5.2** | `v0.5.2` | Mail Viewer & Composer | Mail thread list, message viewer, and composer screens in `nuncio-tui`. | `agy-worker` |
| **M5.3** | `v0.5.3` | HTML Email Text Renderer | `html2text` converter converting HTML email bodies into formatted text in `nuncio-tui`. | `agy-worker` |
| **M5.4** | `v0.5.4` | Calendar Grid Views | Day, week, and month calendar grid layouts in `nuncio-tui`. | `agy-worker` |
| **M6.1** | `v0.6.1` | Tauri v2 IPC Bridge | `Tauri v2` desktop GUI wrapper calling `nuncio-core` IPC commands in `nuncio-gui`. | `agy-worker` |
| **M6.2** | `v0.6.2` | Untrusted HTML Sandboxing | Isolated `<iframe sandbox>` tags with CSP headers & custom `nuncio-mail://` scheme. | `agy-worker` |
| **M6.3** | `v0.6.3` | Native OS Integrations | Native OS accessibility trees (VoiceOver, NVDA, Orca), system tray, notifications. | `agy-worker` |
| **M7.1** | `v0.7.1` | Full Workspace E2E Suite | Workspace-wide integration test suite with 100% unit test line coverage verification. | `agy` (Master) |
| **M7.2** | `v0.7.2` | Security & Memory Audit | Secret scanning, CSP header validation, and memory leak profiling. | `agy` (Master) |
| **M1.0** | `v1.0.0` | Production Release | GitHub Actions automated release pipeline (`.msi`, `.dmg`, `.AppImage`) & v1.0.0 binaries. | `agy` (Master) |
