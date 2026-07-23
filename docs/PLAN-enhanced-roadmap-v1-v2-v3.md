# Nuncio Enhanced Production Roadmap (V1, V2, V3)

> **Authoritative Master Roadmap & Feature Specification**
> Developed by Master Orchestrator Agent and Senior PM Subagents (Mail, Calendar, Security & Platform).
> Benchmarked against Superhuman, Mimestream, Apple Mail, Outlook, HEY, Fastmail, ProtonMail, Cron / Notion Calendar, Fantastical, Thunderbird, Obsidian, Raycast, and YubiKey.

---

## Executive Summary & Commercial Benchmark Matrix

Nuncio is designed as a library-first, zero-mock, ultra-high-speed Mail, Calendar, and Contacts suite built in Rust with presentation shells for CLI, Ratatui TUI, and Tauri v2 Desktop GUI (React + Vite + TypeScript).

### Market Benchmark Matrix

| Feature / Subsystem | Superhuman / Mimestream | Fantastical / Cron | Thunderbird / Proton | Nuncio Target Architecture |
| :--- | :--- | :--- | :--- | :--- |
| **Transport Protocols** | Gmail API / IMAP / JMAP | CalDAV / WebDAV / Google | IMAP / OpenPGP / E2EE | Native Rust JMAP (RFC 8620/8621), IMAP4rev1 (QRESYNC/IDLE), SMTP, CalDAV (RFC 4791), CardDAV |
| **Keyboard Ergonomics** | Benchmark Vim / Cmd+K palette | Basic hotkeys / NLP | Standard legacy shortcuts | Vim motion navigation, Cmd+K fuzzy command palette, single-frame (<16ms) optimistic UI mutations |
| **Data Storage & Search** | Local cache / sub-50ms search | Local cache / Google search | Local SQLite DB / slow search | Local SQLite WAL + SQLCipher + Tantivy / FTS5 trigram search (<10ms across 100k messages) |
| **Security & Privacy** | Remote proxy image shield | Basic privacy | OpenPGP / Hardware tokens | OS Keyring + Argon2id fallback, OpenPGP (RFC 3156), S/MIME (RFC 8551), PKCS#11 YubiKey, strict HTML CSP iframe sandbox |
| **Extensibility & CLI** | Proprietary SaaS / No CLI | Proprietary / No CLI | Extension APIs / No CLI | Pure POSIX Noun + Verb CLI (`--json`), headless `nunciod` daemon, UNIX socket/Named Pipe IPC, QuickJS/Wasmtime plugins |

---

## Detailed Version Release Breakdown

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                   RELEASE ROADMAP & MILESTONES                                   │
├──────────────────────────────────┬────────────────────────────────┬──────────────────────────────┤
│          V1: GA LAUNCH           │    V2: POWER USER & ENTERPRISE  │    V3: PLATFORM & AI AUTO    │
│    (Core Interoperability & MVP) │    (Scalability & Deep Work)   │    (Autonomous Intelligence) │
├──────────────────────────────────┼────────────────────────────────┼──────────────────────────────┤
│ • Native IMAP / SMTP / CalDAV    │ • JMAP RFC 8620/8621 Transport │ • Local LLM Thread Triage    │
│ • Vim & Cmd+K Palette UX         │ • Multi-Account Unified Inbox  │ • Natural Language Scheduling│
│ • SQLite FTS5 / Tantivy Search   │ • Cron-Style 1-on-1 Booking    │ • WASM / QuickJS Plugins     │
│ • Strict HTML iframe Sandbox     │ • Multi-Timezone Timeline Grid │ • Cryptographic Webhooks     │
│ • Pure POSIX CLI with --json     │ • OpenPGP & S/MIME Encryption  │ • Realtime Peer Mesh Sync    │
│ • Tauri v2 Desktop & Ratatui TUI │ • Headless Daemon & IPC Bus    │ • Enterprise Key Escrow      │
└──────────────────────────────────┴────────────────────────────────┴──────────────────────────────┘
```

---

## Phase V1: GA Launch (Core Interoperability & Baseline Suite)

### 1. Mail & Transport Protocol Drivers (`nuncio-mail`)
- **IMAP4rev1 Engine (RFC 3501)**:
  - TLS connection manager with `tokio-rustls` and WebPKI root certificates.
  - RFC 2177 IDLE push notification listener loop.
  - RFC 7162 QRESYNC differential synchronization using `HIGHESTMODSEQ`.
  - RFC 6154 Special-Use folder discovery (`\Archive`, `\Drafts`, `\Sent`, `\Junk`, `\Trash`).
- **SMTP Transport Driver (RFC 5321)**:
  - `lettre`-based transport engine supporting Implicit TLS (465) and STARTTLS (587).
  - RFC 5322 MIME builder for text/plain, text/html, `multipart/alternative`, and `multipart/mixed` attachments.
  - Transactional local Outbox queue with exponential backoff retries.
- **Authentication & Credentials**:
  - OAuth2 PKCE Authorization Code Flow (RFC 7636) with local loopback callback server.
  - OS Keyring integration via `keyring` crate with ZeroizeOnDrop memory wrappers.

### 2. Calendar & Scheduling Engine (`nuncio-cal`)
- **CalDAV Protocol Engine (RFC 4791)**:
  - HTTP `calendar-query` and `calendar-multiget` WebDAV REPORT query builder using `quick-xml`.
  - RFC 6578 `sync-collection` differential delta synchronization via `sync-token`.
- **iCalendar Recurrence Engine (RFC 5545)**:
  - RRULE recurrence expansion engine supporting `FREQ`, `INTERVAL`, `BYDAY`, `BYMONTH`, `UNTIL`, `COUNT`.
  - Bounded lazy window expansion preventing infinite recurrence loops.
  - iTIP RSVP state machine (RFC 5546) parsing and emitting `REQUEST`, `REPLY`, `CANCEL`.
- **Grid Layout Algorithm**:
  - Greedy interval coloring algorithm for overlapping event packing with connected component partitioning.
  - 15-minute drag-and-drop snapping grid.

### 3. Data Storage, Search & Security (`nuncio-store`)
- **SQLite Engine**:
  - SQLite WAL mode pragmas with connection pool.
  - Column-level AES-256-GCM authenticated payload encryption.
- **Full-Text Search Engine**:
  - FTS5 trigram indexing over email bodies, subjects, senders, and calendar event summaries with sub-10ms query performance.
  - Structured query parser supporting `from:`, `to:`, `subject:`, `has:attachment`, `after:`, `before:`.

### 4. Presentation Shells (`nuncio-cli`, `nuncio-tui`, `nuncio-gui`)
- **CLI Shell (`nuncio-cli`)**:
  - Pure POSIX Noun + Verb subcommands (`nuncio mail sync`, `nuncio account add`, `nuncio cal list`).
  - `--json` machine-readable output format.
  - Interactive password prompt via `rpassword` fallback.
- **Terminal TUI (`nuncio-tui`)**:
  - Keyboard-first Ratatui 3-pane layout (Folders, Messages, Reader).
  - Dynamic folder unread badges, message list unread flags (`*`), and HTML plain text reader view.
  - Hotkey help modal, splash screen, and account settings view.
- **Desktop GUI (`nuncio-gui` / `nuncio-gui-tauri`)**:
  - Native Tauri v2 desktop window wrapper.
  - React 18 + Vite + TypeScript frontend with CSS custom properties and glassmorphic split-view layout.
  - Strict HTML email iframe sandboxing (`sandbox=""`) enforcing CSP (`default-src 'none'`).

---

## Phase V2: Power User & Enterprise Tier

### 1. Advanced Protocol & Email Workflows
- **JMAP Protocol Engine (RFC 8620 Core / RFC 8621 Mail)**:
  - Native JMAP client with SSE event streaming (`/jmap/event/`).
  - Multi-method request batching and `Email/changes` state token delta sync.
- **Multi-Account Unified Inbox**:
  - Unified mailbox view aggregating Inbox, Sent, Archive across Gmail, Fastmail, Outlook, and generic IMAP accounts.
  - Per-account color badges and sender attribution.
- **HEY-Style Triage Engine**:
  - "The Screener" queue isolating first-time senders for one-time Accept / Block decisions.
  - Category Classifier sorting messages into *Primary*, *Feed (Newsletters)*, and *Paper Trail (Transactions)*.
- **Snooze & Send Later**:
  - Deferred message handling with local background timers and scheduled outbox execution.
- **Custom Domain & DNS Validator**:
  - Multi-identity `From:` selector with background SPF, DKIM, and DMARC alignment validation.

### 2. Advanced Calendar & Scheduling Features
- **Multi-Timezone Grid**:
  - Dual and triple vertical timeline columns with dynamic OS location detection and DST transition indicators.
- **Complex Recurrence Overrides**:
  - Detached instance override engine (`RECURRENCE-ID`) allowing single recurrence modifications without breaking series.
- **CardDAV & Contact Autocomplete**:
  - VCard (RFC 6350) and CardDAV integration for attendee auto-complete and avatar rendering.
- **Cron-Style Embedded Booking Link Engine**:
  - Self-hosted appointment scheduler with customizable duration choices, buffer rules, and auto-generated video call links.
- **System Menu Bar Popover**:
  - Lightweight OS tray application featuring mini-calendar, upcoming meeting countdown, and global hotkeys (`Cmd+Shift+K`).
- **Calendar Sets & Privacy Blockouts**:
  - Contextual calendar profile switching ("Work", "Personal", "Deep Work") and privacy-preserving auto-blocking of personal events as "Busy".

### 3. Enterprise Security & Developer Ergonomics
- **OpenPGP (RFC 3156) & S/MIME v4 (RFC 8551)**:
  - End-to-end PGP and S/MIME message signing, verification, and encryption.
  - Integration with local `gpg-agent` sockets and system X.509 certificate trust stores.
- **Hardware Security Key Integration**:
  - PKCS#11 smartcard interface (`yubikey-pkcs11`) for hardware-isolated private keys.
  - WebAuthn / FIDO2 passkey support for identity verification.
- **Headless Daemon & IPC Bus**:
  - Detached background daemon (`nunciod`) with <50MB RAM footprint.
  - UNIX Socket and Windows Named Pipe IPC bus streaming JSON-RPC 2.0 and SSE events.
- **Sandboxed Plugin Runtime**:
  - QuickJS / Wasmtime sandboxed plugin engine with capability-based security permissions (`net:fetch`, `fs:read`).
- **Cryptographic Webhooks**:
  - Outbound HTTP POST notifications signed with HMAC-SHA256 (`X-Nuncio-Signature-256`) and dead-letter retry queues.

---

## Phase V3: Platform & AI Automation Tier

### 1. Privacy-Preserving Local AI Engine
- **On-Device LLM Summarization**:
  - Embedded small language model (via `llama.cpp` bindings) generating zero-privacy-leak thread summaries and action item extractions.
- **Smart Natural Language Scheduling Bot**:
  - Conversational AI parsing natural language scheduling prompts to find mutual availability slots and draft meeting invites automatically.
- **Smart Slot Recommendation Engine**:
  - Machine learning model analyzing historical meeting patterns, energy levels, and focus time rules to recommend optimal meeting times.

### 2. Advanced Platform & Ecosystem Features
- **Dynamic Transit Buffer Calculator**:
  - Integration with mapping services to automatically calculate travel buffers and insert transit events before in-person meetings.
- **Automated Document & Context Indexing**:
  - Automatic linking of relevant documents, notes, email threads, and previous meeting notes based on attendee graph analysis.
- **Multi-Tenant Enterprise Key Escrow**:
  - Enterprise cryptographic key escrow framework supporting administrative compliance recovery policies and HSM attestation.
- **Real-Time Peer State Synchronization Mesh**:
  - Peer-to-peer end-to-end encrypted event mesh synchronizing state instantly across desktop, mobile, and CLI instances.

---

## Summary Metric & Delivery Matrix

| Phase | Milestone Target | Key Quality & Performance Metric |
| :--- | :--- | :--- |
| **V1** | GA Launch Baseline | 100% Rust workspace test coverage, <16ms TUI/GUI single-frame response, <10ms local search queries. |
| **V2** | Power User & Enterprise | Sub-50MB daemon RAM idle, full OpenPGP/S/MIME compliance, unified multi-account inbox aggregation. |
| **V3** | Platform & AI Automation | 100% on-device local LLM execution, zero-knowledge peer mesh sync, WASM plugin capability isolation. |
