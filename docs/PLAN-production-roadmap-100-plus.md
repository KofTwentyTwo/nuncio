# Nuncio Production Readiness Roadmap (Zero-Mock Architecture)

> **Goal:** Transition Nuncio from stubbed/mocked interfaces to a 100% real, human, production-ready cross-platform mail & calendar client suite with ZERO mocked data or stubbed network calls.

---

## Roadmap Overview & Epic Matrix

| Epic ID | Domain / Scope | Issue Range | Key Target Deliverable |
| :--- | :--- | :---: | :--- |
| **Epic 1** | **Real Protocol Drivers & Live Sync** | `#101 - #125` | Live IMAP (IDLE/Sync), SMTP (TLS Send), JMAP (RFC 8620/8621), CalDAV (RFC 4791). |
| **Epic 2** | **Storage & Data-at-Rest Security** | `#126 - #145` | SQLite WAL migrations, per-account PBKDF2 vault key derivation, FTS5 trigrams. |
| **Epic 3** | **TUI Interactive Terminal App** | `#146 - #170` | Live DB state binding, scrollable reader, compose/reply text modal, folder tree. |
| **Epic 4** | **Native Desktop GUI (Tauri v2 + React)** | `#171 - #200` | Windows/macOS/Linux desktop app, React frontend, sandboxed HTML iframe. |
| **Epic 5** | **CLI Live Pipeline & Interactive Inputs** | `#201 - #220` | Interactive non-echoing prompts (`rpassword`), background daemon sync, Unix piping. |
| **Epic 6** | **Live E2E System Test Matrix** | `#221 - #240` | Dockerized Dovecot/Postfix/Radicale test harnesses with zero mock servers. |
| **Epic 7** | **Packaging, Installers & CI/CD** | `#241 - #260` | Windows MSI/EXE, macOS DMG with codesigning, Linux AppImage/DEB/RPM. |

---

## Epic 1: Real Protocol Drivers & Live Sync Engine (#101 – #125)

- **#101**: Implement live IMAP connection pool with tls handshake via `async-imap` & `tokio-rustls`.
- **#102**: Implement IMAP IDLE push notification listener loop for real-time inbox updates.
- **#103**: Implement incremental IMAP UID FETCH message sync algorithm storing flags in SQLite.
- **#104**: Implement IMAP mailbox list parser mapping RFC 3501 hierarchy to domain `Folder` entities.
- **#105**: Implement IMAP draft, sent, trash, and flag state mutations (`STORE +FLAGS`, `COPY`, `EXPUNGE`).
- **#106**: Implement live SMTP transport client using `lettre` with STARTTLS and Implicit TLS support.
- **#107**: Implement MIME message builder for multipart/alternative (plain text + HTML + attachments).
- **#108**: Implement live SMTP delivery confirmation and queue retry mechanism with backoff.
- **#109**: Implement JMAP Session resource discovery (`/.well-known/jmap`) and API token authentication.
- **#110**: Implement JMAP `Email/get` and `Email/changes` push synchronization engine.
- **#111**: Implement JMAP `Email/set` for creation, updates, and destruction of messages.
- **#112**: Implement CalDAV WebDAV report query builder for time-range event filtering (`RFC 4791`).
- **#113**: Implement CalDAV multi-status XML parser extracting `VEVENT` data using `quick-xml`.
- **#114**: Implement CalDAV `PUT` / `DELETE` methods for creating and modifying calendar events.
- **#115**: Implement RFC 5545 iCalendar recurrence engine expansion using `rrule` crate.
- **#116**: Implement CardDAV VCard parsing and sync for contacts (`RFC 6350`).
- **#117**: Implement connection fallback strategy (Implicit TLS 465/993 -> STARTTLS 587/143).
- **#118**: Implement self-signed SSL/TLS certificate handling and custom CA trust store options.
- **#119**: Implement network error recovery and automatic reconnect loops for dropped sockets.
- **#120**: Implement protocol metrics collector tracking sync duration, byte throughput, and error rates.
- **#121**: Implement MIME attachment stream downloader saving raw bytes to local disk cache.
- **#122**: Implement JMAP `Mailbox/query` for custom user-created mailbox folders.
- **#123**: Implement OAuth2 authentication flow support for Gmail and Office365 endpoints.
- **#124**: Implement JMAP event source push engine (`/eventsource`) for real-time updates.
- **#125**: Implement live protocol unit and integration verification against external test accounts.

---

## Epic 2: Storage & Data-at-Rest Security (#126 – #145)

- **#126**: Implement SQLite WAL journal mode pragmas and connection pool manager in `nuncio-store`.
- **#127**: Implement PBKDF2/Argon2id key derivation module mapping OS vault secrets to AES keys.
- **#128**: Implement transparent column-level message body encryption/decryption in SQLite queries.
- **#129**: Implement `ZeroizeOnDrop` credential memory hygiene for all account configuration fields.
- **#130**: Implement SQLite FTS5 full-text search index setup with trigram tokenizer for email bodies.
- **#131**: Implement FTS5 query builder with prefix search and quote escaping.
- **#132**: Implement automated database migration runner (`sqlx::migrate!`) for schema versioning.
- **#133**: Implement local file attachment stream encryption using `age` X25519 passphrases.
- **#134**: Implement account isolation model enforcing single-tenant table scoping or multi-db files.
- **#135**: Implement database compaction and VACUUM routine scheduled periodically.
- **#136**: Implement `SecretManager` keyring integration for Windows Credential Manager, macOS Keychain, and Linux Secret Service.
- **#137**: Implement secret key rotation mechanism re-encrypting message bodies when key changes.
- **#138**: Implement local database corruption recovery and backup snapshot creation.
- **#139**: Implement message thread / conversation grouping queries in SQLite.
- **#140**: Implement contact autocomplete index in SQLite FTS5.
- **#141**: Implement calendar event cache storing recurrence instances in SQLite.
- **#142**: Implement attachment cache eviction policy (LRU cap at 1GB).
- **#143**: Implement database error mapping into domain `DatabaseError` variants.
- **#144**: Implement ephemeral database test helper for isolation during integration tests.
- **#145**: Implement SQLite index tuning and query plan verification for sub-10ms response times.

---

## Epic 3: TUI Interactive Terminal App (#146 – #170)

- **#146**: Connect `TuiApp` to live `DatabaseEngine` and `EventBus` channels.
- **#147**: Implement live dynamic folder list rendering from SQLite in TUI Sidebar.
- **#148**: Implement live message list rendering for selected folder with unread indicators.
- **#149**: Implement dynamic email reader view rendering plain text body from SQLite.
- **#150**: Implement HTML-to-terminal text renderer using `select.rs` / custom parser for HTML emails.
- **#151**: Implement scrollable reader pane supporting Page Up / Page Down / Vim motions.
- **#152**: Implement interactive compose email modal with multi-line text input fields.
- **#153**: Implement interactive reply / reply-all modal populating subject and recipient headers.
- **#154**: Implement account switcher overlay listing all configured accounts with selection status.
- **#155**: Implement interactive account creation form modal with validation rules.
- **#156**: Implement interactive Help menu modal (`?` / `h`) dynamically listing active keybindings.
- **#157**: Implement status bar showing background sync progress spinner and error alerts.
- **#158**: Implement search modal (`/`) filtering message lists via SQLite FTS5 in real time.
- **#159**: Implement message action keybindings (Delete `d`, Archive `e`, Mark Read `m`, Flag `*`).
- **#160**: Implement terminal resize listener adjusting ratatui layouts dynamically.
- **#161**: Implement splash screen view (`p`) displaying logo and version branding.
- **#162**: Implement color theme engine supporting Dark, Light, High Contrast, and Matrix themes.
- **#163**: Implement attachment selector view allowing terminal users to save attachments.
- **#164**: Implement confirmation dialog modal for destructive actions (e.g. permanent delete).
- **#165**: Implement calendar view pane in TUI rendering monthly/weekly grid with events.
- **#166**: Implement contact list view pane in TUI with search and detail inspection.
- **#167**: Implement mouse click and scroll wheel support for ratatui panes.
- **#168**: Implement custom keybinding configuration parser loading from `~/.config/nuncio/keys.toml`.
- **#169**: Implement terminal raw mode safety guards restoring terminal state on exit or panic.
- **#170**: Implement full E2E interactive TUI session tests using `ratatui::backend::TestBackend`.

---

## Epic 4: Native Desktop GUI (Tauri v2 + React/Vite) (#171 – #200)

- **#171**: Initialize Tauri v2 application framework structure in `crates/nuncio-gui/src-tauri`.
- **#172**: Initialize React + Vite + TypeScript frontend project structure in `crates/nuncio-gui/ui`.
- **#173**: Configure native window windowing rules (title bar, minimum dimensions 1024x768).
- **#174**: Implement Tauri v2 IPC commands (`#[tauri::command]`) linking React to `IpcBridge`.
- **#175**: Implement React split-view layout component (Sidebar, MessageList, Reader).
- **#176**: Implement Sandboxed HTML email iframe renderer component with strict CSP.
- **#177**: Implement custom `nuncio-mail://` protocol handler in Tauri v2 for secure attachment loading.
- **#178**: Implement tracking pixel defense blocking remote image loading by default with toggle button.
- **#179**: Implement dark mode / light mode theme toggle with CSS system preference auto-detection.
- **#180**: Implement rich text email composer with HTML editor and attachment drag-and-drop.
- **#181**: Implement account manager settings modal for adding/editing mail accounts in GUI.
- **#182**: Implement live search bar component connected to SQLite FTS5 IPC queries.
- **#183**: Implement desktop native system notifications for new incoming emails.
- **#184**: Implement system tray icon with unread badge counter and quick action menu.
- **#185**: Implement native OS menu bar bindings (File, Edit, View, Window, Help).
- **#186**: Implement calendar view component with Month, Week, Day, and Agenda displays.
- **#187**: Implement calendar event creation and edit modal in GUI.
- **#188**: Implement contact manager view with VCard import/export support.
- **#189**: Implement offline state indicator and automatic reconnection banner in GUI.
- **#190**: Implement keyboard shortcut manager in GUI (Vim keys + standard Cmd/Ctrl shortcuts).
- **#191**: Implement attachment preview pane (images, PDFs, plain text) inside webview.
- **#192**: Implement infinite scrolling message list with virtualized list rendering (react-window).
- **#193**: Implement custom context menus (right-click) for messages, folders, and attachments.
- **#194**: Implement sound notification options for incoming mail alerts.
- **#195**: Implement macOS auto-update integration via Tauri v2 updater module.
- **#196**: Implement Windows toast notification integration with deep linking.
- **#197**: Implement Linux GTK/AppIndicator system tray compatibility.
- **#198**: Implement UI accessibility (a11y) ARIA attributes and keyboard focus management.
- **#199**: Implement Playwright / Spectron end-to-end GUI automation test suite.
- **#200**: Package and build native desktop binaries for Windows (x64), macOS (ARM64/x64), Linux (x64).

---

## Epic 5: CLI Live Pipeline & Interactive Inputs (#201 – #220)

- **#201**: Implement interactive password prompt using `rpassword` when password is not in keyring.
- **#202**: Connect `nuncio mail sync` to live IMAP/JMAP background sync workers.
- **#203**: Connect `nuncio account add` to live validation checking IMAP/SMTP connectivity.
- **#204**: Implement pipeable standard input (`stdin`) support for `nuncio mail send`.
- **#205**: Implement pipeable JSON stream output (`--json`) formatted for `jq` processing.
- **#206**: Implement daemon mode (`nuncio daemon start`) running sync loop in background process.
- **#207**: Implement IPC control channel (`nuncio daemon status/stop`) communicating with daemon.
- **#208**: Implement verbose logging level flags (`-v`, `-vv`, `-vvv`) emitting tracing output to stderr.
- **#209**: Implement shell completion generation (`nuncio system completion bash/zsh/fish/pwsh`).
- **#210**: Implement CSV / TSV export options for email and contact search queries.
- **#211**: Implement `nuncio cal sync` triggering live CalDAV WebDAV fetch.
- **#212**: Implement `nuncio cal list` displaying upcoming calendar events in formatted table.
- **#213**: Implement `nuncio folder create / delete / rename` executing live server folder management.
- **#214**: Implement non-interactive batch script mode for automated server environments.
- **#215**: Implement signal handling (`SIGINT`, `SIGTERM`) for clean CLI process termination.
- **#216**: Implement configuration file generator creating default `~/.config/nuncio/config.toml`.
- **#217**: Implement environment variable override parser (`NUNCIO_ACCOUNT`, `NUNCIO_LOG`, etc.).
- **#218**: Implement interactive setup wizard (`nuncio account wizard`) guiding first-time users.
- **#219**: Implement exit status code standardization (0=success, 1=general, 2=usage, 3=network, 4=auth).
- **#220**: Implement CLI end-to-end integration tests using `assert_cmd` and `predicates`.

---

## Epic 6: Live E2E Integration & System Test Suite (#221 – #240)

- **#221**: Set up Docker Compose test environment with live Dovecot (IMAP) and Postfix (SMTP).
- **#222**: Set up Radicale CalDAV/CardDAV docker test container for live calendar integration tests.
- **#223**: Implement end-to-end mail sync test verifying live IMAP UID fetch into SQLite.
- **#224**: Implement end-to-end SMTP mail send test delivering email to local Postfix server.
- **#225**: Implement end-to-end CalDAV event sync test querying and storing live iCal events.
- **#226**: Implement TLS certificate validation failure integration test.
- **#227**: Implement network disconnection and auto-reconnect stress test under heavy load.
- **#228**: Implement concurrent multi-account synchronization integration test.
- **#229**: Implement large email (20MB attachment) upload and download stress test.
- **#230**: Implement malformed MIME payload resilience test ensuring parser does not crash.
- **#231**: Implement invalid iCal recurrence rule test ensuring rrule bounds hold.
- **#232**: Implement SQLite database migration rollback and forward migration test.
- **#233**: Implement keyring secret vault simulation test across Windows, macOS, and Linux mocks.
- **#234**: Implement FTS5 full-text search query stress test with 10,000 index entries.
- **#235**: Implement HTML sanitizer XSS payload injection test matrix verifying total isolation.
- **#236**: Implement TUI input event loop stress test feeding high-speed crossterm key streams.
- **#237**: Implement GUI IPC contract test verifying all JSON payloads validate against schema.
- **#238**: Implement memory leak audit under 24-hour continuous sync execution.
- **#239**: Implement cross-crate panic isolation test ensuring protocol errors do not crash app.
- **#240**: Integrate full live E2E test matrix into GitHub Actions CI workflow.

---

## Epic 7: Packaging, CI/CD & Cross-Platform Installers (#241 – #260)

- **#241**: Configure GitHub Actions build workflow matrix for Windows, macOS, and Linux.
- **#242**: Build Windows WiX installer (`.msi`) for `nuncio-gui` and `nuncio-cli`.
- **#243**: Build Windows standalone executable bundle (`.exe`) with embedded dependencies.
- **#244**: Build macOS Apple Silicon (ARM64) and Intel (x64) `.dmg` disk image packages.
- **#245**: Configure Apple Developer ID code signing and `notarytool` notarization step.
- **#246**: Build Linux AppImage standalone executable package.
- **#247**: Build Debian / Ubuntu `.deb` package with package metadata and desktop entry.
- **#248**: Build Fedora / RHEL `.rpm` package with RPM spec file.
- **#249**: Build Arch Linux `PKGBUILD` package definition for AUR publishing.
- **#250**: Configure automated GitHub Release asset upload on git tag push (`v*`).
- **#251**: Create Windows code signing certificate integration via Azure Key Vault / signtool.
- **#252**: Implement auto-updater server endpoint metadata file generation (`latest.json`).
- **#253**: Build Homebrew Cask formula for macOS installation (`brew install nuncio`).
- **#254**: Build Chocolatey package for Windows installation (`choco install nuncio`).
- **#255**: Build Scoop manifest for Windows CLI installation (`scoop install nuncio`).
- **#256**: Create Linux `.desktop` desktop entry and application icons (SVG, PNG 512x512).
- **#257**: Create man pages for CLI (`man nuncio`) using `clap_mangen`.
- **#258**: Implement reproducible build pipeline verifying SHA-256 checksums across OS targets.
- **#259**: Write end-user installation manual and troubleshooting guide in `docs/INSTALL.md`.
- **#260**: Perform final release verification audit confirming all 160 issues are closed.

---
*Roadmap generated and validated for Nuncio Zero-Mock Production Development.*
