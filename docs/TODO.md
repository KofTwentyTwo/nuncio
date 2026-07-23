# TODO: Nuncio Zero-Mock Production Roadmap & Task Tracking

> **Authoritative Single Source of Truth**: All task tracking, issue status, and milestone progress are maintained in [docs/PLAN-production-roadmap-100-plus.md](file:///R:/Git.Local/KofTwentyTwo/nuncio/docs/PLAN-production-roadmap-100-plus.md) and on GitHub:
> - **GitHub Project Board**: [Nuncio Roadmap Project #5](https://github.com/users/KofTwentyTwo/projects/5)
> - **GitHub Milestones**: [KofTwentyTwo/nuncio/milestones](https://github.com/KofTwentyTwo/nuncio/milestones) (`v0.1.0` through `v3.0.0`)
> - **GitHub Issues**: [KofTwentyTwo/nuncio/issues](https://github.com/KofTwentyTwo/nuncio/issues)

---

## Active Phase: Zero-Mock Production Engineering (#101 – #260)

### Epic 1: Real Protocol Drivers & Live Sync Engine (#101 – #125) - `COMPLETED`
- [x] `#101`: Implement live IMAP connection pool with TLS handshake via `async-imap` & `tokio-rustls`.
- [x] `#102`: Implement IMAP IDLE push notification listener loop for real-time inbox updates.
- [x] `#103`: Implement incremental IMAP UID FETCH message sync algorithm storing flags in SQLite.
- [x] `#104`: Implement IMAP mailbox list parser mapping RFC 3501 hierarchy to domain `Folder` entities.
- [x] `#105`: Implement IMAP draft, sent, trash, and flag state mutations (`STORE +FLAGS`, `COPY`, `EXPUNGE`).
- [x] `#106`: Implement live SMTP transport client using `lettre` with STARTTLS and Implicit TLS support.
- [x] `#107`: Implement MIME message builder for multipart/alternative (plain text + HTML + attachments).
- [x] `#108`: Implement live SMTP delivery confirmation and queue retry mechanism with backoff.
- [x] `#109`: Implement JMAP Session resource discovery (`/.well-known/jmap`) and API token authentication.
- [x] `#110`: Implement JMAP `Email/get` and `Email/changes` push synchronization engine.
- [x] `#112`: Implement CalDAV WebDAV report query builder for time-range event filtering (`RFC 4791`).
- [x] `#113`: Implement CalDAV multi-status XML parser extracting `VEVENT` data using `quick-xml`.

### Epic 2: Storage & Data-at-Rest Security (#126 – #145) - `COMPLETED`
- [x] `#126`: Implement SQLite WAL journal mode pragmas and connection pool manager in `nuncio-store`.
- [x] `#127`: Implement PBKDF2/Argon2id key derivation module mapping OS vault secrets to AES keys.
- [x] `#128`: Implement transparent column-level message body encryption/decryption in SQLite queries.
- [x] `#129`: Implement `ZeroizeOnDrop` credential memory hygiene for all account configuration fields.
- [x] `#130`: Implement SQLite FTS5 full-text search index setup with trigram tokenizer for email bodies.
- [x] `#131`: Implement FTS5 query builder with prefix search and quote escaping.

### Epic 3: TUI Interactive Terminal App (#146 – #170) - `COMPLETED`
- [x] `#146`: Connect `TuiApp` to live `DatabaseEngine` and `EventBus` channels.
- [x] `#147`: Implement dynamic folder list rendering from SQLite in TUI Sidebar.
- [x] `#148`: Implement live message list rendering for selected folder with unread indicators.
- [x] `#149`: Implement dynamic email reader view rendering plain text body from SQLite.
- [x] `#152`: Implement interactive compose email modal with multi-line text input fields.
- [x] `#153`: Implement interactive reply / reply-all modal populating subject and recipient headers.

### Epic 4: Native Desktop GUI (Tauri v2 + React/Vite) (#171 – #200) - `COMPLETED`
- [x] `#171`: Initialize Tauri v2 application framework structure in `crates/nuncio-gui/src-tauri`.
- [x] `#172`: Initialize React + Vite + TypeScript frontend project structure in `crates/nuncio-gui/ui`.
- [x] `#173`: Configure native window windowing rules (title bar, minimum dimensions 1024x768).
- [x] `#174`: Implement Tauri v2 IPC commands (`#[tauri::command]`) linking React to `IpcBridge`.
- [x] `#175`: Implement React split-view layout component (Sidebar, MessageList, Reader).
- [x] `#176`: Implement Sandboxed HTML email iframe renderer component with strict CSP.
- [x] `#177`: Implement Account Settings & Connectivity Manager modal with Add, Edit, Delete, Test.

### Epic 5: CLI Live Pipeline & Interactive Inputs (#201 – #220) - `COMPLETED`
- [x] `#201`: Implement interactive password prompt using `rpassword` when password is not in keyring.
- [x] `#202`: Connect `nuncio mail sync` to live IMAP/JMAP background sync workers.
- [x] `#203`: Connect `nuncio account add` to live validation checking IMAP/SMTP connectivity.

### Epic 6: Live E2E Integration & System Test Suite (#221 – #240) - `COMPLETED`
- [x] `#237`: Implement GUI IPC contract test verifying all JSON payloads validate against schema.
- [x] `#240`: Integrate full E2E test matrix into GitHub Actions CI workflow.

### Epic 7: Packaging, Installers & CI/CD (#241 – #260) - `PLANNED`
- [ ] `#241`: Configure GitHub Actions build workflow matrix for Windows, macOS, and Linux.
- [ ] `#242`: Build Windows WiX installer (`.msi`) for `nuncio-gui` and `nuncio-cli`.
- [ ] `#244`: Build macOS Apple Silicon (ARM64) and Intel (x64) `.dmg` disk image packages.
- [ ] `#246`: Build Linux AppImage standalone executable package.
