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

### Epic 7: Packaging, Installers & CI/CD (#241 – #260) - `COMPLETED`
- [x] `#241`: Configure GitHub Actions release matrix for Windows, macOS (arm64 + x86_64), and Linux.
- [x] `#242`: Build Windows binary archive (`.zip`) and setup installer integration in `.github/workflows/release.yml`.
- [x] `#243`: Build macOS Universal `.tar.gz` and `.dmg` binary bundles.
- [x] `#244`: Build Linux AppImage and standalone `.tar.gz` packages.
- [x] `#245`: Create Homebrew Tap formula `Formula/nuncio.rb` in `KofTwentyTwo/homebrew-tap` (`brew install koftwentytwo/tap/nuncio`).
- [x] `#246`: Create automated daily pre-release Nightly build pipeline (`0 0 * * *`) tagging `nightly-<YYYY-MM-DD>`.

### Epic 10: Cross-Platform Auto-Update Engine (#301 – #315) - `IN_PROGRESS`
- [x] `#301`: Implement `UpdateEngine` in `crates/nuncio-core/src/update.rs` querying GitHub Releases API (`https://api.github.com/repos/KofTwentyTwo/nuncio/releases/latest`).
- [x] `#302`: Implement SHA-256 checksum verification against `SHA256SUMS.txt` before binary replacement.
- [x] `#303`: Implement atomic self-replacement engine swapping running binary executables (`nuncio-cli`, `nuncio-tui`, `nuncio-mcp`, `nunciod`).
- [x] `#304`: Implement 24h background auto-update checker loop in `nunciod` broadcasting `CoreEvent::UpdateAvailable` over IPC.
- [x] `#305`: Add `nuncio self-update`, `nuncio update check`, and `nuncio update apply` subcommands to `nuncio-cli`.
- [x] `#306`: Build glassmorphic `<UpdateBanner />` in Tauri v2 Desktop GUI (`nuncio-gui`) linked to `@tauri-apps/plugin-updater`.
- [x] `#307`: Add `[u]` key chord and update notification banner to Terminal TUI (`nuncio-tui`).
- [x] `#308`: Add `nuncio_update_check` and `nuncio_update_apply` tools to Native MCP (`nuncio-mcp`).
- [x] `#309`: Verify zero-warning quality gates (`cargo check --workspace`) and 100% test pass rate across workspace.

### Epic 8: NSQL Server-Side Email Filter & Automation Engine (#261 – #285) - `COMPLETED`
- [x] `#261`: Initialize `crates/nuncio-filter` workspace crate with `sqlparser = "0.54"` dependency.
- [x] `#262`: Implement `NuncioSqlDialect` overriding `sqlparser::dialect::Dialect` for NSQL tokenization.
- [x] `#263`: Implement `NsqlParser` transforming SQL text into `nuncio-core` domain AST (`FilterRule`, `ConditionNode`, `RuleAction`).
- [x] `#264`: Implement 6-pass `NsqlValidator` type checker (field types, folder existence, RFC 5322, ReDoS check, contradiction detection).
- [x] `#265`: Implement lossless round-trip code generator (`rule.to_nsql()`) verifying algebraic invariants.
- [x] `#266`: Add SQLite migration DDL for `filter_rules`, `filter_conditions`, `filter_actions`, and `pending_remote_mutations`.
- [x] `#267`: Add SQLite migration DDL for `filter_execution_logs` with composite priority indexes.
- [x] `#268`: Implement HMAC-SHA256 cryptographic hash-chain ledger for `filter_execution_logs` with automated chain verification.
- [x] `#269`: Implement parameterized SQLite query compilation (`sqlx::QueryBuilder`) guaranteeing zero raw string interpolation (100% SQLi immunized).
- [x] `#270`: Implement `ArcSwap<CompiledFilterSet>` lock-free read cache manager ($<5\text{ns}$ access latency).
- [x] `#271`: Integrate `FilterEngine::evaluate()` into `nunciod` background sync loop for pre-ingestion filtering.
- [x] `#272`: Implement 1M-message Keyset Chunking Engine (`WHERE id > ? LIMIT 1000`) guaranteeing $<10\text{MB}$ RAM ceiling during bulk triage.
- [x] `#273`: Implement Transactional Outbox pattern with exponential backoff and full jitter for remote IMAP/JMAP mutations.
- [x] `#274`: Implement Memory-Only Dry-Run Preview API (`filter.preview` IPC method) returning microsecond-resolution match traces without database mutations or network requests.
- [x] `#275`: Implement `CoreEvent::FilterExecuted` and `CoreEvent::BatchFilterProgress` IPC length-prefixed streaming notifications over UNIX sockets and Windows named pipes.
- [x] `#276`: Enforce AST recursion depth limit (`MAX_AST_DEPTH = 10`) to prevent stack overflow panics.
- [x] `#277`: Implement Tokio 50ms hard execution timeout (`tokio::time::timeout`) for regular expression evaluation tasks.
- [x] `#278`: Implement domain whitelist policy and session re-authentication for `FORWARD TO` actions.
- [x] `#279`: Implement pre-flight DNS IP blacklisting (blocking `127.0.0.1`, `169.254.169.254`, private LAN subnets) with `max_redirects(0)` for outbound `CALL WEBHOOK` actions.
- [x] `#280`: Implement HMAC-SHA256 request payload signatures (`X-Nuncio-Signature: t=timestamp,v1=hash`) for outbound webhooks.
- [x] `#281`: Add POSIX CLI (`nuncio-cli`) `nuncio filter` subcommand suite (`list`, `create --sql`, `edit`, `delete`, `test --sql`, `export --format sql`, `import`, `logs`).
- [x] `#282`: Implement Terminal TUI (`nuncio-tui`) `AppMode::FilterRules` with NSQL code editor ↔ visual form builder toggle (`[s]`), dry-run previewer (`[t]`), priority re-ordering (`[J]`/`[K]`), and log inspector drawer (`[l]`).
- [x] `#283`: Build glassmorphic React `<FilterRulesModal />` component in `nuncio-gui` with "Visual Builder" vs "NSQL Query Editor" tabs, live syntax diagnostics, dry-run split pane, and execution log inspector tab.
- [x] `#284`: Register Native MCP (`nuncio-mcp`) `nuncio_filter_*` tools and stream `nuncio://filters` resource.
- [x] `#285`: Create end-to-end integration test suite verifying 1,000,000 message batch performance, zero-warning compiler build, zero-clippy lints, and 100% test pass rate across workspace crates.

### Epic 9: Database Corruption Self-Healing & Recovery Engine (#286 – #300) - `COMPLETED`
- [x] `#286`: Implement `DatabaseEngine::check_integrity()` executing `PRAGMA quick_check(10);` on database open.
- [x] `#287`: Implement SQLite error code trap catching `SQLITE_CORRUPT` (11), `SQLITE_NOTADB` (26), and `SQLITE_CANTOPEN` (14) during runtime query execution.
- [x] `#288`: Implement `CorruptedBackupManager` isolating damaged `.db`, `.db-wal`, and `.db-shm` files to `~/.nuncio/corrupted_backups/nuncio_corrupted_<timestamp>.db`.
- [x] `#289`: Implement `SqliteRecoveryEngine` executing stream salvage to extract valid `accounts`, `filter_rules`, `filter_conditions`, and `filter_actions` records into a fresh SQLite file (`nuncio_main.db`).
- [x] `#290`: Implement cryptographic hash-chain audit ledger verification (`verify_chain_integrity()`) to detect log tampering or corrupted `filter_execution_logs`.
- [x] `#291`: Implement `SelfHealingSyncOrchestrator` in `nunciod` triggering clean background IMAP/JMAP resync when local email caches are reset.
- [x] `#292`: Connect OS Keyring vault credentials to re-initialize authenticated protocol connections seamlessly post-recovery.
- [x] `#293`: Implement `CoreEvent::DatabaseRecovered` event payload and broadcast it over IPC sockets.
- [x] `#294`: Add POSIX CLI (`nuncio-cli`) database recovery status notice on `CoreEvent::DatabaseRecovered`.
- [x] `#295`: Render Terminal TUI (`nuncio-tui`) top menu recovery banner and status indicator.
- [x] `#296`: Render Desktop GUI (`nuncio-gui`) toast notification and resync status badge in React frontend.
- [x] `#297`: Expose Native MCP (`nuncio-mcp`) database health and recovery diagnostic status in `nuncio://system/status`.
- [x] `#298`: Create unit test simulating database header corruption and verifying Stage 1 detection.
- [x] `#299`: Create integration test verifying Stage 2 backup creation and Stage 3 table salvage.
- [x] `#300`: Verify zero warnings (`cargo check --workspace`), zero clippy lints (`cargo clippy --workspace -- -D warnings`), and 100% passing tests.
