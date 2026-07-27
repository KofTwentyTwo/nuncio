# API Surface Draft — Candidate `proto/nuncio/v1` RPCs

**Backlog story:** 0.D.1 (GitHub #144) — extract the API-surface draft before
shrinking the workspace (0.D.2).
**Status of this document:** design input, not a spec. It mines the operation
surface that already exists across `nuncio-mcp` (~20 tool handlers), `nuncio-cli`
(noun/verb subcommands), and the `nunciod` JSON-RPC method table, and grades each
candidate RPC against what actually works today (2026-07-26 ground truth — see
`docs/ROADMAP.md` and the second-brain forensic assessment it cites). Nothing
here is a commitment; Phase 2 (`docs/ROADMAP.md`) decides the real
`proto/nuncio/v1` contract, and per the roadmap's anti-theater guardrail #5, the
four "never-real" features below (E2EE, plugins, AI summarization, NLP
scheduling) must be **re-justified as genuine engine capabilities** before they
re-enter scope — do not treat their presence here as an endorsement.

**Reality tags**
- **REAL** — engine logic executes for real (hits SQLite, does real
  parsing/crypto/HTTP) and is reachable end-to-end from at least one existing
  shell.
- **PARTIAL** — the engine method is genuinely implemented, but the request
  path exposed by CLI/MCP/IPC around it is incomplete, uses fixture/placeholder
  inputs, or the write doesn't actually persist/propagate.
- **STUB** — the handler returns a hardcoded/fabricated response regardless of
  input, or the "backend" is a `Simulate ...` comment. No real work happens.

---

## 1. Accounts service

| RPC (suggested) | Purpose | Request / Response (sketch) | Style | Maps to | Reality |
|---|---|---|---|---|---|
| `Accounts.List` | List configured mail/calendar accounts | `ListAccountsRequest{}` → `ListAccountsResponse{accounts: []Account}` | Unary | `nuncio-mcp/src/tools.rs::nuncio_account_list` (RBAC-filtered), `nuncio-cli` `account list` → `HeadlessRunner::handle_accounts_list`, both call `DatabaseEngine::list_accounts` (`crates/nuncio-store/src/db.rs:401`) | REAL |
| `Accounts.Add` | Register a new IMAP/SMTP account | `AddAccountRequest{email, imap_host, imap_port, smtp_host, smtp_port, imap_mode, smtp_mode}` → `Account` | Unary | `nuncio_account_add` tool, `account add` CLI subcommand → `DatabaseEngine::save_account` (`db.rs:371`) | REAL (persists to SQLite) — but note: no secret ever reaches the OS keyring here; `keyring_secret_key` is just a string label. Credential storage itself is tracked separately under Epic 0.E |
| `Accounts.Get` | Show one account's details | `GetAccountRequest{id}` → `Account` | Unary | `account show` CLI (`runner.rs` `AccountSubcommand::Show`) — **no MCP tool exists for this today** | REAL (reads DB) but CLI-only |
| `Accounts.Update` | Edit an account profile | `UpdateAccountRequest{id, email?, imap_host?, ...}` → `Account` | Unary | `nuncio_account_edit` tool, `account edit` CLI | **STUB** — both handlers only echo back the input fields (`tools.rs:567-574`, `runner.rs` `AccountSubcommand::Edit`); no `DatabaseEngine::update_account` method exists at all |
| `Accounts.Delete` | Remove an account | `DeleteAccountRequest{id}` → `Empty` | Unary | `nuncio_account_delete` tool, `account delete` CLI | **STUB** — both just format `{"status":"deleted"}`; no DB delete method exists, nothing is actually removed |
| `Accounts.TestConnection` | Verify TLS handshake / credentials for an account | `TestAccountRequest{id}` → `TestAccountResponse{ok, latency_ms}` | Unary | `nuncio_account_test` tool, `account test` CLI | **STUB** — both hardcode `latency_ms: 24` and `status: "ok"` regardless of whether the account exists or the host is reachable (`tools.rs:582-588`, `runner.rs` `AccountSubcommand::Test`) |

## 2. Mail service

| RPC (suggested) | Purpose | Request / Response (sketch) | Style | Maps to | Reality |
|---|---|---|---|---|---|
| `Mail.ListMessages` | List messages in a folder | `ListMessagesRequest{folder_id, limit}` → `ListMessagesResponse{messages: []Email}` | Unary | `nuncio_mail_list` tool (RBAC + content-sanitized), `mail list` CLI | REAL via MCP (`DatabaseEngine::list_messages`, `db.rs:493`) but CLI's `handle_list_folder` only returns a message **count**, not the messages themselves — a narrower response shape than MCP's |
| `Mail.GetMessage` | Read one message body | `GetMessageRequest{id}` → `Email` | Unary | `mail read` CLI → `DatabaseEngine::get_message` (`db.rs:565`) — **no MCP tool exposes single-message read**; MCP only has list/search | REAL (CLI only) |
| `Mail.SendMessage` | Compose/send an email | `SendMessageRequest{account_id, to, subject, body}` → `SendMessageResponse{email_id, status}` | Unary | `nuncio_mail_send` tool: saves the `Email` row and pings the daemon to trigger `SyncAll` (`tools.rs:343-397`) | PARTIAL — MCP path writes a real `Email` row to the `Sent` folder and best-effort notifies the daemon over IPC, but there is **no actual SMTP transport call**; `nuncio-mail/src/smtp.rs` exists as a client but is never invoked from this path. CLI's `mail send` is a full **STUB** — `handle_send_email` only formats a success string and touches no storage or transport at all |
| `Mail.Search` | Full-text search over subjects/bodies | `SearchRequest{query}` → `SearchResponse{hits: []SearchHit}` | Unary | `nuncio_mail_search` tool → `nuncio_store::search::SearchEngine::search_messages` (real SQLite FTS5, `search.rs:96`) | PARTIAL — real FTS5 query behind the MCP tool, but **CLI's `mail search` is a total STUB** — `handle_search` always returns `results: []` regardless of query or of what's in the FTS index; this is the sharpest CLI/MCP parity gap found |
| `Mail.MarkRead` | Toggle read/unread flag | `MarkReadRequest{message_id, read}` → `Empty` | Unary | IPC method `mail.mark_read` (`crates/nuncio-core/src/ipc/server.rs:156`) | PARTIAL — updates the in-memory `AppState.unread_count` and publishes `MessageFlagsChanged`, but never writes the flag back to the `messages` table (no `update_message_flags`-style DB method exists). Also: **no CLI subcommand and no MCP tool call this at all** — it's IPC-only and unreachable from any shell today |
| `Mail.SyncAll` / `Mail.SyncAccount` | Trigger IMAP/JMAP synchronization | `SyncRequest{account_id?}` → stream of `SyncProgress{status, account_id}` | **Server-streaming** (progress) | IPC `mail.sync_all`, `mail sync` CLI, `nunciod`'s outbox worker | PARTIAL — `EventBus::process_command(SyncAll)` flips status to `Syncing` and emits `SyncStarted`/would emit `SyncCompleted`, but there is no real IMAP/JMAP fetch loop wired to it; nothing round-trips against a live or mocked server today. Today it's a status-flag toggle, not a sync |
| `Mail.Export` | Export messages to MBOX/EML-zip/JSON | `ExportRequest{format, output_path, folder_id, limit}` → `ExportResponse{summary}` | Unary | `nuncio_export_data` tool → `DatabaseEngine::export_messages_to_file` / `nuncio_core::export::ExportEngine` (`export.rs`) | REAL — genuine MBOX/zip/JSON writers. **CLI has no `export` subcommand at all** — MCP-only capability today |
| `Mail.Events` | Push stream of mail/system events | (no request) → stream of `Event{type, payload}` | **Server-streaming** | IPC `events.notify` notification broadcast from `EventBus::subscribe_events()` over every open TCP connection (`ipc/server.rs:109-124`) | PARTIAL — the plumbing is real (every connected client gets every `CoreEvent` pushed), but the events it carries are themselves thin (`SyncStarted`/`FilterExecuted`/etc. without real sync behind them); payload today is just `format!("{:?}", evt)` (debug-formatted, not structured JSON) |

## 3. Folders service

| RPC (suggested) | Purpose | Request / Response | Style | Maps to | Reality |
|---|---|---|---|---|---|
| `Folders.List` | List mailbox folders | `Empty` → `ListFoldersResponse{folders: []Folder}` | Unary | `folder list` CLI → `DatabaseEngine::list_folders` (`db.rs:615`) — **no MCP tool exists for folders at all** | REAL (CLI only) |

## 4. Search service

Mail-body search is `Mail.Search` above (kept in the Mail service so message
search sits next to message list/read). This section covers the one search
surface that doesn't belong to Mail:

| RPC (suggested) | Purpose | Request / Response | Style | Maps to | Reality |
|---|---|---|---|---|---|
| `Search.Events` | FTS5 search over calendar events | `SearchRequest{query}` → `SearchResponse{hits}` | Unary | `SearchEngine::search_events` (`nuncio-store/src/search.rs:123`) | PARTIAL — genuine FTS5 engine method exists and is tested, but it is **exposed by no shell today** (no CLI subcommand, no MCP tool, no IPC method calls it), so it doesn't clear the REAL bar of "reachable end-to-end from at least one existing shell" |

## 5. Filters service (NSQL)

| RPC (suggested) | Purpose | Request / Response | Style | Maps to | Reality |
|---|---|---|---|---|---|
| `Filters.List` | List NSQL rules | `Empty` → `ListFilterRulesResponse{rules}` | Unary | `nuncio_filter_list` tool, `filter list` CLI, IPC `filter.list` | REAL — all three paths hit `DatabaseEngine::list_filter_rules` |
| `Filters.Create` | Create an NSQL rule (6-pass validated) | `CreateFilterRuleRequest{name, sql, priority}` → `FilterRule` | Unary | `nuncio_filter_create`, `filter create` CLI, IPC `filter.create` → `NsqlParser::parse_rule` + `NsqlValidator::validate` (`crates/nuncio-filter/src/validator.rs`) + `save_filter_rule` | REAL — genuine parse/validate/persist pipeline, and the daemon path additionally reloads the live `FilterEngine` (`nunciod/src/main.rs:120-135`); the MCP/CLI paths persist but do **not** trigger an engine reload, so a running `nunciod` won't pick up the new rule until restarted or edited again via IPC |
| `Filters.Update` | Edit an NSQL rule | `UpdateFilterRuleRequest{id, name?, sql?, priority?}` → `FilterRule` | Unary | `nuncio_filter_edit`, `filter edit` CLI, IPC `filter.edit` | REAL, same reload caveat as above |
| `Filters.Delete` | Delete a rule by ID | `DeleteFilterRuleRequest{id}` → `Empty` | Unary | `nuncio_filter_delete`, `filter delete` CLI, IPC `filter.delete` | REAL |
| `Filters.Test` (dry-run preview) | Evaluate NSQL against a sample/real email without side effects | `TestFilterRequest{sql, message_id?}` → `FilterPreview{matched, actions_evaluated, execution_time_us}` | Unary | `nuncio_filter_test`, `filter test` CLI, IPC `filter.preview` → `FilterEngine::preview` | REAL — CLI additionally supports evaluating against a real stored message via `--message-id`; MCP/IPC always test against a synthetic sample email |
| `Filters.Logs` | Fetch hash-chained execution logs | `ListFilterLogsRequest{limit}` → `ListFilterLogsResponse{logs}` | Unary | `nuncio_filter_logs`, `filter logs` CLI, IPC `filter.logs` → `list_filter_execution_logs` / `verify_execution_log_chain` | REAL — HMAC/hash-chained log entries genuinely persisted |
| `Filters.Export` / `Filters.Import` | Bulk export/import NSQL rule sets | `ExportFilterRulesRequest{format}` → bytes; `ImportFilterRulesRequest{file}` → `count` | Unary | `filter export` / `filter import` CLI only | PARTIAL — real read/write, but `import` is a naive line-splitter (`runner.rs` `FilterSubcommand::Import`) that does not use the JSON round-trip `export --format json` produces; **no MCP tool or IPC method for either** |
| `Filters.TriageKeyset` | Batch-evaluate the full mailbox in keyset-chunked pages, executing matched actions | `TriageRequest{batch_size}` → stream of `TriageProgress{processed, total, matched}` | **Server-streaming** | IPC-only `filter.triage_keyset` (`nunciod/src/main.rs:199-254`) | PARTIAL — real chunked scan + rule evaluation + execution-log writes + outbox mutation creation and `BatchFilterProgress` events, but the "remote mutation" it queues is only ever fulfilled by the outbox worker's `// Simulate remote IMAP/JMAP mutation execution` (`nunciod/src/main.rs:46`) — no message is actually moved/flagged/deleted anywhere. **No CLI or MCP surface calls this at all** — IPC-only, effectively unreachable outside a raw JSON-RPC client today |

Not counted as its own RPC: `CALL WEBHOOK` rule actions fire an outbound,
HMAC-SHA256-signed, SSRF-defended HTTP POST via
`nuncio-filter/src/webhook.rs::WebhookDispatcher` — a genuinely real capability,
but it's a side effect of `Filters.TriageKeyset`/rule execution, not something a
client calls directly. It's only worth its own RPC (e.g. `Filters.TestWebhook`)
if webhook configuration/testing becomes a user-facing feature.

## 6. Calendar service

| RPC (suggested) | Purpose | Request / Response | Style | Maps to | Reality |
|---|---|---|---|---|---|
| `Calendar.ListEvents` | List events for a calendar | `ListEventsRequest{calendar_id}` → `ListEventsResponse{events}` | Unary | `nuncio_cal_list_events` tool — direct `sqlx::query_as` against `calendar_events` (`tools.rs:428-456`); **`cal list` CLI is a STUB** always returning `events: []` (`runner.rs` `CalSubcommand::List`) | PARTIAL — real query behind the MCP tool, but the CLI path for the identical operation is a hardcoded empty stub (inverse of the `Mail.Search` gap) |
| `Calendar.CreateEvent` | Create a calendar event | `CreateEventRequest{account_id, calendar_id, summary, start_time, end_time}` → `CalendarEvent` | Unary | `nuncio_cal_create_event` tool — real `INSERT` into `calendar_events` (`tools.rs:457-521`) | REAL via MCP — genuinely inserts a row and is exercised in the test suite. **No CLI subcommand exists to create events at all**, so it's reachable from exactly one shell |
| `Calendar.Sync` | CalDAV synchronization | `SyncRequest{calendar_id?}` → stream `SyncProgress` | Server-streaming | `cal sync` CLI → just calls `EventBus::process_command(SyncAll)` | STUB — same generic status-flag toggle as `Mail.SyncAll`; no CalDAV transport actually runs. `nuncio-cal/src/caldav.rs` only builds `<c:calendar-query>` XML and parses multistatus responses — there's no HTTP client wired to fetch from a real/mocked CalDAV server anywhere in the call path |

Not counted as their own RPCs but relevant supporting engine logic:
`nuncio-cal/src/rrule.rs` (RFC 5545 recurrence expansion) and
`nuncio-cal/src/parser.rs` (iCalendar parsing) are real library code exercised
by unit tests, but nothing in `Calendar.ListEvents`/`Sync` calls them yet — once
`Calendar.Sync` becomes real, recurrence expansion likely surfaces as a field/
flag on `Calendar.ListEvents` rather than its own RPC.

## 7. Contacts service

| RPC (suggested) | Purpose | Request / Response | Style | Maps to | Reality |
|---|---|---|---|---|---|
| `Contacts.List` | List address-book contacts | `Empty` → `ListContactsResponse{contacts}` | Unary | `contact list` CLI → `ContactsDatabase::list_contacts` (real SQLite CRUD, `nuncio-contacts/src/db.rs`) | REAL — **CLI-only; no MCP tool, no IPC method for contacts exist at all**, despite `McpAgentPolicy` already defining a `DataType::Contacts` and `read_contacts` permission that nothing currently checks |
| `Contacts.Search` | FTS search over contacts | `SearchRequest{query}` → `SearchResponse{contacts}` | Unary | `contact search` CLI → `ContactsDatabase::search_contacts` | REAL, CLI-only |
| `Contacts.Add` | Add a contact | `AddContactRequest{name, email, org?}` → `Contact` | Unary | `contact add` CLI → `ContactsDatabase::save_contact` + `Contact::to_vcard()` | REAL, CLI-only |
| `Contacts.SyncCardDav` | CardDAV address-book sync | `SyncRequest{account_id}` → stream `SyncProgress` | Server-streaming | `nuncio-contacts/src/carddav.rs::CardDavClient::fetch_remote_vcards` | **STUB** — hardcoded to return one fabricated `Contact::new("Google CardDAV Sync Contact", ...)` regardless of the configured CardDAV URL; no real PROPFIND/REPORT request is issued. Not exposed by any shell |

## 8. System / Status service

| RPC (suggested) | Purpose | Request / Response | Style | Maps to | Reality |
|---|---|---|---|---|---|
| `System.Ping` | Liveness probe | `Empty` → `PingResponse{"pong"}` | Unary | IPC `system.ping` (`ipc/server.rs:139`) | REAL |
| `System.GetState` | Current daemon/engine state snapshot | `Empty` → `StateResponse{status, accounts_loaded, unread_count, last_error}` | Unary | IPC `system.state`, `system status` CLI → `EventBus::current_state()` + `DatabaseEngine::check_integrity()` | REAL — self-healing integrity probe genuinely runs; `unread_count`/`accounts_loaded` reflect real `AppState`, though `unread_count` is only ever mutated by the in-memory `MarkRead` path (see gap above), not by an actual unread scan of the mailbox |
| `System.GetResource` (`nuncio://system/status`) | MCP resource variant of the same probe | `Empty` → same shape | Unary | `nuncio-mcp/src/resources.rs::read_resource("nuncio://system/status")` | REAL, MCP-only — duplicates `System.GetState` under a different transport idiom (MCP resource vs. tool/IPC RPC); Phase 2 should collapse these into one RPC |
| `System.Licenses` | Third-party OSS license/attribution list | `Empty` → `LicensesResponse{licenses}` | Unary | `nuncio_licenses` tool, `licenses` CLI command | REAL but 100% static/hand-maintained data (not generated from `Cargo.lock`) — fine as a capability, just flag that it will drift from actual dependencies over time |
| `System.Banner` | Splash/branding metadata | `Empty` → `BannerResponse{name, site, version, ...}` | Unary | `banner` CLI command | REAL (static), CLI-only, arguably not worth a proto RPC at all — candidate to drop rather than port |
| `Update.Check` | Query GitHub Releases for a newer version | `Empty` → `UpdateCheckResult{current_version, latest_version, update_available, release_info}` | Unary | `nuncio_update_check` tool, `update check` CLI, IPC `update.check` → `UpdateEngine::check_for_updates` (real GitHub Releases API call + semver compare) | REAL |
| `Update.Apply` | Download/verify/install an update | `ApplyUpdateRequest{version?}` → `ApplyUpdateResponse{status, message}` | Unary | `nuncio_update_apply` tool, `update apply` CLI, IPC `update.apply` → `UpdateEngine::apply_update` | **PARTIAL, and deliberately disabled at the daemon.** The MCP tool is a pure STUB (hardcodes `"status": "update_initiated"` without calling `UpdateEngine` at all — `tools.rs:746-757`). The CLI path calls the real `UpdateEngine`, which does real SHA-256 checksum verification of the downloaded archive against `SHA256SUMS.txt` — **but that verification is fail-open** (installs proceed unverified if the checksum file is missing from the release), a known security gap tracked as GH #140 (0.B.2). The daemon's IPC `update.apply` handler is deliberately hardwired to always return an error until Phase 4 fixes the fail-open bug (`nunciod/src/main.rs:262-275`) |
| `Audit.List` | List WORM (write-once-read-many) audit records | `ListAuditRequest{limit, offset}` → `ListAuditResponse{records}` | Unary | `nuncio_audit_list` tool → `DatabaseEngine::list_worm_audit_records` | REAL — genuine HMAC-SHA256 hash-chained records persisted per mutation. **MCP-only: no CLI `audit` subcommand and no IPC method exist**, despite the roadmap treating WORM integrity as a load-bearing security property |
| `Audit.Verify` | Verify the full HMAC hash-chain integrity | `Empty` → `VerifyAuditResponse{chain_integrity_valid}` | Unary | `nuncio_audit_verify` tool → `DatabaseEngine::verify_worm_audit_chain` | REAL, MCP-only (same gap as above) |

## 9. Not modeled as RPCs (library capabilities with no live call path)

These exist as real or semi-real Rust code but are not reachable from any shell
today, and per `docs/ROADMAP.md` guardrail #5 are explicitly named as
"never-real" features that must be re-justified before they earn a place in
`proto/nuncio/v1`:

- **`nuncio-core/src/ai.rs` (`LocalAiEngine`)** — thread summarization, action-item
  extraction, smart replies. **100% STUB/fabricated**: every method returns a
  hardcoded canned response (fixed 3 bullet points, fixed 2 action items with a
  literal `"2026-07-25"` due date, 3 fixed reply strings) regardless of the input
  email body. No Ollama/llama.cpp HTTP call is ever made despite the
  `endpoint_url`/`model_name` fields implying one. Zero shell exposure.
- **`nuncio-core/src/e2ee.rs` (`E2eeEngine`)** — OpenPGP/S-MIME signature
  verification. Real comparison/validation *logic* exists, but it operates on
  caller-supplied strings with no actual GPG/CMS parsing, no keyring integration,
  and no shell (CLI/MCP/IPC) calls it. Tag: STUB/PLANNED.
- **`nuncio-core/src/plugin.rs` (`PluginRuntime`)** — "sandboxed" plugin/hook
  registry. Real in-memory `Vec<PluginManifest>` bookkeeping, but `register_plugin`
  only validates a non-empty ID and there is no actual sandboxing, no execution
  engine, and no shell surface. Tag: STUB/PLANNED (the roadmap's "WASM plugins"
  never-real feature).
- **`nuncio-cal/src/scheduling.rs` (`SchedulingLinkGenerator`)** — 1:1
  booking-link generation and natural-language duration parsing (e.g. "30m",
  "2 hours"). **STUB/PLANNED**: `generate_1on1_link` is pure string templating
  (`https://nuncio.mx/meet/{email}/{slug}?dur=...`) with no booking backend
  behind that URL, and no shell exposes it at all. Tag: STUB/PLANNED (the
  roadmap's "NLP scheduler" never-real feature).

None of these should be carried into Phase 2's proto surface as-is; they're
listed here only so the mining pass in this document is complete and honest
about what the MCP/CLI/engine layer currently contains.

---

## Parity gaps (CLI vs. MCP vs. IPC vs. TUI)

The pre-pivot POC (CLI, TUI, GUI, MCP, IPC) never had structural parity — each
shell called into the engine independently, and several diverged into different
fake or partially-real behavior. Concretely:

1. **Mail search is inverted between CLI and MCP.** MCP's `nuncio_mail_search`
   is genuinely wired to FTS5 (`SearchEngine::search_messages`); the CLI's
   `mail search` always returns an empty result set no matter the query or the
   index contents.
2. **Calendar list/create is MCP-only and real; CLI's `cal list` is a hardcoded
   empty stub, and CLI has no `cal create` at all.** The only way to create a
   calendar event today is through the MCP tool.
3. **Account mutation (`edit`/`delete`/`test`) is theater in *both* CLI and
   MCP** — neither shell nor the underlying `DatabaseEngine` has real
   update/delete methods; both simply format a success response.
4. **Contacts exist only in the CLI.** `nuncio-contacts` is a real, tested crate
   (SQLite CRUD + FTS + vCard generation), exposed via `nuncio contact
   list/search/add` — but there is no MCP tool, no IPC method, and no TUI
   keybinding for contacts at all, despite `McpAgentPolicy` already reserving a
   `DataType::Contacts` permission bit that no code path checks.
5. **Audit trail (WORM log) is MCP-only.** `nuncio_audit_list` /
   `nuncio_audit_verify` are real, but there's no `nuncio audit ...` CLI
   subcommand and no IPC method — an operator with only daemon/CLI access has no
   way to inspect or verify the tamper-evident audit chain.
6. **Folder listing and message export are MCP/CLI-split.** `folder list` is
   CLI-only (no MCP tool); `nuncio_export_data` is MCP-only (no CLI `export`
   subcommand).
7. **`mail.mark_read` and `filter.triage_keyset` are IPC-only** — reachable only
   by a raw JSON-RPC client speaking to `nunciod` directly; neither the CLI nor
   MCP tool tables call either method today.
8. **TUI parity is the narrowest of all.** `crates/nuncio-tui/src/keybindings.rs`
   only maps actions for mail (list/read/compose/reply/sync), filters
   (list/create/test/reorder/logs), accounts, splash, and update-check — there
   is no calendar, contacts, export, or audit keybinding in the TUI at all.
9. **Two status probes exist that should be one RPC.** IPC `system.state` and
   the MCP resource `nuncio://system/status` return overlapping but
   differently-shaped payloads for the same underlying `EventBus::current_state()`
   + `check_integrity()` call.

None of this is a criticism of any one shell in isolation — it's exactly the
"each shell faked what the spine could not provide" failure mode that
`docs/adr/0001-engine-first-grpc-architecture.md` diagnosed and that motivated
moving to a single published gRPC contract in the first place.

---

## Notes for Phase 2

**Naming conventions**
- Service names group by noun (`Accounts`, `Mail`, `Folders`, `Search`,
  `Filters`, `Calendar`, `Contacts`, `System`, `Update`, `Audit`), matching the
  CLI's existing noun/verb structure (`nuncio <noun> <verb>`) rather than the
  MCP tool table's flat `nuncio_<domain>_<verb>` naming — the noun/verb split
  maps more directly onto gRPC service/method pairs.
- Collapse duplicate surfaces before they become duplicate RPCs: `system.state`
  (IPC) and `nuncio://system/status` (MCP resource) should become the single
  `System.GetState`; `filter.preview` (IPC) and `nuncio_filter_test` (MCP/CLI)
  should become the single `Filters.Test`.

**Versioning**
- `proto/nuncio/v1` as directory-per-major-version, per the roadmap and ADR
  0001. Field additions are backward compatible within v1; anything that
  changes a request/response shape (e.g., finally making `Accounts.Update`
  real, or `Mail.Search` actually searching from the CLI path) is still a v1
  addition, not a break, as long as old fields are preserved.
- Every RPC in this draft that is currently STUB or PARTIAL should ship in
  Phase 2's proto **only** once the underlying engine capability is real, per
  the roadmap's principle #1: "a capability is 'done' only when engine + proto
  + CLI command + offline E2E test all exist together." Do not proto-ize
  fabricated behavior — an `Unimplemented` status is preferred over another
  fake `"status": "ok"` response.

**Auth**
- Per ADR 0001: gRPC over loopback TCP (`127.0.0.1`) only for v1, no networked/
  multi-tenant mode yet. A bearer token is minted into the OS keyring on daemon
  first-run and sent as gRPC metadata (e.g. `authorization: Bearer <token>`) on
  every call. This replaces the current total absence of authentication on the
  JSON-RPC TCP socket (today, anything that can reach `127.0.0.1:9422` can call
  any method).
- The MCP↔gRPC bridge (Phase 5) will need to mint or reuse this same token when
  it proxies `tools/call` into gRPC unary calls, and will need a policy layer
  equivalent to today's `McpAgentPolicy` (RBAC by data type / account / folder)
  sitting in front of the bridge, since gRPC's bearer token alone is
  all-or-nothing per client, not per-agent scoped.

**Push / streaming**
- Everything tagged **server-streaming** above (`Mail.SyncAll`/`SyncAccount`,
  `Mail.Events`, `Filters.TriageKeyset`, `Calendar.Sync`,
  `Contacts.SyncCardDav`) should use a single underlying event/progress stream
  pattern rather than bespoke progress messages per RPC, mirroring how
  `EventBus::subscribe_events()` already fans out one `broadcast::Receiver` to
  every connected client. Replace the current `format!("{:?}", evt)`
  debug-string payload (`ipc/server.rs:114-118`) with a real structured
  `Event` proto message — this is one of the two concrete bugs ADR 0001 calls
  out (the other being the JSON-RPC response/notification demux race), and
  gRPC server-streaming removes the demux race by construction (one stream per
  RPC call instead of one multiplexed duplex socket).
- `System.GetState`/`Mail.ListMessages`/etc. remain unary; only genuinely
  push-shaped operations (new mail arriving, sync progress, filter-triage
  batch progress, live audit/event tailing) should be streaming RPCs.

---

## Summary count

40 candidate RPCs catalogued across 8 services (Accounts: 6, Mail: 8, Folders: 1,
Search: 1, Filters: 8, Calendar: 3, Contacts: 4, System/Update/Audit: 9), plus 4
library capabilities explicitly excluded as not-yet-real ("never-real" features
per roadmap guardrail #5: AI summarization, E2EE, plugins, NLP scheduling) and
one internal-only capability (`CALL WEBHOOK` dispatch) noted but not counted
since it isn't itself a client-callable RPC.

- **REAL:** 25
- **PARTIAL:** 10
- **STUB:** 5
