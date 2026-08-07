//! Headless engine runner executing CLI commands against core services.

use nuncio_core::EventBus;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nuncio_store::{DatabaseEngine, DatabaseError};
use serde_json::json;
use std::sync::Arc;
use thiserror::Error;

use crate::args::{
    AccountSubcommand, AuditSubcommand, CalSubcommand, Commands, ContactSubcommand,
    FilterSubcommand, FolderSubcommand, MailSubcommand, SystemSubcommand, UpdateSubcommand,
};

use crate::output::{format_json, format_json_error, format_json_error_with_info};

/// Parses a `--imap-mode`/`--smtp-mode` CLI string into `nuncio_core::TlsMode`.
/// Rejects anything else rather than silently defaulting, since silently
/// falling back to e.g. `Plain` for a typo'd mode would be a serious
/// transport-security footgun.
fn parse_tls_mode(mode: &str) -> Result<nuncio_core::TlsMode, String> {
    match mode {
        "implicit_tls" => Ok(nuncio_core::TlsMode::ImplicitTls),
        "start_tls" => Ok(nuncio_core::TlsMode::StartTls),
        "plain" => Ok(nuncio_core::TlsMode::Plain),
        other => Err(format!(
            "invalid tls mode '{other}' (expected implicit_tls, start_tls, or plain)"
        )),
    }
}

/// Maps a `nuncio_core::TlsMode` onto its wire-format `nuncio.v1.TlsMode`
/// enum value, mirroring `nunciod::grpc`'s server-side mapping.
fn map_tls_mode_to_proto(mode: nuncio_core::TlsMode) -> nuncio_proto::v1::TlsMode {
    match mode {
        nuncio_core::TlsMode::ImplicitTls => nuncio_proto::v1::TlsMode::ImplicitTls,
        nuncio_core::TlsMode::StartTls => nuncio_proto::v1::TlsMode::StartTls,
        nuncio_core::TlsMode::Plain => nuncio_proto::v1::TlsMode::Plain,
    }
}

/// Parse an `account add --protocol` string into its wire-format
/// `nuncio.v1.AccountProtocol` value. Accepts both the kebab-case
/// serialization (`imap-smtp`) and common aliases.
fn parse_account_protocol(protocol: &str) -> Result<nuncio_proto::v1::AccountProtocol, String> {
    match protocol.to_ascii_lowercase().as_str() {
        "imap-smtp" | "imap_smtp" | "imap" => Ok(nuncio_proto::v1::AccountProtocol::ImapSmtp),
        "jmap" => Ok(nuncio_proto::v1::AccountProtocol::Jmap),
        "caldav" | "cal-dav" | "cal_dav" => Ok(nuncio_proto::v1::AccountProtocol::Caldav),
        other => Err(format!(
            "invalid protocol '{other}' (expected imap-smtp, jmap, or caldav)"
        )),
    }
}

/// Renders an account's active transport as a set of `--json` fields (protocol
/// discriminant plus that transport's own endpoint fields), so `account
/// list`/`show` expose exactly the fields the active transport carries.
fn account_transport_json(config: &nuncio_proto::v1::AccountConfig) -> serde_json::Value {
    use nuncio_proto::v1::account_config::Transport;
    match &config.transport {
        Some(Transport::ImapSmtp(t)) => json!({
            "protocol": "imap-smtp",
            "imap_host": t.imap_host,
            "imap_port": t.imap_port,
            "imap_tls_mode": t.imap_tls_mode().as_str_name(),
            "smtp_host": t.smtp_host,
            "smtp_port": t.smtp_port,
            "smtp_tls_mode": t.smtp_tls_mode().as_str_name(),
        }),
        Some(Transport::Jmap(t)) => json!({
            "protocol": "jmap",
            "endpoint_host": t.endpoint_host,
        }),
        Some(Transport::Dav(t)) => json!({
            "protocol": "caldav",
            "collection_url": t.collection_url,
        }),
        None => json!({ "protocol": "unspecified" }),
    }
}

/// Merges the top-level object fields of `extra` into `base` (both must be
/// JSON objects). Used to fold an account's transport-specific fields into its
/// common-field object without hand-writing every combination.
fn merge_json(base: &mut serde_json::Value, extra: serde_json::Value) {
    if let (Some(base), serde_json::Value::Object(extra)) = (base.as_object_mut(), extra) {
        for (k, v) in extra {
            base.insert(k, v);
        }
    }
}

/// Renders an account's active transport as a single human-readable summary
/// line fragment for `account list`/`show`.
fn account_transport_summary(config: &nuncio_proto::v1::AccountConfig) -> String {
    use nuncio_proto::v1::account_config::Transport;
    match &config.transport {
        Some(Transport::ImapSmtp(t)) => format!(
            "IMAP {}:{} ({})  SMTP {}:{} ({})",
            t.imap_host,
            t.imap_port,
            t.imap_tls_mode().as_str_name(),
            t.smtp_host,
            t.smtp_port,
            t.smtp_tls_mode().as_str_name(),
        ),
        Some(Transport::Jmap(t)) => format!("JMAP {}", t.endpoint_host),
        Some(Transport::Dav(t)) => format!("CalDAV {}", t.collection_url),
        None => "(no transport configured)".to_string(),
    }
}

/// Parses a `filter export --format` CLI string into its wire-format
/// `nuncio.v1.RuleExportFormat` value. Rejects anything else rather than
/// silently falling back to one rendering, since a typo'd format should
/// surface as an error, not a surprising output shape.
fn parse_rule_export_format(format: &str) -> Result<nuncio_proto::v1::RuleExportFormat, String> {
    match format.to_ascii_lowercase().as_str() {
        "sql" => Ok(nuncio_proto::v1::RuleExportFormat::Sql),
        "json" => Ok(nuncio_proto::v1::RuleExportFormat::Json),
        other => Err(format!(
            "invalid export format '{other}' (expected sql or json)"
        )),
    }
}

/// Renders a wire-format `google.protobuf.Timestamp` as an RFC 3339 UTC
/// string for human-readable and `--json` output. `nanos` outside
/// `0..1_000_000_000` (never produced by the daemon's own mappers) and an
/// absent field both render as `"unknown"` rather than fabricating a time.
fn format_timestamp(ts: &Option<nuncio_proto::time::Timestamp>) -> String {
    ts.as_ref()
        .and_then(|ts| {
            u32::try_from(ts.nanos)
                .ok()
                .map(|nanos| (ts.seconds, nanos))
        })
        .and_then(|(seconds, nanos)| chrono::DateTime::from_timestamp(seconds, nanos))
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Extracts the exact unix-nanosecond instant a wire-format `Timestamp`
/// carries, for callers that need the audit ledger's full sub-second
/// precision rather than the RFC 3339 rendering.
fn timestamp_unix_nanos(ts: &Option<nuncio_proto::time::Timestamp>) -> i64 {
    ts.as_ref()
        .map(nuncio_proto::time::timestamp_to_unix_nanos)
        .unwrap_or(0)
}

/// Renders a wire-format `google.protobuf.Duration` as whole seconds for
/// human-readable and `--json` output (e.g. `AccountConfig.sync_interval`).
fn duration_secs(d: &Option<nuncio_proto::time::Duration>) -> u64 {
    d.as_ref()
        .map(nuncio_proto::time::duration_to_secs)
        .unwrap_or(0)
}

/// Renders a wire-format `google.protobuf.Duration` as whole microseconds
/// for human-readable and `--json` output (e.g.
/// `PreviewRuleResponse.execution_time`).
fn duration_micros(d: &Option<nuncio_proto::time::Duration>) -> u64 {
    d.as_ref()
        .map(nuncio_proto::time::duration_to_micros)
        .unwrap_or(0)
}

/// Renders a `nuncio.v1.Message` (as returned by the daemon's `Mail` gRPC
/// service) into the JSON shape used by `mail list`/`mail read`'s
/// `--json` output.
fn message_proto_to_json(message: &nuncio_proto::v1::Message) -> serde_json::Value {
    json!({
        "id": message.id,
        "account_id": message.account_id,
        "folder_id": message.folder_id,
        "subject": message.subject,
        "sender": message.sender,
        "recipient": message.recipient,
        "received_at": format_timestamp(&message.received_at),
        "read": message.read,
        "body_plain": message.body_plain,
        "body_html": message.body_html,
    })
}

/// Renders a `nuncio.v1.CalendarEvent` (as returned by the daemon's
/// `Calendar` gRPC service) into the JSON shape used by `cal list`'s
/// `--json` output.
fn calendar_event_proto_to_json(event: &nuncio_proto::v1::CalendarEvent) -> serde_json::Value {
    json!({
        "id": event.id,
        "account_id": event.account_id,
        "calendar_id": event.calendar_id,
        "summary": event.summary,
        "start_time": format_timestamp(&event.start_time),
        "end_time": format_timestamp(&event.end_time),
        "rrule": event.rrule,
        "location": event.location,
    })
}

/// Renders a `nuncio.v1.Contact` (as returned by the daemon's `Contacts`
/// gRPC service) into the JSON shape used by `contact list`/`search`/`add`'s
/// `--json` output.
fn contact_proto_to_json(contact: &nuncio_proto::v1::Contact) -> serde_json::Value {
    json!({
        "id": contact.id,
        "account_id": contact.account_id,
        "display_name": contact.display_name,
        "given_name": contact.given_name,
        "family_name": contact.family_name,
        "organization": contact.organization,
        "job_title": contact.job_title,
        "emails": contact.emails.iter().map(|e| json!({
            "email": e.email,
            "label": e.label,
            "is_primary": e.is_primary,
        })).collect::<Vec<_>>(),
        "phones": contact.phones.iter().map(|p| json!({
            "phone": p.phone,
            "label": p.label,
            "is_primary": p.is_primary,
        })).collect::<Vec<_>>(),
        "is_favorite": contact.is_favorite,
        "interaction_count": contact.interaction_count,
    })
}

/// Returns `true` if `contact`'s display name, organization, or any email
/// address contains `query` (case-insensitive). Backs `contact search`'s
/// client-side filtering over the full `ListContacts` result, since the
/// `Contacts` gRPC surface has no server-side search RPC.
fn contact_matches_query(contact: &nuncio_proto::v1::Contact, query: &str) -> bool {
    let query = query.to_lowercase();
    contact.display_name.to_lowercase().contains(&query)
        || contact
            .organization
            .as_deref()
            .is_some_and(|org| org.to_lowercase().contains(&query))
        || contact
            .emails
            .iter()
            .any(|e| e.email.to_lowercase().contains(&query))
}

/// Maps a `nuncio_core::export::ExportFormat` onto its wire-format
/// `nuncio.v1.ExportFormat` enum value, mirroring `nunciod::grpc`'s
/// server-side mapping.
fn map_export_format_to_proto(format: nuncio_core::ExportFormat) -> nuncio_proto::v1::ExportFormat {
    match format {
        nuncio_core::ExportFormat::Mbox => nuncio_proto::v1::ExportFormat::Mbox,
        nuncio_core::ExportFormat::EmlZip => nuncio_proto::v1::ExportFormat::EmlZip,
        nuncio_core::ExportFormat::Json => nuncio_proto::v1::ExportFormat::Json,
        nuncio_core::ExportFormat::JsonLines => nuncio_proto::v1::ExportFormat::Jsonl,
    }
}

/// Renders a `nuncio.v1.AuditRecord` (as returned by the daemon's `Audit`
/// gRPC service) into the JSON shape used by
/// `system audit list`'s `--json` output. `record_hmac` is a verification
/// MAC output, never secret key material, so it is safe to include here.
fn audit_record_proto_to_json(record: &nuncio_proto::v1::AuditRecord) -> serde_json::Value {
    json!({
        "sequence": record.sequence,
        "timestamp": format_timestamp(&record.timestamp),
        "timestamp_ns": timestamp_unix_nanos(&record.timestamp),
        "actor": record.actor,
        "action": record.action,
        "data_hash": record.data_hash,
        "previous_block_hash": record.previous_block_hash,
        "record_hmac": record.record_hmac,
    })
}

/// Renders a `nuncio.v1.FilterRule` (as returned by the daemon's `Filters`
/// gRPC service) into the JSON shape used by
/// `filter list`/`filter create`'s `--json` output.
fn filter_rule_proto_to_json(rule: &nuncio_proto::v1::FilterRule) -> serde_json::Value {
    json!({
        "id": rule.id,
        "name": rule.name,
        "target_account": rule.target_account,
        "priority": rule.priority,
        "enabled": rule.enabled,
        "nsql_text": rule.nsql_text,
        "actions": rule.actions,
        "created_at": format_timestamp(&rule.created_at),
        "updated_at": format_timestamp(&rule.updated_at),
    })
}

fn filter_execution_log_proto_to_json(
    log: &nuncio_proto::v1::FilterExecutionLog,
) -> serde_json::Value {
    json!({
        "id": log.id,
        "rule_id": log.rule_id,
        "message_id": log.message_id,
        "action_taken": log.action_taken,
        "matched_at": format_timestamp(&log.matched_at),
        "prev_hash": log.prev_hash,
        "hash": log.hash,
    })
}

/// Errors emitted by the CLI headless runner.
#[derive(Error, Debug)]
pub enum RunnerError {
    /// Engine initialization failure.
    #[error("failed to initialize engine: {0}")]
    InitFailed(String),
    /// Database operation error.
    #[error("database failure: {0}")]
    Database(#[from] DatabaseError),
}

/// Headless core runner executing CLI commands non-interactively.
///
/// Most commands operate against an ephemeral local engine (database +
/// event bus) for now. `system status` (see [`Self::handle_system_status`])
/// and `account add` / `account list` (see [`Self::handle_add_account`] /
/// [`Self::handle_accounts_list`]) are the exceptions: they are thin gRPC
/// clients of the real `nunciod` daemon's `nuncio.v1.System` and
/// `nuncio.v1.Accounts` APIs, authenticated by a bearer token read from an
/// injected [`SecretManager`] — production code uses
/// [`SecretManager::production`] (the real OS keyring), while tests inject
/// [`SecretManager::mock`] so no test ever touches the real vault.
///
/// `account add` persists through the daemon so the account (and its
/// password, stored ONLY in the daemon's OS keyring vault) survives past
/// this CLI process exiting -- unlike this runner's own ephemeral local
/// `db`, which is thrown away when the process exits.
pub struct HeadlessRunner {
    event_bus: EventBus,
    db: DatabaseEngine,
    secrets: Arc<SecretManager>,
    grpc_addr: String,
    // Kept only to hold the ephemeral database's backing directory open for
    // the runner's lifetime: dropping it would unlink the directory out from
    // under `db` while the pool may still need to open new connections.
    _db_dir: tempfile::TempDir,
}

impl HeadlessRunner {
    /// Initialize a new `HeadlessRunner` with an ephemeral database, the
    /// real OS keyring vault ([`SecretManager::production`]), and the gRPC
    /// daemon address resolved from [`nuncio_proto::grpc_addr_from_env`].
    pub async fn ephemeral() -> Result<Self, RunnerError> {
        Self::ephemeral_with(
            Arc::new(SecretManager::production()),
            nuncio_proto::grpc_addr_from_env(),
        )
        .await
    }

    /// Initialize a new `HeadlessRunner` with an ephemeral database and an
    /// explicit secret vault + gRPC daemon address.
    ///
    /// This is the constructor tests MUST use whenever they exercise
    /// `system status`: pass a [`SecretManager::mock`]-backed instance
    /// (never the real OS keyring) and the address of a test-local gRPC
    /// server.
    pub async fn ephemeral_with(
        secrets: Arc<SecretManager>,
        grpc_addr: String,
    ) -> Result<Self, RunnerError> {
        let (db, db_dir) = DatabaseEngine::connect_ephemeral()
            .await
            .map_err(|e| RunnerError::InitFailed(e.to_string()))?;
        let event_bus = EventBus::new();
        Ok(Self {
            event_bus,
            db,
            secrets,
            _db_dir: db_dir,
            grpc_addr,
        })
    }

    /// Access the underlying `EventBus`.
    #[allow(dead_code)]
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// Access the underlying `DatabaseEngine`.
    #[allow(dead_code)]
    pub fn db(&self) -> &DatabaseEngine {
        &self.db
    }

    /// Execute a CLI subcommand, returning a formatted string output.
    pub async fn execute_command(&self, cmd: &Commands, json_mode: bool) -> String {
        match cmd {
            // Pure Noun + Verb Commands
            Commands::Account { action } => match action {
                AccountSubcommand::List => self.handle_accounts_list(json_mode).await,
                AccountSubcommand::Add {
                    email,
                    protocol,
                    collection_url,
                    imap_host,
                    imap_port,
                    smtp_host,
                    smtp_port,
                    imap_mode,
                    smtp_mode,
                    password,
                } => {
                    self.handle_add_account(
                        email,
                        protocol,
                        collection_url.as_deref(),
                        imap_host,
                        *imap_port,
                        smtp_host,
                        *smtp_port,
                        imap_mode,
                        smtp_mode,
                        &password.0,
                        json_mode,
                    )
                    .await
                }
                AccountSubcommand::Show { id } => self.handle_account_show(id, json_mode).await,
                AccountSubcommand::Edit {
                    id,
                    email,
                    imap_host,
                    imap_port,
                    smtp_host,
                    smtp_port,
                    imap_mode,
                    smtp_mode,
                } => {
                    self.handle_account_edit(
                        id,
                        email.as_deref(),
                        imap_host.as_deref(),
                        *imap_port,
                        smtp_host.as_deref(),
                        *smtp_port,
                        imap_mode.as_deref(),
                        smtp_mode.as_deref(),
                        json_mode,
                    )
                    .await
                }
                AccountSubcommand::Delete { id } => self.handle_account_delete(id, json_mode).await,
                AccountSubcommand::Test { id } => self.handle_account_test(id, json_mode).await,
            },
            Commands::Mail { action } => match action {
                MailSubcommand::Sync => self.handle_sync(json_mode).await,
                MailSubcommand::List { folder } => self.handle_list_folder(folder, json_mode).await,
                MailSubcommand::Read { id } => self.handle_read_message(id, json_mode).await,
                MailSubcommand::Send {
                    to,
                    subject,
                    body,
                    account,
                } => {
                    self.handle_send_email(to, subject, body, account.as_deref(), json_mode)
                        .await
                }
                MailSubcommand::Search { query } => self.handle_search(query, json_mode).await,
                MailSubcommand::Mark { id, read, unread } => {
                    self.handle_mark_read(id, *read, *unread, json_mode).await
                }
                MailSubcommand::Export {
                    format,
                    out,
                    account,
                    folder,
                } => {
                    self.handle_mail_export(
                        format,
                        out,
                        account.as_deref(),
                        folder.as_deref(),
                        json_mode,
                    )
                    .await
                }
            },
            Commands::Banner => {
                crate::output::print_splash_banner();
                if json_mode {
                    format_json(&serde_json::json!({
                        "name": "Nuncio",
                        "site": "https://nuncio.mx",
                        "version": "1.0.0",
                        "etymology": "nūntiō (Latin: I announce, I declare, I deliver a message)",
                        "shells": ["cli", "tui", "gui", "mcp"]
                    }))
                } else {
                    String::new()
                }
            }
            Commands::Licenses => {
                let credits = vec![
                    ("tokio", "MIT", "Event-driven asynchronous runtime engine"),
                    (
                        "ratatui",
                        "MIT",
                        "Terminal User Interface rendering library",
                    ),
                    (
                        "tauri",
                        "MIT/Apache-2.0",
                        "Cross-platform desktop application shell",
                    ),
                    ("sqlx", "MIT/Apache-2.0", "Async SQLite database driver"),
                    ("lettre", "MIT", "Email creation & SMTP client"),
                    ("async-imap", "MIT/Apache-2.0", "Async IMAP protocol client"),
                    (
                        "aes-gcm",
                        "MIT/Apache-2.0",
                        "AES-256-GCM authenticated encryption",
                    ),
                    (
                        "age",
                        "MIT/Apache-2.0",
                        "Attachment stream encryption cipher",
                    ),
                    ("zeroize", "MIT/Apache-2.0", "Secure heap memory wiping"),
                    (
                        "keyring",
                        "MIT/Apache-2.0",
                        "OS native key store integration",
                    ),
                ];
                if json_mode {
                    format_json(&serde_json::json!({ "licenses": credits }))
                } else {
                    let mut out = String::from(
                        "\nNuncio Third-Party Open Source Library Acknowledgments:\n\n",
                    );
                    for (lib, lic, desc) in credits {
                        out.push_str(&format!("  • {:<15} [{:<14}] {}\n", lib, lic, desc));
                    }
                    out.push_str("\nFull license terms available in THIRD_PARTY_LICENSES.md\n");
                    out
                }
            }
            Commands::Folder { action } => match action {
                FolderSubcommand::List => self.handle_folders_list(json_mode).await,
            },
            Commands::Cal { action } => match action {
                CalSubcommand::List {
                    account,
                    calendar,
                    start,
                    end,
                } => {
                    self.handle_cal_list(account, calendar, *start, *end, json_mode)
                        .await
                }
                CalSubcommand::Sync {
                    account,
                    calendar,
                    start,
                    end,
                } => {
                    self.handle_cal_sync(account, calendar, *start, *end, json_mode)
                        .await
                }
            },
            Commands::System { action } => match action {
                SystemSubcommand::Status => self.handle_system_status(json_mode).await,
                SystemSubcommand::Audit { action } => match action {
                    AuditSubcommand::List { page_size } => {
                        self.handle_audit_list(*page_size, json_mode).await
                    }
                    AuditSubcommand::Verify => self.handle_audit_verify(json_mode).await,
                },
            },
            Commands::Contact { action } => match action {
                ContactSubcommand::List { account } => {
                    self.handle_contact_list(account, json_mode).await
                }
                ContactSubcommand::Search { account, query } => {
                    self.handle_contact_search(account, query, json_mode).await
                }
                ContactSubcommand::Add {
                    account,
                    name,
                    email,
                    org,
                } => {
                    self.handle_contact_add(account, name, email, org.as_deref(), json_mode)
                        .await
                }
                ContactSubcommand::Sync { account } => {
                    self.handle_contact_sync(account, json_mode).await
                }
            },
            Commands::Filter { action } => match action {
                FilterSubcommand::List => self.handle_filter_list(json_mode).await,
                FilterSubcommand::Create {
                    name,
                    sql,
                    priority,
                } => {
                    self.handle_filter_create(name, sql, *priority, json_mode)
                        .await
                }
                FilterSubcommand::Delete { id } => self.handle_filter_delete(id, json_mode).await,
                FilterSubcommand::Validate { sql } => {
                    self.handle_filter_validate(sql, json_mode).await
                }
                FilterSubcommand::Test { sql, message_id } => {
                    self.handle_filter_test(sql, message_id.as_deref(), json_mode)
                        .await
                }
                FilterSubcommand::Edit {
                    id,
                    name,
                    sql,
                    priority,
                } => {
                    self.handle_filter_edit(
                        id,
                        name.as_deref(),
                        sql.as_deref(),
                        *priority,
                        json_mode,
                    )
                    .await
                }
                FilterSubcommand::Export { format } => {
                    self.handle_filter_export(format, json_mode).await
                }
                FilterSubcommand::Import { file } => {
                    self.handle_filter_import(file, json_mode).await
                }
                FilterSubcommand::Logs { limit } => {
                    self.handle_filter_logs(*limit, json_mode).await
                }
                FilterSubcommand::Triage {
                    rule_id,
                    chunk_size,
                } => {
                    self.handle_filter_triage(rule_id.clone(), *chunk_size, json_mode)
                        .await
                }
            },
            Commands::Update { action } => match action {
                UpdateSubcommand::Check => match nuncio_core::UpdateEngine::new() {
                    Ok(updater) => match updater.check_for_updates().await {
                        Ok(res) => {
                            if json_mode {
                                format_json(&json!(res))
                            } else if res.update_available {
                                let notes = res
                                    .release_info
                                    .as_ref()
                                    .map(|i| i.release_notes.as_str())
                                    .unwrap_or("");
                                format!(
                                    "Update available: v{} (current: v{})\n\nRelease Notes:\n{}",
                                    res.latest_version, res.current_version, notes
                                )
                            } else {
                                format!("Nuncio is up to date (version v{}).", res.current_version)
                            }
                        }
                        Err(e) => {
                            if json_mode {
                                format_json_error(&e.to_string())
                            } else {
                                format!("Error checking updates: {e}")
                            }
                        }
                    },
                    Err(e) => {
                        if json_mode {
                            format_json_error(&e.to_string())
                        } else {
                            format!("Error initializing updater: {e}")
                        }
                    }
                },
                UpdateSubcommand::Apply => match nuncio_core::UpdateEngine::new() {
                    Ok(updater) => match updater.check_for_updates().await {
                        Ok(res) => {
                            if let Some(info) = res.release_info {
                                if !res.update_available {
                                    if json_mode {
                                        format_json(
                                            &json!({ "status": "already_up_to_date", "current_version": res.current_version }),
                                        )
                                    } else {
                                        format!(
                                            "Nuncio is already up to date (version v{}).",
                                            res.current_version
                                        )
                                    }
                                } else {
                                    match updater.apply_update(&info).await {
                                        Ok(msg) => {
                                            if json_mode {
                                                format_json(
                                                    &json!({ "status": "updated", "version": info.version, "message": msg }),
                                                )
                                            } else {
                                                format!("✓ {msg}")
                                            }
                                        }
                                        Err(e) => {
                                            if json_mode {
                                                format_json_error(&e.to_string())
                                            } else {
                                                format!("Failed to apply update: {e}")
                                            }
                                        }
                                    }
                                }
                            } else if json_mode {
                                format_json(
                                    &json!({ "status": "already_up_to_date", "current_version": res.current_version }),
                                )
                            } else {
                                format!(
                                    "Nuncio is already up to date (version v{}).",
                                    res.current_version
                                )
                            }
                        }
                        Err(e) => {
                            if json_mode {
                                format_json_error(&e.to_string())
                            } else {
                                format!("Error checking updates: {e}")
                            }
                        }
                    },
                    Err(e) => {
                        if json_mode {
                            format_json_error(&e.to_string())
                        } else {
                            format!("Error initializing updater: {e}")
                        }
                    }
                },
            },
        }
    }

    /// `mail sync`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Mail/Sync` API.
    /// Replaces this command's previous local-ephemeral behavior (flipping
    /// this runner's own throwaway `EventBus` status flag, which never
    /// fetched a single real message) with a real inbound sync against the
    /// daemon's persistent store -- the RPC awaits full completion before
    /// returning, so a successful response's `synced_count` reflects
    /// messages that are already visible via `mail list`/`mail read`.
    async fn handle_sync(&self, json_mode: bool) -> String {
        let mut client = match self.connect_mail_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .sync(nuncio_proto::v1::SyncRequest { account_id: None })
            .await
        {
            Ok(response) => {
                let synced_count = response.into_inner().synced_count;
                if json_mode {
                    format_json(&json!({
                        "status": "sync_started",
                        "synced_count": synced_count,
                    }))
                } else {
                    format!("Synchronization complete: {synced_count} message(s) synced")
                }
            }
            Err(status) => Self::render_status_error("sync", &status, json_mode),
        }
    }

    /// `mail list`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Mail` API. Lists
    /// messages in `folder`, newest first, from the daemon's real,
    /// persistent store -- NOT this runner's own ephemeral local `db`, which
    /// is thrown away when this CLI process exits.
    async fn handle_list_folder(&self, folder: &str, json_mode: bool) -> String {
        let mut client = match self.connect_mail_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        // Follow `next_page_token` to the end so the command shows the whole
        // folder, transparently exercising the keyset pagination convention.
        let mut messages = Vec::new();
        let mut page_token = String::new();
        loop {
            match client
                .list_messages(nuncio_proto::v1::ListMessagesRequest {
                    folder_id: folder.to_string(),
                    page_size: 0,
                    page_token: page_token.clone(),
                })
                .await
            {
                Ok(response) => {
                    let page = response.into_inner();
                    messages.extend(page.messages);
                    if page.next_page_token.is_empty() {
                        break;
                    }
                    page_token = page.next_page_token;
                }
                Err(status) => {
                    return Self::render_status_error("list_messages", &status, json_mode);
                }
            }
        }

        if json_mode {
            let messages_json: Vec<serde_json::Value> =
                messages.iter().map(message_proto_to_json).collect();
            format_json(&json!({
                "folder": folder,
                "messages": messages_json
            }))
        } else {
            format!("Folder '{}': {} message(s) found", folder, messages.len())
        }
    }

    /// `mail send`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Mail/SendMessage` API. The daemon builds a real
    /// SMTP transport from the configured account's SMTP endpoint and
    /// keyring password, and only reports success when the transport
    /// genuinely accepted the
    /// message -- this NEVER prints a fabricated "Message sent" without a
    /// real send actually happening.
    async fn handle_send_email(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        account: Option<&str>,
        json_mode: bool,
    ) -> String {
        let mut client = match self.connect_mail_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .send_message(nuncio_proto::v1::SendMessageRequest {
                to: to.to_string(),
                cc: None,
                subject: subject.to_string(),
                body_text: body.to_string(),
                body_html: None,
                attachments: Vec::new(),
                account_id: account.map(str::to_string),
            })
            .await
        {
            Ok(response) => {
                let message_id = response.into_inner().message_id;
                if json_mode {
                    format_json(&json!({
                        "sent": true,
                        "message_id": message_id,
                        "to": to,
                        "subject": subject,
                        "bytes": body.len()
                    }))
                } else {
                    format!(
                        "Message sent to {} ('{}') [id: {}]",
                        to, subject, message_id
                    )
                }
            }
            Err(status) => Self::render_status_error("send_message", &status, json_mode),
        }
    }

    /// `mail search`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Mail` API. Runs a
    /// full-text (FTS5) search over the daemon's real, persistent store.
    async fn handle_search(&self, query: &str, json_mode: bool) -> String {
        let mut client = match self.connect_mail_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        // Follow `next_page_token` to the end so search returns every hit.
        let mut hits = Vec::new();
        let mut page_token = String::new();
        loop {
            match client
                .search_messages(nuncio_proto::v1::SearchMessagesRequest {
                    query: query.to_string(),
                    page_size: 0,
                    page_token: page_token.clone(),
                })
                .await
            {
                Ok(response) => {
                    let page = response.into_inner();
                    hits.extend(page.hits);
                    if page.next_page_token.is_empty() {
                        break;
                    }
                    page_token = page.next_page_token;
                }
                Err(status) => {
                    return Self::render_status_error("search_messages", &status, json_mode);
                }
            }
        }

        if json_mode {
            let hits_json: Vec<serde_json::Value> = hits
                .iter()
                .map(|h| {
                    json!({
                        "id": h.message_id,
                        "title": h.title,
                        "snippet": h.snippet,
                    })
                })
                .collect();
            format_json(&json!({
                "query": query,
                "results": hits_json
            }))
        } else {
            format!("Search complete for '{}' ({} matches)", query, hits.len())
        }
    }

    /// `folder list`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Mail` API.
    async fn handle_folders_list(&self, json_mode: bool) -> String {
        let mut client = match self.connect_mail_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        // Follow `next_page_token` to the end so every folder is listed.
        let mut folders = Vec::new();
        let mut page_token = String::new();
        loop {
            match client
                .list_folders(nuncio_proto::v1::ListFoldersRequest {
                    page_size: 0,
                    page_token: page_token.clone(),
                })
                .await
            {
                Ok(response) => {
                    let page = response.into_inner();
                    folders.extend(page.folders);
                    if page.next_page_token.is_empty() {
                        break;
                    }
                    page_token = page.next_page_token;
                }
                Err(status) => {
                    return Self::render_status_error("list_folders", &status, json_mode);
                }
            }
        }

        if json_mode {
            let folders_json: Vec<serde_json::Value> = folders
                .iter()
                .map(|f| {
                    json!({
                        "id": f.id,
                        "name": f.name,
                        "total_messages": f.total_messages,
                        "unread_messages": f.unread_messages,
                    })
                })
                .collect();
            format_json(&json!({ "folders": folders_json }))
        } else {
            format!("Available Mailbox Folders: {} folders found", folders.len())
        }
    }

    /// `mail read`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Mail` API. Returns
    /// the full message, including its (decrypted) body, from the daemon's
    /// real, persistent store.
    async fn handle_read_message(&self, id: &str, json_mode: bool) -> String {
        let mut client = match self.connect_mail_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .get_message(nuncio_proto::v1::GetMessageRequest {
                message_id: id.to_string(),
            })
            .await
        {
            Ok(response) => match response.into_inner().message {
                Some(msg) => {
                    if json_mode {
                        format_json(&json!({ "message": message_proto_to_json(&msg) }))
                    } else {
                        format!(
                            "Message {}: Subject: '{}', From: {}, Date: {}",
                            msg.id,
                            msg.subject,
                            msg.sender,
                            format_timestamp(&msg.received_at)
                        )
                    }
                }
                None => Self::render_error(&format!("message '{}' not found", id), json_mode),
            },
            Err(status) if status.code() == tonic::Code::NotFound => {
                Self::render_error(&format!("message '{}' not found", id), json_mode)
            }
            Err(status) => Self::render_status_error("get_message", &status, json_mode),
        }
    }

    /// `mail mark`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Mail` API. Exactly
    /// one of `read`/`unread` must be set (enforced both by Clap's
    /// `conflicts_with` and this runtime check, so a caller invoking this
    /// programmatically without going through Clap still cannot request an
    /// ambiguous state).
    async fn handle_mark_read(
        &self,
        id: &str,
        read: bool,
        unread: bool,
        json_mode: bool,
    ) -> String {
        if read == unread {
            return Self::render_error(
                "exactly one of --read or --unread must be specified",
                json_mode,
            );
        }

        let mut client = match self.connect_mail_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .mark_read(nuncio_proto::v1::MarkReadRequest {
                message_id: id.to_string(),
                read,
            })
            .await
        {
            Ok(_) => {
                if json_mode {
                    format_json(&json!({ "status": "marked", "id": id, "read": read }))
                } else {
                    format!(
                        "Message '{}' marked as {}",
                        id,
                        if read { "read" } else { "unread" }
                    )
                }
            }
            Err(status) if status.code() == tonic::Code::NotFound => {
                Self::render_error(&format!("message '{}' not found", id), json_mode)
            }
            Err(status) => Self::render_status_error("mark_read", &status, json_mode),
        }
    }

    /// `mail export`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Export` API. The
    /// daemon writes the export to `out` on ITS OWN host filesystem (the
    /// same host this CLI runs on, in this local deployment) using the real
    /// `nuncio_core::export::ExportEngine`, and reports back the real
    /// message/byte counts it actually wrote -- never a fabricated
    /// summary. `account`/`folder` are mutually exclusive (enforced both by
    /// Clap's `conflicts_with` and the daemon's own oneof `scope`); leaving
    /// both unset exports every message.
    async fn handle_mail_export(
        &self,
        format: &str,
        out: &str,
        account: Option<&str>,
        folder: Option<&str>,
        json_mode: bool,
    ) -> String {
        let export_format = match format.parse::<nuncio_core::ExportFormat>() {
            Ok(f) => f,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        let mut client = match self.connect_export_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        let scope = if let Some(account_id) = account {
            Some(nuncio_proto::v1::export_request::Scope::AccountId(
                account_id.to_string(),
            ))
        } else {
            folder.map(|folder_id| {
                nuncio_proto::v1::export_request::Scope::FolderId(folder_id.to_string())
            })
        };

        match client
            .export_mailbox(nuncio_proto::v1::ExportRequest {
                scope,
                format: map_export_format_to_proto(export_format).into(),
                output_path: out.to_string(),
            })
            .await
        {
            Ok(response) => {
                let resp = response.into_inner();
                if json_mode {
                    format_json(&json!({
                        "output_path": resp.output_path,
                        "message_count": resp.message_count,
                        "bytes_written": resp.bytes_written,
                    }))
                } else {
                    format!(
                        "Exported {} message(s) to '{}' ({} bytes)",
                        resp.message_count, resp.output_path, resp.bytes_written
                    )
                }
            }
            Err(status) => Self::render_status_error("export_mailbox", &status, json_mode),
        }
    }

    /// Resolves the gRPC bearer token from the injected vault and dials the
    /// `nuncio.v1.Export` service at `self.grpc_addr`, shared by
    /// [`Self::handle_mail_export`].
    async fn connect_export_client(
        &self,
    ) -> Result<nuncio_proto::client::AuthenticatedExportClient, String> {
        let token_bytes = self
            .secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|e| format!("failed to read gRPC bearer token from vault: {e}"))?;
        let token = hex::encode(token_bytes);

        nuncio_proto::client::connect_export(&self.grpc_addr, &token)
            .await
            .map_err(|e| format!("nunciod daemon unreachable at {}: {e}", self.grpc_addr))
    }

    /// `system audit list`: a real thin gRPC client of the running
    /// `nunciod` daemon's `nuncio.v1.Audit` API. Lists a page of the
    /// daemon's real, persisted WORM audit ledger, sequence ascending.
    async fn handle_audit_list(&self, page_size: u32, json_mode: bool) -> String {
        let mut client = match self.connect_audit_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        // Follow `next_page_token` to the end so the whole ledger is listed,
        // fetching `page_size` records per keyset page.
        let mut records = Vec::new();
        let mut page_token = String::new();
        loop {
            match client
                .list_records(nuncio_proto::v1::ListRecordsRequest {
                    page_size,
                    page_token: page_token.clone(),
                })
                .await
            {
                Ok(response) => {
                    let page = response.into_inner();
                    records.extend(page.records);
                    if page.next_page_token.is_empty() {
                        break;
                    }
                    page_token = page.next_page_token;
                }
                Err(status) => {
                    return Self::render_status_error("list_records", &status, json_mode);
                }
            }
        }

        if json_mode {
            let records_json: Vec<serde_json::Value> =
                records.iter().map(audit_record_proto_to_json).collect();
            format_json(&json!({ "records": records_json }))
        } else if records.is_empty() {
            "No audit records recorded.".to_string()
        } else {
            let mut out =
                String::from("SEQ  ACTOR                ACTION               TIMESTAMP\n");
            for r in records {
                out.push_str(&format!(
                    "{:<4} {:<20} {:<20} {}\n",
                    r.sequence,
                    r.actor,
                    r.action,
                    format_timestamp(&r.timestamp)
                ));
            }
            out
        }
    }

    /// `system audit verify`: a real thin gRPC client of the running
    /// `nunciod` daemon's `nuncio.v1.Audit` API. Re-verifies the ENTIRE
    /// persisted WORM audit ledger's
    /// HMAC hash-chain integrity server-side, using the ledger's real WORM
    /// HMAC key -- never fabricates a `valid` verdict.
    async fn handle_audit_verify(&self, json_mode: bool) -> String {
        let mut client = match self.connect_audit_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .verify_chain(nuncio_proto::v1::VerifyChainRequest {})
            .await
        {
            Ok(response) => {
                let resp = response.into_inner();
                if json_mode {
                    format_json(&json!({
                        "valid": resp.valid,
                        "record_count": resp.record_count,
                        "first_broken_seq": resp.first_broken_seq,
                    }))
                } else if resp.valid {
                    format!("Audit chain OK: {} record(s) verified.", resp.record_count)
                } else {
                    format!(
                        "Audit chain TAMPERED: first broken at sequence {}.",
                        resp.first_broken_seq
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    )
                }
            }
            Err(status) => Self::render_status_error("verify_chain", &status, json_mode),
        }
    }

    /// Resolves the gRPC bearer token from the injected vault and dials the
    /// `nuncio.v1.Audit` service at `self.grpc_addr`, shared by
    /// [`Self::handle_audit_list`] / [`Self::handle_audit_verify`].
    async fn connect_audit_client(
        &self,
    ) -> Result<nuncio_proto::client::AuthenticatedAuditClient, String> {
        let token_bytes = self
            .secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|e| format!("failed to read gRPC bearer token from vault: {e}"))?;
        let token = hex::encode(token_bytes);

        nuncio_proto::client::connect_audit(&self.grpc_addr, &token)
            .await
            .map_err(|e| format!("nunciod daemon unreachable at {}: {e}", self.grpc_addr))
    }

    /// `filter list`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Filters` API. Lists
    /// every persisted filter rule from the daemon's real, persistent
    /// store -- NOT this runner's own ephemeral local `db`, which is thrown
    /// away when this CLI process exits.
    async fn handle_filter_list(&self, json_mode: bool) -> String {
        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        // Follow `next_page_token` to the end so every rule is listed.
        let mut rules = Vec::new();
        let mut page_token = String::new();
        loop {
            match client
                .list_rules(nuncio_proto::v1::ListRulesRequest {
                    page_size: 0,
                    page_token: page_token.clone(),
                })
                .await
            {
                Ok(response) => {
                    let page = response.into_inner();
                    rules.extend(page.rules);
                    if page.next_page_token.is_empty() {
                        break;
                    }
                    page_token = page.next_page_token;
                }
                Err(status) => {
                    return Self::render_status_error("list_rules", &status, json_mode);
                }
            }
        }

        if json_mode {
            let rules_json: Vec<serde_json::Value> =
                rules.iter().map(filter_rule_proto_to_json).collect();
            format_json(&json!({ "rules": rules_json }))
        } else if rules.is_empty() {
            "No filter rules configured.".to_string()
        } else {
            let mut out = String::from("ID         PRIORITY ENABLED NAME                  NSQL\n");
            for r in rules {
                out.push_str(&format!(
                    "{:<10} {:<8} {:<7} {:<20} {}\n",
                    r.id, r.priority, r.enabled, r.name, r.nsql_text
                ));
            }
            out
        }
    }

    /// `filter create`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Filters` API. The daemon parses, validates
    /// (6-pass `NsqlValidator`), and persists the
    /// rule, then reloads its own live `FilterEngine` -- this runner's own
    /// ephemeral local `db` is never touched, so the rule survives this CLI
    /// process exiting.
    async fn handle_filter_create(
        &self,
        name: &str,
        sql: &str,
        priority: i32,
        json_mode: bool,
    ) -> String {
        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .create_rule(nuncio_proto::v1::CreateRuleRequest {
                name: name.to_string(),
                nsql: sql.to_string(),
                priority,
            })
            .await
        {
            Ok(response) => match response.into_inner().rule {
                Some(rule) => {
                    if json_mode {
                        format_json(&json!({ "rule": filter_rule_proto_to_json(&rule) }))
                    } else {
                        format!("✓ Created filter rule '{}' (ID: {}).", rule.name, rule.id)
                    }
                }
                None => Self::render_error(
                    "nunciod daemon accepted create_rule but returned no rule",
                    json_mode,
                ),
            },
            Err(status) => Self::render_status_error("create_rule", &status, json_mode),
        }
    }

    /// `filter delete`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Filters` API. The daemon deletes the persisted
    /// rule and reloads its own live
    /// `FilterEngine` so the removal takes effect immediately.
    async fn handle_filter_delete(&self, id: &str, json_mode: bool) -> String {
        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .delete_rule(nuncio_proto::v1::DeleteRuleRequest {
                rule_id: id.to_string(),
            })
            .await
        {
            Ok(_) => {
                if json_mode {
                    format_json(&json!({ "status": "deleted", "id": id }))
                } else {
                    format!("✓ Filter rule '{}' deleted.", id)
                }
            }
            Err(status) => Self::render_status_error("delete_rule", &status, json_mode),
        }
    }

    /// `filter validate`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Filters` API. Unlike `filter create`, an invalid
    /// rule is never a connection/RPC failure
    /// here -- it is the daemon's honestly reported `valid: false` result,
    /// which this renders as a validation error without ever suggesting
    /// the daemon itself was unreachable or misbehaving.
    async fn handle_filter_validate(&self, sql: &str, json_mode: bool) -> String {
        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .validate_rule(nuncio_proto::v1::ValidateRuleRequest {
                nsql: sql.to_string(),
            })
            .await
        {
            Ok(response) => {
                let response = response.into_inner();
                if json_mode {
                    format_json(&json!({ "valid": response.valid, "error": response.error }))
                } else if response.valid {
                    "✓ NSQL rule is valid.".to_string()
                } else {
                    format!("Validation Error: {}", response.error)
                }
            }
            Err(status) => Self::render_status_error("validate_rule", &status, json_mode),
        }
    }

    /// `filter test`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Filters` API. Dry-run evaluates the given NSQL
    /// rule against a stored message (by
    /// `message_id`, read from the daemon's real, persistent store) or a
    /// fixed synthetic sample when none is given/resolvable -- the rule is
    /// never persisted and no action is ever executed.
    async fn handle_filter_test(
        &self,
        sql: &str,
        message_id: Option<&str>,
        json_mode: bool,
    ) -> String {
        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .preview_rule(nuncio_proto::v1::PreviewRuleRequest {
                nsql: sql.to_string(),
                message_id: message_id.map(str::to_string),
            })
            .await
        {
            Ok(response) => {
                let preview = response.into_inner();
                if json_mode {
                    format_json(&json!({
                        "message_id": preview.message_id,
                        "matched": preview.matched,
                        "matched_rule_id": preview.matched_rule_id,
                        "matched_rule_name": preview.matched_rule_name,
                        "actions_evaluated": preview.actions_evaluated,
                        "execution_time_us": duration_micros(&preview.execution_time),
                        "condition_traces": preview.condition_traces,
                    }))
                } else {
                    format!(
                        "Dry-run evaluation result: matched={}, actions={:?}, elapsed={}us",
                        preview.matched,
                        preview.actions_evaluated,
                        duration_micros(&preview.execution_time)
                    )
                }
            }
            Err(status) => Self::render_status_error("preview_rule", &status, json_mode),
        }
    }

    /// `filter edit`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Filters` API. Only the supplied overrides are
    /// sent; the daemon applies them to the existing persisted rule,
    /// re-validates, persists, and reloads its own live `FilterEngine`.
    async fn handle_filter_edit(
        &self,
        id: &str,
        name: Option<&str>,
        sql: Option<&str>,
        priority: Option<i32>,
        json_mode: bool,
    ) -> String {
        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .update_rule(nuncio_proto::v1::UpdateRuleRequest {
                rule_id: id.to_string(),
                name: name.map(str::to_string),
                nsql: sql.map(str::to_string),
                priority,
            })
            .await
        {
            Ok(response) => match response.into_inner().rule {
                Some(rule) => {
                    if json_mode {
                        format_json(&json!({ "rule": filter_rule_proto_to_json(&rule) }))
                    } else {
                        format!("✓ Updated filter rule '{}'.", rule.id)
                    }
                }
                None => Self::render_error(
                    "nunciod daemon accepted update_rule but returned no rule",
                    json_mode,
                ),
            },
            Err(status) => Self::render_status_error("update_rule", &status, json_mode),
        }
    }

    /// `filter export`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Filters` API, rendering every rule persisted in
    /// the daemon's real, persistent store -- not this runner's own
    /// ephemeral local `db`.
    async fn handle_filter_export(&self, format: &str, json_mode: bool) -> String {
        let format = match parse_rule_export_format(format) {
            Ok(f) => f,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .export_rules(nuncio_proto::v1::ExportRulesRequest {
                format: format.into(),
            })
            .await
        {
            Ok(response) => {
                let content = response.into_inner().content;
                if json_mode {
                    format_json(&json!({ "content": content }))
                } else {
                    content
                }
            }
            Err(status) => Self::render_status_error("export_rules", &status, json_mode),
        }
    }

    /// `filter import`: reads the local file (the file lives on the CLI's
    /// filesystem, which is why this stays a local read), then forwards the
    /// raw content to the daemon for parsing -- so the same
    /// `NsqlParser::parse_rule` and persistence path `filter create` uses is
    /// exercised, and rules land in the daemon's real, persistent store.
    async fn handle_filter_import(&self, file: &str, json_mode: bool) -> String {
        let content = match std::fs::read_to_string(file) {
            Ok(content) => content,
            Err(e) => {
                return if json_mode {
                    format_json_error(&e.to_string())
                } else {
                    format!("Failed to read file: {e}")
                };
            }
        };

        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .import_rules(nuncio_proto::v1::ImportRulesRequest { content })
            .await
        {
            Ok(response) => {
                let response = response.into_inner();
                if json_mode {
                    format_json(&json!({
                        "imported_count": response.imported_count,
                        "errors": response.errors,
                    }))
                } else {
                    let mut out = format!(
                        "✓ Successfully imported {} filter rules.",
                        response.imported_count
                    );
                    for err in &response.errors {
                        out.push_str(&format!("\n  ✗ {err}"));
                    }
                    out
                }
            }
            Err(status) => Self::render_status_error("import_rules", &status, json_mode),
        }
    }

    /// `filter logs`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Filters` API, reading the daemon's real,
    /// persistent filter execution log ledger.
    async fn handle_filter_logs(&self, limit: usize, json_mode: bool) -> String {
        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .get_execution_logs(nuncio_proto::v1::GetExecutionLogsRequest {
                limit: limit as u32,
            })
            .await
        {
            Ok(response) => {
                let logs = response.into_inner().logs;
                if json_mode {
                    let logs_json: Vec<serde_json::Value> = logs
                        .iter()
                        .map(filter_execution_log_proto_to_json)
                        .collect();
                    format_json(&json!({ "logs": logs_json }))
                } else if logs.is_empty() {
                    "No execution logs recorded.".to_string()
                } else {
                    let mut out =
                        String::from("ID   RULE_ID    MSG_ID     ACTION       TIMESTAMP\n");
                    for l in logs {
                        out.push_str(&format!(
                            "{:<4} {:<10} {:<10} {:<12} {}\n",
                            l.id,
                            l.rule_id,
                            l.message_id,
                            l.action_taken,
                            format_timestamp(&l.matched_at)
                        ));
                    }
                    out
                }
            }
            Err(status) => Self::render_status_error("get_execution_logs", &status, json_mode),
        }
    }

    /// Drains the server-streaming `Filters.Triage` RPC, printing (or
    /// accumulating, in `--json` mode) each `TriageProgress` update as it
    /// arrives so the caller can watch a retroactive rescan of the whole
    /// message store progress in real time rather than blocking silently
    /// until it finishes.
    async fn handle_filter_triage(
        &self,
        rule_id: Option<String>,
        chunk_size: Option<u32>,
        json_mode: bool,
    ) -> String {
        let mut client = match self.connect_filters_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        let mut stream = match client
            .triage(nuncio_proto::v1::TriageRequest {
                rule_id,
                chunk_size: chunk_size.unwrap_or(0),
            })
            .await
        {
            Ok(response) => response.into_inner(),
            Err(status) => return Self::render_status_error("triage", &status, json_mode),
        };

        let mut updates = Vec::new();
        loop {
            match stream.message().await {
                Ok(Some(progress)) => {
                    if !json_mode {
                        println!(
                            "scanned={} matched={} actions_applied={} last_message_id={} done={}",
                            progress.scanned_count,
                            progress.matched_count,
                            progress.actions_applied_count,
                            progress.last_message_id,
                            progress.done
                        );
                    }
                    let done = progress.done;
                    updates.push(progress);
                    if done {
                        break;
                    }
                }
                Ok(None) => break,
                Err(status) => {
                    return Self::render_status_error("triage stream", &status, json_mode)
                }
            }
        }

        if json_mode {
            let updates_json: Vec<serde_json::Value> = updates
                .iter()
                .map(|p| {
                    json!({
                        "scanned_count": p.scanned_count,
                        "matched_count": p.matched_count,
                        "actions_applied_count": p.actions_applied_count,
                        "last_message_id": p.last_message_id,
                        "done": p.done,
                    })
                })
                .collect();
            format_json(&json!({ "updates": updates_json }))
        } else {
            match updates.last() {
                Some(last) => format!(
                    "Triage complete: scanned={} matched={} actions_applied={}",
                    last.scanned_count, last.matched_count, last.actions_applied_count
                ),
                None => "Triage produced no progress updates.".to_string(),
            }
        }
    }

    /// Resolves the gRPC bearer token from the injected vault and dials the
    /// `nuncio.v1.Filters` service at `self.grpc_addr`, shared by every
    /// `filter` handler above.
    async fn connect_filters_client(
        &self,
    ) -> Result<nuncio_proto::client::AuthenticatedFiltersClient, String> {
        let token_bytes = self
            .secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|e| format!("failed to read gRPC bearer token from vault: {e}"))?;
        let token = hex::encode(token_bytes);

        nuncio_proto::client::connect_filters(&self.grpc_addr, &token)
            .await
            .map_err(|e| format!("nunciod daemon unreachable at {}: {e}", self.grpc_addr))
    }

    /// `account add`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Accounts` API.
    ///
    /// The account configuration AND `password` are sent to the daemon in a
    /// single `AddAccount` RPC; the daemon is solely responsible for
    /// writing the password to the OS keyring vault and the config to its
    /// persistent database -- this runner's own ephemeral local `db` is
    /// never touched for this command, so the account survives this CLI
    /// process exiting. `password` is never logged here, only forwarded.
    #[allow(clippy::too_many_arguments)]
    async fn handle_add_account(
        &self,
        email: &str,
        protocol: &str,
        collection_url: Option<&str>,
        imap_host: &str,
        imap_port: u16,
        smtp_host: &str,
        smtp_port: u16,
        imap_mode: &str,
        smtp_mode: &str,
        password: &str,
        json_mode: bool,
    ) -> String {
        let imap_tls_mode = match parse_tls_mode(imap_mode) {
            Ok(mode) => mode,
            Err(e) => return Self::render_error(&e, json_mode),
        };
        let smtp_tls_mode = match parse_tls_mode(smtp_mode) {
            Ok(mode) => mode,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        let account_protocol = match parse_account_protocol(protocol) {
            Ok(p) => p,
            Err(e) => return Self::render_error(&e, json_mode),
        };
        let is_caldav = account_protocol == nuncio_proto::v1::AccountProtocol::Caldav;
        if is_caldav && collection_url.map(str::trim).unwrap_or_default().is_empty() {
            return Self::render_error("a caldav account requires --collection-url", json_mode);
        }

        let keyring_key = format!("nuncio/{}", email);
        let account_id = format!("acct-{}", email.replace('@', "-at-").replace('.', "-"));

        // Build the single transport the account speaks. Each transport carries
        // only its own endpoint fields, so a nonsensical mix is unrepresentable.
        let transport = match account_protocol {
            nuncio_proto::v1::AccountProtocol::ImapSmtp => {
                nuncio_proto::v1::account_config::Transport::ImapSmtp(
                    nuncio_proto::v1::ImapSmtpTransport {
                        imap_host: imap_host.to_string(),
                        imap_port: u32::from(imap_port),
                        imap_tls_mode: map_tls_mode_to_proto(imap_tls_mode).into(),
                        smtp_host: smtp_host.to_string(),
                        smtp_port: u32::from(smtp_port),
                        smtp_tls_mode: map_tls_mode_to_proto(smtp_tls_mode).into(),
                    },
                )
            }
            nuncio_proto::v1::AccountProtocol::Jmap => {
                // A JMAP account addresses its provider by a session endpoint
                // host; the `--imap-host` flag supplies it.
                nuncio_proto::v1::account_config::Transport::Jmap(nuncio_proto::v1::JmapTransport {
                    endpoint_host: imap_host.to_string(),
                })
            }
            nuncio_proto::v1::AccountProtocol::Caldav => {
                nuncio_proto::v1::account_config::Transport::Dav(nuncio_proto::v1::DavTransport {
                    collection_url: collection_url.unwrap_or_default().to_string(),
                })
            }
            nuncio_proto::v1::AccountProtocol::Unspecified => {
                return Self::render_error("account protocol is required", json_mode)
            }
        };

        let proto_config = nuncio_proto::v1::AccountConfig {
            id: account_id.clone(),
            name: email.to_string(),
            email_address: email.to_string(),
            keyring_secret_key: keyring_key.clone(),
            sync_interval: Some(nuncio_proto::time::duration_from_secs(300)),
            transport: Some(transport),
        };

        let mut client = match self.connect_accounts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .add_account(nuncio_proto::v1::AddAccountRequest {
                config: Some(proto_config),
                password: password.to_string(),
            })
            .await
        {
            Ok(response) => {
                // Prefer the daemon-confirmed id from the persisted config it
                // echoes back; fall back to the id this call requested if
                // the response is somehow missing its config.
                let account_id = response
                    .into_inner()
                    .config
                    .map(|c| c.id)
                    .unwrap_or(account_id);
                let is_jmap = account_protocol == nuncio_proto::v1::AccountProtocol::Jmap;
                if is_caldav {
                    let collection = collection_url.unwrap_or_default();
                    if json_mode {
                        format_json(&json!({
                            "configured": true,
                            "account_id": account_id,
                            "email": email,
                            "protocol": "caldav",
                            "collection_url": collection,
                            "keyring_key": keyring_key
                        }))
                    } else {
                        format!(
                            "CalDAV account '{email}' (ID: {account_id}) added via nunciod daemon with collection URL {collection}"
                        )
                    }
                } else if is_jmap {
                    if json_mode {
                        format_json(&json!({
                            "configured": true,
                            "account_id": account_id,
                            "email": email,
                            "protocol": "jmap",
                            "endpoint_host": imap_host,
                            "keyring_key": keyring_key
                        }))
                    } else {
                        format!(
                            "JMAP account '{email}' (ID: {account_id}) added via nunciod daemon with endpoint host {imap_host}"
                        )
                    }
                } else if json_mode {
                    format_json(&json!({
                        "configured": true,
                        "account_id": account_id,
                        "email": email,
                        "imap_host": imap_host,
                        "imap_port": imap_port,
                        "imap_mode": imap_mode,
                        "smtp_host": smtp_host,
                        "smtp_port": smtp_port,
                        "smtp_mode": smtp_mode,
                        "keyring_key": keyring_key
                    }))
                } else {
                    format!(
                        "Account '{}' (ID: {}) added via nunciod daemon and configured for IMAP ({}:{}, mode: {}) and SMTP ({}:{}, mode: {})",
                        email, account_id, imap_host, imap_port, imap_mode, smtp_host, smtp_port, smtp_mode
                    )
                }
            }
            Err(status) => Self::render_status_error("add_account", &status, json_mode),
        }
    }

    /// `account list`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Accounts` API. The daemon's `ListAccounts`
    /// response never
    /// contains password credentials (see `nunciod::grpc`'s `Accounts`
    /// implementation), so there is nothing to scrub here.
    async fn handle_accounts_list(&self, json_mode: bool) -> String {
        let mut client = match self.connect_accounts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .list_accounts(nuncio_proto::v1::ListAccountsRequest {})
            .await
        {
            Ok(response) => {
                let accounts = response.into_inner().accounts;
                if json_mode {
                    let accounts_json: Vec<serde_json::Value> = accounts
                        .iter()
                        .map(|a| {
                            let mut base = json!({
                                "id": a.id,
                                "name": a.name,
                                "email_address": a.email_address,
                                "keyring_secret_key": a.keyring_secret_key,
                                "sync_interval_secs": duration_secs(&a.sync_interval),
                            });
                            merge_json(&mut base, account_transport_json(a));
                            base
                        })
                        .collect();
                    format_json(&json!({ "accounts": accounts_json }))
                } else {
                    let mut out = format!(
                        "Configured Accounts: {} account(s) registered",
                        accounts.len()
                    );
                    for a in &accounts {
                        out.push_str(&format!(
                            "\n  [{}] {} <{}>  {}",
                            a.id,
                            a.name,
                            a.email_address,
                            account_transport_summary(a),
                        ));
                    }
                    out
                }
            }
            Err(status) => Self::render_status_error("list_accounts", &status, json_mode),
        }
    }

    /// Fetches every account from the daemon and returns the one matching
    /// `id`, or an error string. There is deliberately no single-account
    /// `GetAccount` RPC in the contract, so `account show`/`edit` filter the
    /// `ListAccounts` result client-side -- always over the real daemon API,
    /// never this runner's own ephemeral local `db`.
    async fn fetch_account_by_id(
        &self,
        id: &str,
    ) -> Result<nuncio_proto::v1::AccountConfig, String> {
        let mut client = self.connect_accounts_client().await?;
        let accounts = client
            .list_accounts(nuncio_proto::v1::ListAccountsRequest {})
            .await
            .map_err(|status| Self::describe_status_error("list_accounts", &status))?
            .into_inner()
            .accounts;
        accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| format!("account '{id}' not found"))
    }

    /// `account show`: a real thin gRPC client of the daemon's
    /// `nuncio.v1.Accounts/ListAccounts` API, filtered client-side to a single
    /// account. Reads ONLY the daemon's persistent store -- never this
    /// runner's own ephemeral local `db`.
    async fn handle_account_show(&self, id: &str, json_mode: bool) -> String {
        let account = match self.fetch_account_by_id(id).await {
            Ok(account) => account,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        if json_mode {
            let mut base = json!({
                "id": account.id,
                "name": account.name,
                "email_address": account.email_address,
                "keyring_secret_key": account.keyring_secret_key,
                "sync_interval_secs": duration_secs(&account.sync_interval),
            });
            merge_json(&mut base, account_transport_json(&account));
            format_json(&base)
        } else {
            format!(
                "Account [{}] {} <{}>  {}",
                account.id,
                account.name,
                account.email_address,
                account_transport_summary(&account),
            )
        }
    }

    /// `account edit`: a real thin gRPC client of the daemon's
    /// `nuncio.v1.Accounts/UpdateAccount` API. Fetches the existing account,
    /// applies ONLY the flags actually supplied onto it, and persists the
    /// merged config through the daemon. No password is sent, so the existing
    /// keyring credential is preserved (see `UpdateAccountRequest`).
    #[allow(clippy::too_many_arguments)]
    async fn handle_account_edit(
        &self,
        id: &str,
        email: Option<&str>,
        imap_host: Option<&str>,
        imap_port: Option<u16>,
        smtp_host: Option<&str>,
        smtp_port: Option<u16>,
        imap_mode: Option<&str>,
        smtp_mode: Option<&str>,
        json_mode: bool,
    ) -> String {
        let mut config = match self.fetch_account_by_id(id).await {
            Ok(config) => config,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        if let Some(email) = email {
            config.email_address = email.to_string();
        }

        // The IMAP/SMTP endpoint flags only apply to an imap-smtp account; the
        // transport oneof makes the alternatives (JMAP endpoint, DAV URL)
        // structurally distinct, so applying these to another transport would
        // be meaningless -- reject it rather than silently ignoring the flags.
        let touches_imap_smtp_fields = imap_host.is_some()
            || imap_port.is_some()
            || smtp_host.is_some()
            || smtp_port.is_some()
            || imap_mode.is_some()
            || smtp_mode.is_some();
        if touches_imap_smtp_fields {
            match &mut config.transport {
                Some(nuncio_proto::v1::account_config::Transport::ImapSmtp(t)) => {
                    if let Some(host) = imap_host {
                        t.imap_host = host.to_string();
                    }
                    if let Some(port) = imap_port {
                        t.imap_port = u32::from(port);
                    }
                    if let Some(host) = smtp_host {
                        t.smtp_host = host.to_string();
                    }
                    if let Some(port) = smtp_port {
                        t.smtp_port = u32::from(port);
                    }
                    if let Some(mode) = imap_mode {
                        match parse_tls_mode(mode) {
                            Ok(mode) => t.imap_tls_mode = map_tls_mode_to_proto(mode).into(),
                            Err(e) => return Self::render_error(&e, json_mode),
                        }
                    }
                    if let Some(mode) = smtp_mode {
                        match parse_tls_mode(mode) {
                            Ok(mode) => t.smtp_tls_mode = map_tls_mode_to_proto(mode).into(),
                            Err(e) => return Self::render_error(&e, json_mode),
                        }
                    }
                }
                _ => {
                    return Self::render_error(
                        "the IMAP/SMTP flags only apply to an imap-smtp account",
                        json_mode,
                    )
                }
            }
        }

        let mut client = match self.connect_accounts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .update_account(nuncio_proto::v1::UpdateAccountRequest {
                config: Some(config),
                password: None,
            })
            .await
        {
            Ok(_) => {
                if json_mode {
                    format_json(&json!({ "status": "updated", "id": id }))
                } else {
                    format!("Account '{id}' updated via nunciod daemon.")
                }
            }
            Err(status) => Self::render_status_error("update_account", &status, json_mode),
        }
    }

    /// `account delete`: a real thin gRPC client of the daemon's
    /// `nuncio.v1.Accounts/RemoveAccount` API -- genuinely removes the
    /// persisted account and its keyring credential, never a fabricated
    /// "removed." string.
    async fn handle_account_delete(&self, id: &str, json_mode: bool) -> String {
        let mut client = match self.connect_accounts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .remove_account(nuncio_proto::v1::RemoveAccountRequest {
                account_id: id.to_string(),
            })
            .await
        {
            Ok(_) => {
                if json_mode {
                    format_json(&json!({ "status": "deleted", "id": id }))
                } else {
                    format!("Account '{id}' removed via nunciod daemon.")
                }
            }
            Err(status) => Self::render_status_error("remove_account", &status, json_mode),
        }
    }

    /// `account test`: a real thin gRPC client of the daemon's
    /// `nuncio.v1.Accounts/TestAccountConnection` API. Reports the GENUINE
    /// per-protocol outcome the daemon measured -- never a fabricated
    /// "connection test OK (24ms latency)". The two legs verify different
    /// depths (see the RPC's proto doc): IMAP is a full login, SMTP is
    /// transport reachability only (no AUTH), so the output labels them
    /// distinctly and never implies SMTP authentication was verified.
    async fn handle_account_test(&self, id: &str, json_mode: bool) -> String {
        let mut client = match self.connect_accounts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .test_account_connection(nuncio_proto::v1::TestAccountConnectionRequest {
                account_id: id.to_string(),
            })
            .await
        {
            Ok(response) => {
                let report = response.into_inner();
                if json_mode {
                    format_json(&json!({
                        "id": id,
                        "imap_ok": report.imap_ok,
                        "smtp_ok": report.smtp_ok,
                        "imap_error": report.imap_error,
                        "smtp_error": report.smtp_error,
                    }))
                } else {
                    // Label each leg by what it actually verifies: IMAP is a
                    // full login, SMTP is transport reachability only (no
                    // AUTH). The wording must never imply SMTP authentication
                    // was checked when it was not.
                    let imap = match &report.imap_error {
                        Some(e) => format!("IMAP (login): FAIL ({e})"),
                        None if report.imap_ok => "IMAP (login): OK".to_string(),
                        None => "IMAP (login): FAIL".to_string(),
                    };
                    let smtp = match &report.smtp_error {
                        Some(e) => format!("SMTP (reachability, no auth): FAIL ({e})"),
                        None if report.smtp_ok => "SMTP (reachability, no auth): OK".to_string(),
                        None => "SMTP (reachability, no auth): FAIL".to_string(),
                    };
                    format!("Connection test for '{id}'\n  {imap}\n  {smtp}")
                }
            }
            Err(status) => Self::render_status_error("test_account_connection", &status, json_mode),
        }
    }

    /// Resolves the gRPC bearer token from the injected vault and dials the
    /// `nuncio.v1.Accounts` service at `self.grpc_addr`, shared by
    /// [`Self::handle_add_account`] and [`Self::handle_accounts_list`].
    async fn connect_accounts_client(
        &self,
    ) -> Result<nuncio_proto::client::AuthenticatedAccountsClient, String> {
        let token_bytes = self
            .secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|e| format!("failed to read gRPC bearer token from vault: {e}"))?;
        let token = hex::encode(token_bytes);

        nuncio_proto::client::connect_accounts(&self.grpc_addr, &token)
            .await
            .map_err(|e| format!("nunciod daemon unreachable at {}: {e}", self.grpc_addr))
    }

    /// Resolves the gRPC bearer token from the injected vault and dials the
    /// `nuncio.v1.Mail` service at `self.grpc_addr`, shared by every
    /// `mail`/`folder` read-path handler above.
    async fn connect_mail_client(
        &self,
    ) -> Result<nuncio_proto::client::AuthenticatedMailClient, String> {
        let token_bytes = self
            .secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|e| format!("failed to read gRPC bearer token from vault: {e}"))?;
        let token = hex::encode(token_bytes);

        nuncio_proto::client::connect_mail(&self.grpc_addr, &token)
            .await
            .map_err(|e| format!("nunciod daemon unreachable at {}: {e}", self.grpc_addr))
    }

    /// Resolves the gRPC bearer token from the injected vault and dials the
    /// `nuncio.v1.Calendar` service at `self.grpc_addr`, shared by
    /// [`Self::handle_cal_list`] and [`Self::handle_cal_sync`].
    async fn connect_calendar_client(
        &self,
    ) -> Result<nuncio_proto::client::AuthenticatedCalendarClient, String> {
        let token_bytes = self
            .secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|e| format!("failed to read gRPC bearer token from vault: {e}"))?;
        let token = hex::encode(token_bytes);

        nuncio_proto::client::connect_calendar(&self.grpc_addr, &token)
            .await
            .map_err(|e| format!("nunciod daemon unreachable at {}: {e}", self.grpc_addr))
    }

    /// `cal list`: a real thin gRPC client of the running `nunciod` daemon's
    /// `nuncio.v1.Calendar/ListEvents` API. Returns only genuinely persisted
    /// events from the daemon's real store -- never a fabricated
    /// "0 events found".
    async fn handle_cal_list(
        &self,
        account: &str,
        calendar: &str,
        start: i64,
        end: i64,
        json_mode: bool,
    ) -> String {
        let mut client = match self.connect_calendar_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        // Follow `next_page_token` to the end so every event in the window is
        // listed, transparently exercising keyset pagination.
        let mut events = Vec::new();
        let mut page_token = String::new();
        loop {
            match client
                .list_events(nuncio_proto::v1::ListEventsRequest {
                    account_id: account.to_string(),
                    calendar_id: calendar.to_string(),
                    start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(start)),
                    end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(end)),
                    page_size: 0,
                    page_token: page_token.clone(),
                })
                .await
            {
                Ok(response) => {
                    let page = response.into_inner();
                    events.extend(page.events);
                    if page.next_page_token.is_empty() {
                        break;
                    }
                    page_token = page.next_page_token;
                }
                Err(status) => {
                    return Self::render_status_error("list_events", &status, json_mode);
                }
            }
        }

        if json_mode {
            let events_json: Vec<serde_json::Value> =
                events.iter().map(calendar_event_proto_to_json).collect();
            format_json(&json!({
                "account": account,
                "calendar": calendar,
                "events": events_json
            }))
        } else {
            format!("Calendar Events: {} event(s) found", events.len())
        }
    }

    /// `cal sync`: a real thin gRPC client of the running `nunciod` daemon's
    /// `nuncio.v1.Calendar/Sync` API. With no persisted per-account CalDAV
    /// configuration, the daemon's production `Sync` path returns an honest
    /// error -- this NEVER prints a fabricated "Calendar synchronization
    /// started" the way this command used to.
    async fn handle_cal_sync(
        &self,
        account: &str,
        calendar: &str,
        start: i64,
        end: i64,
        json_mode: bool,
    ) -> String {
        let mut client = match self.connect_calendar_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .sync(nuncio_proto::v1::CalendarSyncRequest {
                account_id: account.to_string(),
                calendar_id: calendar.to_string(),
                start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(start)),
                end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(end)),
            })
            .await
        {
            Ok(response) => {
                let synced_count = response.into_inner().synced_count;
                if json_mode {
                    format_json(&json!({
                        "status": "calendar_sync_complete",
                        "synced_count": synced_count,
                    }))
                } else {
                    format!("Calendar synchronization complete: {synced_count} event(s) synced")
                }
            }
            Err(status) => Self::render_status_error("calendar sync", &status, json_mode),
        }
    }

    /// Resolves the gRPC bearer token from the injected vault and dials the
    /// `nuncio.v1.Contacts` service at `self.grpc_addr`, shared by every
    /// `contact` handler above.
    async fn connect_contacts_client(
        &self,
    ) -> Result<nuncio_proto::client::AuthenticatedContactsClient, String> {
        let token_bytes = self
            .secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|e| format!("failed to read gRPC bearer token from vault: {e}"))?;
        let token = hex::encode(token_bytes);

        nuncio_proto::client::connect_contacts(&self.grpc_addr, &token)
            .await
            .map_err(|e| format!("nunciod daemon unreachable at {}: {e}", self.grpc_addr))
    }

    /// `contact list`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Contacts/ListContacts` API. Returns only
    /// genuinely persisted contacts from the daemon's real store -- never a
    /// fabricated "0 contacts found".
    async fn handle_contact_list(&self, account: &str, json_mode: bool) -> String {
        let mut client = match self.connect_contacts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        let contacts = match self.fetch_all_contacts(&mut client, account).await {
            Ok(contacts) => contacts,
            Err(status) => {
                return Self::render_status_error("list_contacts", &status, json_mode);
            }
        };

        if json_mode {
            let contacts_json: Vec<serde_json::Value> =
                contacts.iter().map(contact_proto_to_json).collect();
            format_json(&json!({ "account": account, "contacts": contacts_json }))
        } else {
            format!(
                "Contacts: {} contact(s) found in address book.",
                contacts.len()
            )
        }
    }

    /// Follows `next_page_token` to fetch every contact in `account`'s address
    /// book over the keyset-paginated `ListContacts` RPC, shared by
    /// [`Self::handle_contact_list`] and [`Self::handle_contact_search`].
    async fn fetch_all_contacts(
        &self,
        client: &mut nuncio_proto::client::AuthenticatedContactsClient,
        account: &str,
    ) -> Result<Vec<nuncio_proto::v1::Contact>, tonic::Status> {
        let mut contacts = Vec::new();
        let mut page_token = String::new();
        loop {
            let page = client
                .list_contacts(nuncio_proto::v1::ListContactsRequest {
                    account_id: account.to_string(),
                    page_size: 0,
                    page_token: page_token.clone(),
                })
                .await?
                .into_inner();
            contacts.extend(page.contacts);
            if page.next_page_token.is_empty() {
                break;
            }
            page_token = page.next_page_token;
        }
        Ok(contacts)
    }

    /// `contact search`: fetches `account`'s full `ListContacts` result over
    /// the real gRPC API, then filters it client-side by `query` against
    /// display name, organization, and email address (case-insensitive) --
    /// there is no server-side search RPC on the `Contacts` surface.
    async fn handle_contact_search(&self, account: &str, query: &str, json_mode: bool) -> String {
        let mut client = match self.connect_contacts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        let contacts = match self.fetch_all_contacts(&mut client, account).await {
            Ok(contacts) => contacts,
            Err(status) => {
                return Self::render_status_error("list_contacts", &status, json_mode);
            }
        };

        let matches: Vec<_> = contacts
            .into_iter()
            .filter(|c| contact_matches_query(c, query))
            .collect();
        if json_mode {
            let contacts_json: Vec<serde_json::Value> =
                matches.iter().map(contact_proto_to_json).collect();
            format_json(&json!({
                "account": account,
                "query": query,
                "contacts": contacts_json
            }))
        } else {
            format!(
                "Search ('{}'): {} contact(s) matched.",
                query,
                matches.len()
            )
        }
    }

    /// `contact add`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Contacts/CreateContact` API. Persists directly to
    /// the daemon's own store (never a CardDAV write-back), so the created
    /// contact survives past this CLI process exiting and is visible to a
    /// later, separate `contact list` invocation against the same daemon.
    async fn handle_contact_add(
        &self,
        account: &str,
        name: &str,
        email: &str,
        org: Option<&str>,
        json_mode: bool,
    ) -> String {
        let mut client = match self.connect_contacts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .create_contact(nuncio_proto::v1::CreateContactRequest {
                account_id: account.to_string(),
                display_name: name.to_string(),
                organization: org.map(str::to_string),
                emails: vec![nuncio_proto::v1::ContactEmail {
                    email: email.to_string(),
                    label: "work".to_string(),
                    is_primary: true,
                }],
                phones: Vec::new(),
            })
            .await
        {
            Ok(response) => match response.into_inner().contact {
                Some(contact) => {
                    if json_mode {
                        format_json(&json!({
                            "status": "contact_created",
                            "contact": contact_proto_to_json(&contact),
                        }))
                    } else {
                        format!(
                            "✓ Contact '{}' saved to address book.",
                            contact.display_name
                        )
                    }
                }
                None => Self::render_error(
                    "nunciod daemon accepted create_contact but returned no contact",
                    json_mode,
                ),
            },
            Err(status) => Self::render_status_error("create_contact", &status, json_mode),
        }
    }

    /// `contact sync`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Contacts/Sync` API. With no persisted per-account
    /// CardDAV configuration, the daemon's production `Sync` path returns an
    /// honest error -- this NEVER prints a fabricated success.
    async fn handle_contact_sync(&self, account: &str, json_mode: bool) -> String {
        let mut client = match self.connect_contacts_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .sync(nuncio_proto::v1::ContactsSyncRequest {
                account_id: account.to_string(),
            })
            .await
        {
            Ok(response) => {
                let synced_count = response.into_inner().synced_count;
                if json_mode {
                    format_json(&json!({
                        "status": "contacts_sync_complete",
                        "synced_count": synced_count,
                    }))
                } else {
                    format!("Contacts synchronization complete: {synced_count} contact(s) synced")
                }
            }
            Err(status) => Self::render_status_error("contacts sync", &status, json_mode),
        }
    }

    /// Formats an error message consistently for both JSON and human-text
    /// output modes.
    fn render_error(message: &str, json_mode: bool) -> String {
        if json_mode {
            format_json_error(message)
        } else {
            format!("Error: {message}")
        }
    }

    /// Extracts the clean, user-facing text from a `tonic::Status` returned
    /// by a daemon RPC. Uses the server-provided message when present;
    /// falls back to a short "<code> error" form when the message is empty.
    /// Callers MUST use this (never the `Display`/`Debug` of `tonic::Status`
    /// directly) for user-facing output, since `Display`/`Debug` on
    /// `tonic::Status` dumps raw gRPC internals (`status: ..., metadata:
    /// MetadataMap { ... }`) that are meaningless to a CLI user.
    fn clean_status_message(status: &tonic::Status) -> String {
        let message = status.message();
        if message.is_empty() {
            format!("{:?} error", status.code())
        } else {
            message.to_string()
        }
    }

    /// Strips the `ERROR_REASON_` prefix off a decoded [`v1::ErrorReason`]'s
    /// generated `as_str_name()`, so the CLI renders the short, readable form
    /// (`ACCOUNT_NOT_FOUND`) rather than the wire enum's full name.
    fn reason_name(reason: nuncio_proto::v1::ErrorReason) -> String {
        reason
            .as_str_name()
            .trim_start_matches("ERROR_REASON_")
            .to_string()
    }

    /// Builds the full human-readable text for a failed daemon RPC: the
    /// server-provided message, the gRPC status code, and -- when the daemon
    /// attached a WS-A1 [`nuncio_proto::v1::ErrorInfo`] to the status details
    /// -- the typed [`nuncio_proto::v1::ErrorReason`]. Falls back to just the
    /// code when no typed details are present (e.g. a status raised by a
    /// library this client talks to that predates `ErrorInfo`).
    fn describe_status_error(context: &str, status: &tonic::Status) -> String {
        let message = Self::clean_status_message(status);
        let code = status.code();
        match nuncio_proto::errors::error_reason(status) {
            Some(reason) => format!(
                "nunciod daemon rejected {context}: {message} (code: {code:?}, reason: {})",
                Self::reason_name(reason)
            ),
            None => format!("nunciod daemon rejected {context}: {message} (code: {code:?})"),
        }
    }

    /// Renders a failed daemon RPC for both text and `--json` output modes,
    /// decoding the WS-A1 `ErrorInfo` from the status details when present.
    ///
    /// Text mode gets the human message, the gRPC code, and the typed reason
    /// (see [`Self::describe_status_error`]). JSON mode gets a coherent
    /// `{error, code, reason, metadata}` shape via
    /// [`format_json_error_with_info`] -- `reason`/`metadata` are omitted
    /// (rather than fabricated) when the status carried no typed
    /// `ErrorInfo`, so a caller can tell "no reason given" apart from an
    /// actual reason.
    fn render_status_error(context: &str, status: &tonic::Status, json_mode: bool) -> String {
        if json_mode {
            let info = nuncio_proto::errors::error_info(status);
            let message = format!(
                "nunciod daemon rejected {context}: {}",
                Self::clean_status_message(status)
            );
            let reason = info.as_ref().map(|i| Self::reason_name(i.reason()));
            let metadata = info.map(|i| i.metadata).unwrap_or_default();
            format_json_error_with_info(
                &message,
                Some(format!("{:?}", status.code())),
                reason,
                metadata.into_iter().collect(),
            )
        } else {
            format!("Error: {}", Self::describe_status_error(context, status))
        }
    }

    /// `system status`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.System` API.
    ///
    /// Resolves the bearer token from the injected `SecretManager`, dials
    /// the configured gRPC daemon address via
    /// `nuncio_proto::client::connect_system`, and calls `GetStatus`. If the
    /// daemon is unreachable or rejects the call, this returns a clear,
    /// honest error — it never fabricates a status. This deliberately does
    /// NOT auto-spawn the daemon.
    async fn handle_system_status(&self, json_mode: bool) -> String {
        match self.query_daemon_status().await {
            Ok(status) => {
                let uptime_secs = status
                    .uptime
                    .as_ref()
                    .map(nuncio_proto::time::duration_to_secs)
                    .unwrap_or(0);
                if json_mode {
                    let accounts: Vec<_> = status
                        .account_sync_states
                        .iter()
                        .map(|a| {
                            json!({
                                "account_id": a.account_id,
                                "state": sync_state_label(a.state),
                                "last_synced": a
                                    .last_synced
                                    .as_ref()
                                    .map(nuncio_proto::time::timestamp_to_unix_secs),
                                "last_error": a.last_error,
                            })
                        })
                        .collect();
                    format_json(&json!({
                        "engine_status": status.engine_status,
                        "version": status.version,
                        "ready": status.ready,
                        "db_healthy": status.db_healthy,
                        "uptime_secs": uptime_secs,
                        "accounts_loaded": status.accounts_loaded,
                        "unread_count": status.unread_count,
                        "outbox_depth": status.outbox_depth,
                        "last_error": status.last_error,
                        "accounts": accounts,
                    }))
                } else {
                    let mut out = format!(
                        "Nuncio daemon status: {} (nunciod v{}, {})\n",
                        status.engine_status, status.version, self.grpc_addr
                    );
                    out.push_str(&format!(
                        "  ready: {}  db_healthy: {}\n",
                        status.ready, status.db_healthy
                    ));
                    out.push_str(&format!("  uptime: {uptime_secs}s\n"));
                    out.push_str(&format!(
                        "  accounts_loaded: {}  unread: {}  outbox_depth: {}\n",
                        status.accounts_loaded, status.unread_count, status.outbox_depth
                    ));
                    out.push_str(&format!(
                        "  last_error: {}",
                        status.last_error.as_deref().unwrap_or("<none>")
                    ));
                    if status.account_sync_states.is_empty() {
                        out.push_str("\n  accounts: <none>");
                    } else {
                        out.push_str("\n  accounts:");
                        for a in &status.account_sync_states {
                            out.push_str(&format!(
                                "\n    {}: {}",
                                a.account_id,
                                sync_state_label(a.state)
                            ));
                            if let Some(ts) = a.last_synced.as_ref() {
                                out.push_str(&format!(
                                    " (last synced: {})",
                                    nuncio_proto::time::timestamp_to_unix_secs(ts)
                                ));
                            }
                            if let Some(err) = a.last_error.as_deref() {
                                out.push_str(&format!(" [error: {err}]"));
                            }
                        }
                    }
                    out
                }
            }
            Err(e) => {
                if json_mode {
                    format_json_error(&e)
                } else {
                    format!("Error: {e}")
                }
            }
        }
    }

    /// Reads the gRPC bearer token from the injected vault, connects to the
    /// daemon over gRPC, and calls `GetStatus`. Returns the full live health
    /// response on success, or a human-readable error string describing
    /// exactly what failed (vault, connection, or the RPC itself).
    async fn query_daemon_status(&self) -> Result<nuncio_proto::v1::GetStatusResponse, String> {
        let token_bytes = self
            .secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .map_err(|e| format!("failed to read gRPC bearer token from vault: {e}"))?;
        let token = hex::encode(token_bytes);

        let mut client = nuncio_proto::client::connect_system(&self.grpc_addr, &token)
            .await
            .map_err(|e| format!("nunciod daemon unreachable at {}: {e}", self.grpc_addr))?;

        let response = client
            .get_status(nuncio_proto::v1::GetStatusRequest {})
            .await
            .map_err(|status| {
                format!(
                    "nunciod daemon rejected status request: {}",
                    Self::clean_status_message(&status)
                )
            })?
            .into_inner();

        Ok(response)
    }
}

/// Maps a `nuncio.v1.SyncState` wire discriminant onto a short human label
/// for the CLI's `system status` rendering. An unrecognized value renders as
/// "unknown" rather than failing.
fn sync_state_label(state: i32) -> &'static str {
    match nuncio_proto::v1::SyncState::try_from(state)
        .unwrap_or(nuncio_proto::v1::SyncState::Unspecified)
    {
        nuncio_proto::v1::SyncState::Idle => "idle",
        nuncio_proto::v1::SyncState::Syncing => "syncing",
        nuncio_proto::v1::SyncState::Error => "error",
        nuncio_proto::v1::SyncState::Unspecified => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tls_mode_accepts_all_valid_modes_and_rejects_garbage() {
        assert_eq!(
            parse_tls_mode("implicit_tls").expect("valid mode"),
            nuncio_core::TlsMode::ImplicitTls
        );
        assert_eq!(
            parse_tls_mode("start_tls").expect("valid mode"),
            nuncio_core::TlsMode::StartTls
        );
        assert_eq!(
            parse_tls_mode("plain").expect("valid mode"),
            nuncio_core::TlsMode::Plain
        );

        let err = parse_tls_mode("not-a-real-mode").expect_err("garbage mode must be rejected");
        assert!(err.contains("invalid tls mode"));
    }

    #[test]
    fn map_tls_mode_to_proto_mirrors_every_variant() {
        assert_eq!(
            map_tls_mode_to_proto(nuncio_core::TlsMode::ImplicitTls),
            nuncio_proto::v1::TlsMode::ImplicitTls
        );
        assert_eq!(
            map_tls_mode_to_proto(nuncio_core::TlsMode::StartTls),
            nuncio_proto::v1::TlsMode::StartTls
        );
        assert_eq!(
            map_tls_mode_to_proto(nuncio_core::TlsMode::Plain),
            nuncio_proto::v1::TlsMode::Plain
        );
    }

    #[test]
    fn parse_account_protocol_accepts_known_names_and_rejects_garbage() {
        assert_eq!(
            parse_account_protocol("imap-smtp").expect("valid"),
            nuncio_proto::v1::AccountProtocol::ImapSmtp
        );
        assert_eq!(
            parse_account_protocol("jmap").expect("valid"),
            nuncio_proto::v1::AccountProtocol::Jmap
        );
        assert_eq!(
            parse_account_protocol("caldav").expect("valid"),
            nuncio_proto::v1::AccountProtocol::Caldav
        );
        assert_eq!(
            parse_account_protocol("CalDAV").expect("case-insensitive"),
            nuncio_proto::v1::AccountProtocol::Caldav
        );
        let err = parse_account_protocol("smoke-signals").expect_err("garbage rejected");
        assert!(err.contains("invalid protocol"));
    }

    /// `account add --protocol caldav` with no `--collection-url` must be
    /// rejected BEFORE dialing the daemon -- proven by pointing at an address
    /// nothing is listening on and confirming the failure is the validation
    /// error, not a connection error.
    #[tokio::test]
    async fn account_add_caldav_requires_collection_url_before_dialing() {
        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), "127.0.0.1:1".into())
                .await
                .expect("runner init");

        let out = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Add {
                        email: "cal@nuncio.mx".to_string(),
                        protocol: "caldav".to_string(),
                        collection_url: None,
                        imap_host: "unused".to_string(),
                        imap_port: 993,
                        smtp_host: "unused".to_string(),
                        smtp_port: 465,
                        imap_mode: "implicit_tls".to_string(),
                        smtp_mode: "implicit_tls".to_string(),
                        password: crate::args::PasswordArg("pw".to_string()),
                    },
                },
                false,
            )
            .await;
        assert!(out.contains("requires --collection-url"));
        assert!(!out.contains("unreachable"));
    }

    /// `account add` must reject an invalid `--imap-mode`/`--smtp-mode`
    /// string BEFORE ever dialing the daemon -- proven here by pointing at
    /// an address nothing is listening on and confirming the failure is the
    /// validation error, not a connection error.
    #[tokio::test]
    async fn account_add_rejects_invalid_tls_mode_before_dialing_the_daemon() {
        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), "127.0.0.1:1".into())
                .await
                .expect("runner init");

        let bad_imap_mode = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Add {
                        email: "x@y.com".to_string(),
                        protocol: "imap-smtp".to_string(),
                        collection_url: None,
                        imap_host: "imap.y.com".to_string(),
                        imap_port: 993,
                        smtp_host: "smtp.y.com".to_string(),
                        smtp_port: 465,
                        imap_mode: "not-a-real-mode".to_string(),
                        smtp_mode: "implicit_tls".to_string(),
                        password: crate::args::PasswordArg("pw".to_string()),
                    },
                },
                true,
            )
            .await;
        assert!(bad_imap_mode.contains("invalid tls mode"));
        assert!(!bad_imap_mode.contains("unreachable"));

        let bad_smtp_mode = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Add {
                        email: "x@y.com".to_string(),
                        protocol: "imap-smtp".to_string(),
                        collection_url: None,
                        imap_host: "imap.y.com".to_string(),
                        imap_port: 993,
                        smtp_host: "smtp.y.com".to_string(),
                        smtp_port: 465,
                        imap_mode: "implicit_tls".to_string(),
                        smtp_mode: "also-not-real".to_string(),
                        password: crate::args::PasswordArg("pw".to_string()),
                    },
                },
                false,
            )
            .await;
        assert!(bad_smtp_mode.starts_with("Error: invalid tls mode"));
    }

    /// Binds an ephemeral loopback TCP listener, reads back its OS-assigned
    /// address, then immediately drops the listener so the port is free
    /// again. Nothing is listening on the returned address, so connecting
    /// to it deterministically fails with "connection refused" — used to
    /// prove `system status` reports an honest error instead of fabricating
    /// one when no daemon is reachable, without depending on any specific
    /// hardcoded port that might collide with a real service.
    async fn reserve_unreachable_addr() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        drop(listener);
        addr.to_string()
    }

    #[tokio::test]
    async fn system_status_reports_honest_error_when_daemon_unreachable() {
        let addr = reserve_unreachable_addr().await;
        let runner = HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr)
            .await
            .expect("ephemeral runner initializes");

        let json_out = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Status,
                },
                true,
            )
            .await;
        assert!(json_out.contains(r#""status":"error""#));
        assert!(json_out.contains("unreachable"));

        let text_out = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Status,
                },
                false,
            )
            .await;
        assert!(text_out.starts_with("Error: "));
        assert!(text_out.contains("unreachable"));
    }

    #[tokio::test]
    async fn system_status_reports_live_daemon_status_over_grpc_when_reachable() {
        use nuncio_proto::v1::system_server::{System as SystemService, SystemServer};
        use nuncio_proto::v1::{
            Event, GetStatusRequest, GetStatusResponse, ShutdownRequest, ShutdownResponse,
            SubscribeRequest,
        };

        /// Minimal test-only stub of the `nuncio.v1.System` service: no
        /// auth interceptor, just a fixed status. Bearer-token acceptance
        /// itself is proven by `nuncio-proto`'s own client tests and
        /// `nunciod`'s server + E2E tests; this test exists purely to
        /// prove the CLI's connect + call + JSON-format happy path.
        struct StubSystem;

        #[tonic::async_trait]
        impl SystemService for StubSystem {
            async fn get_status(
                &self,
                _request: tonic::Request<GetStatusRequest>,
            ) -> Result<tonic::Response<GetStatusResponse>, tonic::Status> {
                Ok(tonic::Response::new(GetStatusResponse {
                    engine_status: "Ready".to_string(),
                    version: "9.9.9".to_string(),
                    uptime: Some(nuncio_proto::time::duration_from_secs(5)),
                    accounts_loaded: 2,
                    unread_count: 3,
                    last_error: None,
                    outbox_depth: 1,
                    account_sync_states: vec![nuncio_proto::v1::AccountSyncState {
                        account_id: "acct-1".to_string(),
                        state: nuncio_proto::v1::SyncState::Idle as i32,
                        last_synced: Some(nuncio_proto::time::timestamp_from_unix_secs(
                            1_700_000_000,
                        )),
                        last_error: None,
                    }],
                    ready: true,
                    db_healthy: true,
                }))
            }

            // `Subscribe` streaming is exercised by `nunciod`'s own
            // `grpc::tests`; this stub only needs to satisfy the trait so
            // the CLI's `GetStatus` happy
            // path above can compile against the real `System` service
            // definition, so it deliberately returns `unimplemented` rather
            // than fabricating stream behavior no test here relies on.
            type SubscribeStream = std::pin::Pin<
                Box<dyn tokio_stream::Stream<Item = Result<Event, tonic::Status>> + Send + 'static>,
            >;

            async fn subscribe(
                &self,
                _request: tonic::Request<SubscribeRequest>,
            ) -> Result<tonic::Response<Self::SubscribeStream>, tonic::Status> {
                Err(tonic::Status::unimplemented(
                    "subscribe is not exercised by this stub",
                ))
            }

            // `Shutdown` is exercised by `nunciod`'s own `grpc::tests`; this
            // stub only needs to satisfy the trait so the CLI's `GetStatus`
            // happy path above can compile against the real `System` service
            // definition.
            async fn shutdown(
                &self,
                _request: tonic::Request<ShutdownRequest>,
            ) -> Result<tonic::Response<ShutdownResponse>, tonic::Status> {
                Err(tonic::Status::unimplemented(
                    "shutdown is not exercised by this stub",
                ))
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(SystemServer::new(StubSystem))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        let out = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Status,
                },
                true,
            )
            .await;
        assert!(out.contains(r#""status":"ok""#));
        assert!(out.contains(r#""engine_status":"Ready""#));
        assert!(out.contains(r#""version":"9.9.9""#));
        assert!(out.contains(r#""ready":true"#));
        assert!(out.contains(r#""db_healthy":true"#));
        assert!(out.contains(r#""accounts_loaded":2"#));
        assert!(out.contains(r#""unread_count":3"#));
        assert!(out.contains(r#""outbox_depth":1"#));
        assert!(out.contains(r#""account_id":"acct-1""#));

        let text_out = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Status,
                },
                false,
            )
            .await;
        assert!(text_out.contains("Ready"));
        assert!(text_out.contains("9.9.9"));
    }

    /// Reference-client proof: boots a stub `nuncio.v1.Accounts` gRPC server (mirroring
    /// `system_status_reports_live_daemon_status_over_grpc_when_reachable`'s
    /// `StubSystem` pattern above) and drives the real `HeadlessRunner`'s
    /// `account add` / `account list` gRPC client paths against it.
    ///
    /// Real persistence-across-restart and credential-secrecy (never in
    /// SQLite, never in `ListAccounts`) are proven by `nunciod`'s own
    /// `grpc::tests`; this test exists purely to prove the CLI's connect +
    /// call + JSON-format happy path, AND that the password the CLI sends
    /// over the wire is exactly what the caller supplied (never mangled,
    /// dropped, or substituted).
    #[tokio::test]
    async fn account_add_and_list_round_trip_over_grpc_to_a_stub_daemon() {
        use nuncio_proto::v1::accounts_server::{Accounts as AccountsService, AccountsServer};
        use nuncio_proto::v1::{
            AccountConfig as AccountConfigProto, AddAccountRequest, AddAccountResponse,
            ListAccountsRequest, ListAccountsResponse, RemoveAccountRequest, RemoveAccountResponse,
            TestAccountConnectionRequest, TestAccountConnectionResponse, TlsMode as TlsModeProto,
            UpdateAccountRequest, UpdateAccountResponse,
        };
        use std::sync::Mutex;

        /// Minimal test-only stub of `nuncio.v1.Accounts`: records the last
        /// `AddAccountRequest` it received (so this test can assert on
        /// exactly what the CLI sent over the wire, including the
        /// password) and returns a fixed `ListAccounts` response.
        #[derive(Default)]
        struct StubAccounts {
            last_add_request: Arc<Mutex<Option<AddAccountRequest>>>,
        }

        #[tonic::async_trait]
        impl AccountsService for StubAccounts {
            async fn add_account(
                &self,
                request: tonic::Request<AddAccountRequest>,
            ) -> Result<tonic::Response<AddAccountResponse>, tonic::Status> {
                let req = request.into_inner();
                let config = req.config.clone();
                *self
                    .last_add_request
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(AddAccountResponse { config }))
            }

            async fn update_account(
                &self,
                _request: tonic::Request<UpdateAccountRequest>,
            ) -> Result<tonic::Response<UpdateAccountResponse>, tonic::Status> {
                Ok(tonic::Response::new(UpdateAccountResponse {}))
            }

            async fn remove_account(
                &self,
                _request: tonic::Request<RemoveAccountRequest>,
            ) -> Result<tonic::Response<RemoveAccountResponse>, tonic::Status> {
                Ok(tonic::Response::new(RemoveAccountResponse {}))
            }

            async fn test_account_connection(
                &self,
                _request: tonic::Request<TestAccountConnectionRequest>,
            ) -> Result<tonic::Response<TestAccountConnectionResponse>, tonic::Status> {
                Ok(tonic::Response::new(TestAccountConnectionResponse {
                    imap_ok: true,
                    smtp_ok: true,
                    imap_error: None,
                    smtp_error: None,
                }))
            }

            async fn list_accounts(
                &self,
                _request: tonic::Request<ListAccountsRequest>,
            ) -> Result<tonic::Response<ListAccountsResponse>, tonic::Status> {
                Ok(tonic::Response::new(ListAccountsResponse {
                    accounts: vec![AccountConfigProto {
                        id: "acct-stub-1".to_string(),
                        name: "Stub Account".to_string(),
                        email_address: "stub@nuncio.mx".to_string(),
                        keyring_secret_key: "nuncio/acct-stub-1".to_string(),
                        sync_interval: Some(nuncio_proto::time::duration_from_secs(300)),
                        transport: Some(nuncio_proto::v1::account_config::Transport::ImapSmtp(
                            nuncio_proto::v1::ImapSmtpTransport {
                                imap_host: "imap.nuncio.mx".to_string(),
                                imap_port: 993,
                                imap_tls_mode: TlsModeProto::ImplicitTls.into(),
                                smtp_host: "smtp.nuncio.mx".to_string(),
                                smtp_port: 465,
                                smtp_tls_mode: TlsModeProto::ImplicitTls.into(),
                            },
                        )),
                    }],
                }))
            }
        }

        let stub = StubAccounts::default();
        let probe = stub.last_add_request.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(AccountsServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        let add_out = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Add {
                        email: "james.maes@kof22.com".to_string(),
                        protocol: "imap-smtp".to_string(),
                        collection_url: None,
                        imap_host: "mail.kof22.com".to_string(),
                        imap_port: 993,
                        smtp_host: "mail.kof22.com".to_string(),
                        smtp_port: 465,
                        imap_mode: "implicit_tls".to_string(),
                        smtp_mode: "implicit_tls".to_string(),
                        password: crate::args::PasswordArg("s3cr3t-cli-password".to_string()),
                    },
                },
                true,
            )
            .await;
        assert!(add_out.contains(r#""email":"james.maes@kof22.com""#));
        assert!(add_out.contains("acct-james-maes-at-kof22-com"));
        // The password must never be echoed back in the CLI's own output.
        assert!(!add_out.contains("s3cr3t-cli-password"));

        // ...but it MUST have been sent to the daemon intact, exactly as
        // the caller supplied it.
        let recorded = probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received an add_account request");
        assert_eq!(recorded.password, "s3cr3t-cli-password");
        assert_eq!(
            recorded
                .config
                .expect("config present on the request")
                .keyring_secret_key,
            "nuncio/james.maes@kof22.com"
        );

        let list_out = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::List,
                },
                true,
            )
            .await;
        assert!(list_out.contains("acct-stub-1"));
        assert!(list_out.contains("stub@nuncio.mx"));
        assert!(list_out.contains(r#""protocol":"imap-smtp""#));
        assert!(list_out.contains(r#""imap_host":"imap.nuncio.mx""#));

        let list_out_text = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::List,
                },
                false,
            )
            .await;
        assert!(list_out_text.contains("1 account(s) registered"));
        // Plain output must show each account's details, not just the count.
        assert!(list_out_text.contains("stub@nuncio.mx"));
        assert!(list_out_text.contains("imap.nuncio.mx"));

        // `account show` reads the daemon's ListAccounts result (filtered
        // client-side), not the runner's own ephemeral local db.
        let show_out = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Show {
                        id: "acct-stub-1".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(show_out.contains("stub@nuncio.mx"));
        assert!(show_out.contains("acct-stub-1"));

        // `account edit` drives the real UpdateAccount RPC (stub accepts it).
        let edit_out = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Edit {
                        id: "acct-stub-1".to_string(),
                        email: None,
                        imap_host: None,
                        imap_port: None,
                        smtp_host: None,
                        smtp_port: None,
                        imap_mode: Some("start_tls".to_string()),
                        smtp_mode: None,
                    },
                },
                true,
            )
            .await;
        assert!(edit_out.contains(r#""status":"updated""#));

        // A garbage transport mode is rejected client-side before any RPC.
        let bad_edit = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Edit {
                        id: "acct-stub-1".to_string(),
                        email: None,
                        imap_host: None,
                        imap_port: None,
                        smtp_host: None,
                        smtp_port: None,
                        imap_mode: Some("nonsense".to_string()),
                        smtp_mode: None,
                    },
                },
                true,
            )
            .await;
        assert!(bad_edit.contains("invalid tls mode"));

        // `account test` reports the daemon's genuine per-protocol result --
        // never a fabricated "OK (24ms latency)".
        let test_out = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Test {
                        id: "acct-stub-1".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(test_out.contains(r#""imap_ok":true"#));
        assert!(test_out.contains(r#""smtp_ok":true"#));
        assert!(!test_out.contains("latency"));

        // `account delete` drives the real RemoveAccount RPC.
        let delete_out = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Delete {
                        id: "acct-stub-1".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(delete_out.contains(r#""status":"deleted""#));
    }

    /// Reference-client proof: boots a stub `nuncio.v1.Mail` gRPC server
    /// (mirroring the `Accounts` stub pattern above) and drives the real
    /// `HeadlessRunner`'s
    /// `mail sync`/`mail list`/`mail read`/`mail mark`/`mail search`/
    /// `folder list` gRPC client paths against it.
    ///
    /// Real persistence (`sync` actually fetching and persisting messages,
    /// `mark_read` actually flipping the flag in the daemon's store,
    /// `list`/`get` returning REAL synced data, `MessageFlagsChanged`
    /// streaming) is proven by `nunciod`'s own `grpc::tests`; this test
    /// exists purely to prove the CLI's connect + call + JSON-format happy
    /// path, its honest not-found error handling, and that `mail mark`
    /// rejects an ambiguous `--read`/`--unread` combination before ever
    /// dialing the daemon.
    #[tokio::test]
    async fn mail_rpcs_round_trip_over_grpc_to_a_stub_daemon() {
        use nuncio_proto::v1::mail_server::{Mail as MailService, MailServer};
        use nuncio_proto::v1::{
            Folder as FolderProto, GetMessageRequest, GetMessageResponse, ListFoldersRequest,
            ListFoldersResponse, ListMessagesRequest, ListMessagesResponse, MarkReadRequest,
            MarkReadResponse, Message as MessageProto, MessageSearchHit, SearchMessagesRequest,
            SearchMessagesResponse, SendMessageRequest, SendMessageResponse, SyncRequest,
            SyncResponse,
        };
        use std::sync::Mutex;

        fn stub_message() -> MessageProto {
            MessageProto {
                id: "msg-stub-1".to_string(),
                account_id: "acct-stub-1".to_string(),
                folder_id: "inbox".to_string(),
                subject: "Stub Subject".to_string(),
                sender: "alice@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: Some(nuncio_proto::time::timestamp_from_unix_secs(1_700_000_000)),
                read: false,
                body_plain: Some("Stub body text".to_string()),
                body_html: None,
                attachments: Vec::new(),
            }
        }

        /// Minimal test-only stub of `nuncio.v1.Mail`: records the last
        /// `MarkReadRequest`/`SyncRequest` it received (so this test can
        /// assert on exactly what the CLI sent over the wire) and otherwise
        /// returns fixed responses.
        #[derive(Default)]
        struct StubMail {
            last_mark_read: Arc<Mutex<Option<MarkReadRequest>>>,
            last_send_message: Arc<Mutex<Option<SendMessageRequest>>>,
            last_sync: Arc<Mutex<Option<SyncRequest>>>,
        }

        #[tonic::async_trait]
        impl MailService for StubMail {
            async fn list_folders(
                &self,
                _request: tonic::Request<ListFoldersRequest>,
            ) -> Result<tonic::Response<ListFoldersResponse>, tonic::Status> {
                Ok(tonic::Response::new(ListFoldersResponse {
                    folders: vec![FolderProto {
                        id: "inbox".to_string(),
                        name: "inbox".to_string(),
                        total_messages: 3,
                        unread_messages: 1,
                    }],
                    next_page_token: String::new(),
                }))
            }

            async fn list_messages(
                &self,
                request: tonic::Request<ListMessagesRequest>,
            ) -> Result<tonic::Response<ListMessagesResponse>, tonic::Status> {
                let req = request.into_inner();
                let mut message = stub_message();
                message.folder_id = req.folder_id;
                Ok(tonic::Response::new(ListMessagesResponse {
                    messages: vec![message],
                    next_page_token: String::new(),
                }))
            }

            async fn get_message(
                &self,
                request: tonic::Request<GetMessageRequest>,
            ) -> Result<tonic::Response<GetMessageResponse>, tonic::Status> {
                let req = request.into_inner();
                if req.message_id == "msg-stub-1" {
                    Ok(tonic::Response::new(GetMessageResponse {
                        message: Some(stub_message()),
                    }))
                } else {
                    Err(tonic::Status::not_found(format!(
                        "message '{}' not found",
                        req.message_id
                    )))
                }
            }

            async fn mark_read(
                &self,
                request: tonic::Request<MarkReadRequest>,
            ) -> Result<tonic::Response<MarkReadResponse>, tonic::Status> {
                let req = request.into_inner();
                *self
                    .last_mark_read
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(MarkReadResponse {}))
            }

            async fn search_messages(
                &self,
                request: tonic::Request<SearchMessagesRequest>,
            ) -> Result<tonic::Response<SearchMessagesResponse>, tonic::Status> {
                let req = request.into_inner();
                Ok(tonic::Response::new(SearchMessagesResponse {
                    hits: vec![MessageSearchHit {
                        message_id: "msg-stub-1".to_string(),
                        title: "Stub Subject".to_string(),
                        snippet: format!("...{}...", req.query),
                    }],
                    next_page_token: String::new(),
                }))
            }

            async fn send_message(
                &self,
                request: tonic::Request<SendMessageRequest>,
            ) -> Result<tonic::Response<SendMessageResponse>, tonic::Status> {
                let req = request.into_inner();
                *self
                    .last_send_message
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(SendMessageResponse {
                    message_id: "sent-stub-1".to_string(),
                }))
            }

            async fn sync(
                &self,
                request: tonic::Request<SyncRequest>,
            ) -> Result<tonic::Response<SyncResponse>, tonic::Status> {
                let req = request.into_inner();
                *self.last_sync.lock().unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(SyncResponse { synced_count: 2 }))
            }
        }

        let stub = StubMail::default();
        let probe = stub.last_mark_read.clone();
        let send_probe = stub.last_send_message.clone();
        let sync_probe = stub.last_sync.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(MailServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        // `mail sync`: proves the CLI is a real gRPC client of `Mail/Sync`
        // -- it sends a `SyncRequest` with no `account_id` (sync every
        // account) and reports the daemon's real
        // `synced_count` in its output, rather than fabricating a status by
        // only flipping this runner's own throwaway local `EventBus`.
        let mail_sync = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Sync,
                },
                true,
            )
            .await;
        assert!(mail_sync.contains(r#""status":"sync_started""#));
        assert!(mail_sync.contains(r#""synced_count":2"#));
        let recorded_sync = sync_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a sync request");
        assert_eq!(recorded_sync.account_id, None);

        // `folder list`
        let folder_list = runner
            .execute_command(
                &Commands::Folder {
                    action: FolderSubcommand::List,
                },
                true,
            )
            .await;
        assert!(folder_list.contains(r#""id":"inbox""#));
        assert!(folder_list.contains(r#""total_messages":3"#));

        // `mail list`
        let mail_list = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::List {
                        folder: "inbox".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(mail_list.contains("msg-stub-1"));
        assert!(mail_list.contains(r#""folder_id":"inbox""#));

        // `mail read` happy path
        let mail_read = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Read {
                        id: "msg-stub-1".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(mail_read.contains("Stub Subject"));
        assert!(mail_read.contains("Stub body text"));

        // `mail read` honest not-found error
        let mail_read_missing = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Read {
                        id: "missing".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(mail_read_missing.contains(r#""status":"error""#));
        assert!(mail_read_missing.contains("not found"));

        // `mail search`
        let mail_search = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Search {
                        query: "roadmap".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(mail_search.contains(r#""query":"roadmap""#));
        assert!(mail_search.contains("msg-stub-1"));

        // `mail mark --read`
        let mail_mark = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Mark {
                        id: "msg-stub-1".to_string(),
                        read: true,
                        unread: false,
                    },
                },
                true,
            )
            .await;
        assert!(mail_mark.contains(r#""status":"marked""#));
        assert!(mail_mark.contains(r#""read":true"#));

        let recorded = probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a mark_read request");
        assert_eq!(recorded.message_id, "msg-stub-1");
        assert!(recorded.read);

        // `mail mark` with neither --read nor --unread fails BEFORE dialing
        // the daemon.
        let mark_no_flag = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Mark {
                        id: "msg-stub-1".to_string(),
                        read: false,
                        unread: false,
                    },
                },
                true,
            )
            .await;
        assert!(mark_no_flag.contains("exactly one of --read or --unread"));

        // `mail send`: proves the CLI is a real gRPC client of
        // `Mail/SendMessage` -- the exact recipient/subject/body the
        // caller supplied reaches the daemon
        // over the wire, and the daemon's returned message id (NOT a
        // fabricated "Message sent") appears in the CLI's output.
        let mail_send = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Send {
                        to: "alice@nuncio.mx".to_string(),
                        subject: "Quarterly Roadmap".to_string(),
                        body: "Let's discuss the roadmap.".to_string(),
                        account: None,
                    },
                },
                true,
            )
            .await;
        assert!(mail_send.contains(r#""sent":true"#));
        assert!(mail_send.contains("sent-stub-1"));
        assert!(mail_send.contains("alice@nuncio.mx"));

        let recorded_send = send_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a send_message request");
        assert_eq!(recorded_send.to, "alice@nuncio.mx");
        assert_eq!(recorded_send.subject, "Quarterly Roadmap");
        assert_eq!(recorded_send.body_text, "Let's discuss the roadmap.");
        assert_eq!(recorded_send.account_id, None);

        // `mail send --account <id>` threads the explicit selector through
        // to the daemon over the wire, proving the CLI flag is not merely
        // parsed but actually reaches `SendMessageRequest.account_id`.
        let mail_send_with_account = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Send {
                        to: "alice@nuncio.mx".to_string(),
                        subject: "Quarterly Roadmap".to_string(),
                        body: "Let's discuss the roadmap.".to_string(),
                        account: Some("acct-work".to_string()),
                    },
                },
                true,
            )
            .await;
        assert!(mail_send_with_account.contains(r#""sent":true"#));

        let recorded_send_with_account = send_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a second send_message request");
        assert_eq!(
            recorded_send_with_account.account_id,
            Some("acct-work".to_string())
        );
    }

    /// Proves that a generic (non-`NotFound`/`InvalidArgument`) RPC failure
    /// is rendered as the server-provided `status.message()` text, never
    /// the raw `Display`/`Debug` dump of `tonic::Status` (which leaks
    /// internal gRPC framing like `metadata: MetadataMap { ... }` into
    /// CLI output).
    #[tokio::test]
    async fn mail_rpc_error_renders_clean_message_not_raw_status_dump() {
        use nuncio_proto::v1::mail_server::{Mail as MailService, MailServer};
        use nuncio_proto::v1::{
            GetMessageRequest, GetMessageResponse, ListFoldersRequest, ListFoldersResponse,
            ListMessagesRequest, ListMessagesResponse, MarkReadRequest, MarkReadResponse,
            SearchMessagesRequest, SearchMessagesResponse, SendMessageRequest, SendMessageResponse,
            SyncRequest, SyncResponse,
        };

        /// Minimal test-only stub of `nuncio.v1.Mail` whose `ListFolders`
        /// always fails with an internal error, so this test can assert
        /// on exactly how the CLI renders a generic RPC failure.
        struct StubMailAlwaysErrors;

        #[tonic::async_trait]
        impl MailService for StubMailAlwaysErrors {
            async fn list_folders(
                &self,
                _request: tonic::Request<ListFoldersRequest>,
            ) -> Result<tonic::Response<ListFoldersResponse>, tonic::Status> {
                Err(tonic::Status::internal("mailbox index is corrupt"))
            }

            async fn list_messages(
                &self,
                _request: tonic::Request<ListMessagesRequest>,
            ) -> Result<tonic::Response<ListMessagesResponse>, tonic::Status> {
                Err(tonic::Status::internal("mailbox index is corrupt"))
            }

            async fn get_message(
                &self,
                _request: tonic::Request<GetMessageRequest>,
            ) -> Result<tonic::Response<GetMessageResponse>, tonic::Status> {
                Err(tonic::Status::internal("mailbox index is corrupt"))
            }

            async fn mark_read(
                &self,
                _request: tonic::Request<MarkReadRequest>,
            ) -> Result<tonic::Response<MarkReadResponse>, tonic::Status> {
                Err(tonic::Status::internal("mailbox index is corrupt"))
            }

            async fn search_messages(
                &self,
                _request: tonic::Request<SearchMessagesRequest>,
            ) -> Result<tonic::Response<SearchMessagesResponse>, tonic::Status> {
                Err(tonic::Status::internal("mailbox index is corrupt"))
            }

            async fn send_message(
                &self,
                _request: tonic::Request<SendMessageRequest>,
            ) -> Result<tonic::Response<SendMessageResponse>, tonic::Status> {
                Err(tonic::Status::internal("mailbox index is corrupt"))
            }

            async fn sync(
                &self,
                _request: tonic::Request<SyncRequest>,
            ) -> Result<tonic::Response<SyncResponse>, tonic::Status> {
                Err(tonic::Status::internal("mailbox index is corrupt"))
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(MailServer::new(StubMailAlwaysErrors))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        let out = runner
            .execute_command(
                &Commands::Folder {
                    action: FolderSubcommand::List,
                },
                false,
            )
            .await;

        assert!(out.contains("mailbox index is corrupt"));
        assert!(!out.contains("metadata:"));
        assert!(!out.contains("MetadataMap"));
        assert!(!out.contains("status:"));
    }

    /// Proves the CLI decodes a WS-A1 `ErrorInfo` from a daemon RPC's status
    /// details and renders the human message, the gRPC code, and the typed
    /// reason -- in both text and `--json` mode -- instead of dropping the
    /// code or the reason the daemon went to the trouble of attaching.
    #[tokio::test]
    async fn typed_error_info_renders_code_reason_and_metadata_in_text_and_json() {
        use nuncio_proto::v1::mail_server::{Mail as MailService, MailServer};
        use nuncio_proto::v1::ErrorReason;
        use nuncio_proto::v1::{
            GetMessageRequest, GetMessageResponse, ListFoldersRequest, ListFoldersResponse,
            ListMessagesRequest, ListMessagesResponse, MarkReadRequest, MarkReadResponse,
            SearchMessagesRequest, SearchMessagesResponse, SendMessageRequest, SendMessageResponse,
            SyncRequest, SyncResponse,
        };

        /// Minimal test-only stub of `nuncio.v1.Mail` whose `ListFolders`
        /// always fails with a typed `ErrorInfo` (account-not-found), so this
        /// test can assert on exactly how the CLI decodes and renders it.
        struct StubMailTypedError;

        #[tonic::async_trait]
        impl MailService for StubMailTypedError {
            async fn list_folders(
                &self,
                _request: tonic::Request<ListFoldersRequest>,
            ) -> Result<tonic::Response<ListFoldersResponse>, tonic::Status> {
                Err(nuncio_proto::errors::status_with_metadata(
                    ErrorReason::AccountNotFound,
                    "account 'acct-missing' not found",
                    [("account_id".to_string(), "acct-missing".to_string())],
                ))
            }

            async fn list_messages(
                &self,
                _request: tonic::Request<ListMessagesRequest>,
            ) -> Result<tonic::Response<ListMessagesResponse>, tonic::Status> {
                Err(tonic::Status::internal("unused in this test"))
            }

            async fn get_message(
                &self,
                _request: tonic::Request<GetMessageRequest>,
            ) -> Result<tonic::Response<GetMessageResponse>, tonic::Status> {
                Err(tonic::Status::internal("unused in this test"))
            }

            async fn mark_read(
                &self,
                _request: tonic::Request<MarkReadRequest>,
            ) -> Result<tonic::Response<MarkReadResponse>, tonic::Status> {
                Err(tonic::Status::internal("unused in this test"))
            }

            async fn search_messages(
                &self,
                _request: tonic::Request<SearchMessagesRequest>,
            ) -> Result<tonic::Response<SearchMessagesResponse>, tonic::Status> {
                Err(tonic::Status::internal("unused in this test"))
            }

            async fn send_message(
                &self,
                _request: tonic::Request<SendMessageRequest>,
            ) -> Result<tonic::Response<SendMessageResponse>, tonic::Status> {
                Err(tonic::Status::internal("unused in this test"))
            }

            async fn sync(
                &self,
                _request: tonic::Request<SyncRequest>,
            ) -> Result<tonic::Response<SyncResponse>, tonic::Status> {
                Err(tonic::Status::internal("unused in this test"))
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(MailServer::new(StubMailTypedError))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        // Text mode: human message, gRPC code, and the typed reason -- not a
        // raw status dump, and not silently dropping the code/reason.
        let text_out = runner
            .execute_command(
                &Commands::Folder {
                    action: FolderSubcommand::List,
                },
                false,
            )
            .await;
        assert!(text_out.contains("account 'acct-missing' not found"));
        assert!(text_out.contains("code: NotFound"));
        assert!(text_out.contains("reason: ACCOUNT_NOT_FOUND"));

        // JSON mode: a coherent {error, code, reason, metadata} shape.
        let json_out = runner
            .execute_command(
                &Commands::Folder {
                    action: FolderSubcommand::List,
                },
                true,
            )
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&json_out).expect("valid JSON output");
        assert_eq!(parsed["status"], "error");
        assert!(parsed["error"]
            .as_str()
            .expect("error is a string")
            .contains("account 'acct-missing' not found"));
        assert_eq!(parsed["code"], "NotFound");
        assert_eq!(parsed["reason"], "ACCOUNT_NOT_FOUND");
        assert_eq!(parsed["metadata"]["account_id"], "acct-missing");
    }

    /// Reference-client proof: boots a stub `nuncio.v1.Calendar` gRPC server
    /// (mirroring the `Mail` stub pattern above) and drives the real
    /// `HeadlessRunner`'s `cal list`/`cal sync` gRPC client paths against it.
    ///
    /// Real persistence and the honest-error-when-no-backend-injected
    /// behavior are proven by `nunciod`'s own `grpc::tests`; this test exists
    /// purely to prove the CLI's connect + call + JSON-format happy path,
    /// and that the exact account/calendar/window it was given reaches the
    /// daemon over the wire.
    #[tokio::test]
    async fn calendar_rpcs_round_trip_over_grpc_to_a_stub_daemon() {
        use nuncio_proto::v1::calendar_server::{Calendar as CalendarService, CalendarServer};
        use nuncio_proto::v1::{
            CalendarEvent as CalendarEventProto, CalendarSyncRequest, CalendarSyncResponse,
            GetEventRequest, GetEventResponse, ListEventsRequest, ListEventsResponse,
        };
        use std::sync::Mutex;

        fn stub_event() -> CalendarEventProto {
            CalendarEventProto {
                id: "evt-stub-1".to_string(),
                account_id: "acct-stub-1".to_string(),
                calendar_id: "cal-stub".to_string(),
                summary: "Stub Meeting".to_string(),
                start_time: Some(nuncio_proto::time::timestamp_from_unix_secs(1_700_000_000)),
                end_time: Some(nuncio_proto::time::timestamp_from_unix_secs(1_700_003_600)),
                rrule: None,
                location: Some("Room 1".to_string()),
            }
        }

        /// Minimal test-only stub of `nuncio.v1.Calendar`: records the last
        /// `ListEventsRequest`/`CalendarSyncRequest` it received (so this
        /// test can assert on exactly what the CLI sent over the wire) and
        /// otherwise returns fixed responses.
        #[derive(Default)]
        struct StubCalendar {
            last_list_events: Arc<Mutex<Option<ListEventsRequest>>>,
            last_sync: Arc<Mutex<Option<CalendarSyncRequest>>>,
        }

        #[tonic::async_trait]
        impl CalendarService for StubCalendar {
            async fn sync(
                &self,
                request: tonic::Request<CalendarSyncRequest>,
            ) -> Result<tonic::Response<CalendarSyncResponse>, tonic::Status> {
                let req = request.into_inner();
                *self.last_sync.lock().unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(CalendarSyncResponse {
                    synced_count: 2,
                }))
            }

            async fn list_events(
                &self,
                request: tonic::Request<ListEventsRequest>,
            ) -> Result<tonic::Response<ListEventsResponse>, tonic::Status> {
                let req = request.into_inner();
                *self
                    .last_list_events
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(ListEventsResponse {
                    events: vec![stub_event()],
                    next_page_token: String::new(),
                }))
            }

            async fn get_event(
                &self,
                request: tonic::Request<GetEventRequest>,
            ) -> Result<tonic::Response<GetEventResponse>, tonic::Status> {
                let req = request.into_inner();
                if req.event_id == "evt-stub-1" {
                    Ok(tonic::Response::new(GetEventResponse {
                        event: Some(stub_event()),
                    }))
                } else {
                    Err(tonic::Status::not_found(format!(
                        "event '{}' not found",
                        req.event_id
                    )))
                }
            }
        }

        let stub = StubCalendar::default();
        let list_probe = stub.last_list_events.clone();
        let sync_probe = stub.last_sync.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(CalendarServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        // `cal list`: proves the CLI is a real gRPC client of
        // `Calendar/ListEvents` -- the exact account/calendar/window
        // supplied reaches the daemon over the wire, and the daemon's real
        // returned events (NOT a fabricated "0 events found") appear in the
        // CLI's output.
        let cal_list = runner
            .execute_command(
                &Commands::Cal {
                    action: CalSubcommand::List {
                        account: "acct-stub-1".to_string(),
                        calendar: "cal-stub".to_string(),
                        start: 0,
                        end: i64::MAX,
                    },
                },
                true,
            )
            .await;
        assert!(cal_list.contains("evt-stub-1"));
        assert!(cal_list.contains("Stub Meeting"));
        let recorded_list = list_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a list_events request");
        assert_eq!(recorded_list.account_id, "acct-stub-1");
        assert_eq!(recorded_list.calendar_id, "cal-stub");
        assert_eq!(
            recorded_list.start_window,
            Some(nuncio_proto::time::timestamp_from_unix_secs(0))
        );
        assert_eq!(
            recorded_list.end_window,
            Some(nuncio_proto::time::timestamp_from_unix_secs(i64::MAX))
        );

        // `cal sync`: proves the CLI is a real gRPC client of
        // `Calendar/Sync` and reports the daemon's real `synced_count`,
        // rather than fabricating a "calendar_sync_started" status by only
        // flipping this runner's own throwaway local `EventBus`.
        let cal_sync = runner
            .execute_command(
                &Commands::Cal {
                    action: CalSubcommand::Sync {
                        account: "acct-stub-1".to_string(),
                        calendar: "cal-stub".to_string(),
                        start: 0,
                        end: i64::MAX,
                    },
                },
                true,
            )
            .await;
        assert!(cal_sync.contains(r#""status":"calendar_sync_complete""#));
        assert!(cal_sync.contains(r#""synced_count":2"#));
        let recorded_sync = sync_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a sync request");
        assert_eq!(recorded_sync.account_id, "acct-stub-1");
        assert_eq!(recorded_sync.calendar_id, "cal-stub");
    }

    /// Reference-client proof: boots a stub `nuncio.v1.Contacts` gRPC server
    /// (mirroring the `Calendar` stub pattern above) and drives the real
    /// `HeadlessRunner`'s `contact list`/`search`/`add`/`sync` gRPC client
    /// paths against it.
    ///
    /// Real persistence and honest error semantics are proven by
    /// `nunciod`'s own `grpc::tests`; this test exists purely to prove the
    /// CLI's connect + call + JSON-format happy path, including `contact
    /// search`'s client-side filtering over the stub's full
    /// `ListContacts` result.
    #[tokio::test]
    async fn contacts_rpcs_round_trip_over_grpc_to_a_stub_daemon() {
        use nuncio_proto::v1::contacts_server::{Contacts as ContactsService, ContactsServer};
        use nuncio_proto::v1::{
            Contact as ContactProto, ContactEmail as ContactEmailProto, ContactsSyncRequest,
            ContactsSyncResponse, CreateContactRequest, CreateContactResponse, GetContactRequest,
            GetContactResponse, ListContactsRequest, ListContactsResponse,
        };
        use std::sync::Mutex;

        fn stub_contact(id: &str, display_name: &str, org: &str, email: &str) -> ContactProto {
            ContactProto {
                id: id.to_string(),
                account_id: Some("acct-stub-1".to_string()),
                display_name: display_name.to_string(),
                given_name: None,
                family_name: None,
                organization: Some(org.to_string()),
                job_title: None,
                notes: None,
                avatar_url: None,
                emails: vec![ContactEmailProto {
                    email: email.to_string(),
                    label: "work".to_string(),
                    is_primary: true,
                }],
                phones: vec![],
                is_favorite: false,
                interaction_count: 0,
            }
        }

        /// Minimal test-only stub of `nuncio.v1.Contacts`: records the last
        /// `ListContactsRequest`/`ContactsSyncRequest`/`CreateContactRequest`
        /// it received (so this test can assert on exactly what the CLI
        /// sent over the wire) and otherwise returns fixed responses.
        #[derive(Default)]
        struct StubContacts {
            last_list: Arc<Mutex<Option<ListContactsRequest>>>,
            last_sync: Arc<Mutex<Option<ContactsSyncRequest>>>,
            last_create: Arc<Mutex<Option<CreateContactRequest>>>,
        }

        #[tonic::async_trait]
        impl ContactsService for StubContacts {
            async fn sync(
                &self,
                request: tonic::Request<ContactsSyncRequest>,
            ) -> Result<tonic::Response<ContactsSyncResponse>, tonic::Status> {
                let req = request.into_inner();
                *self.last_sync.lock().unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(ContactsSyncResponse {
                    synced_count: 2,
                }))
            }

            async fn list_contacts(
                &self,
                request: tonic::Request<ListContactsRequest>,
            ) -> Result<tonic::Response<ListContactsResponse>, tonic::Status> {
                let req = request.into_inner();
                *self.last_list.lock().unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(ListContactsResponse {
                    contacts: vec![
                        stub_contact("ct-stub-1", "Alice Stub", "Kof22", "alice@nuncio.mx"),
                        stub_contact("ct-stub-2", "Bob Other", "OtherCo", "bob@nuncio.mx"),
                    ],
                    next_page_token: String::new(),
                }))
            }

            async fn get_contact(
                &self,
                request: tonic::Request<GetContactRequest>,
            ) -> Result<tonic::Response<GetContactResponse>, tonic::Status> {
                let req = request.into_inner();
                if req.contact_id == "ct-stub-1" {
                    Ok(tonic::Response::new(GetContactResponse {
                        contact: Some(stub_contact(
                            "ct-stub-1",
                            "Alice Stub",
                            "Kof22",
                            "alice@nuncio.mx",
                        )),
                    }))
                } else {
                    Err(tonic::Status::not_found(format!(
                        "contact '{}' not found",
                        req.contact_id
                    )))
                }
            }

            async fn create_contact(
                &self,
                request: tonic::Request<CreateContactRequest>,
            ) -> Result<tonic::Response<CreateContactResponse>, tonic::Status> {
                let req = request.into_inner();
                *self.last_create.lock().unwrap_or_else(|e| e.into_inner()) = Some(req.clone());
                Ok(tonic::Response::new(CreateContactResponse {
                    contact: Some(stub_contact(
                        "ct-stub-new",
                        &req.display_name,
                        req.organization.as_deref().unwrap_or(""),
                        req.emails.first().map(|e| e.email.as_str()).unwrap_or(""),
                    )),
                }))
            }
        }

        let stub = StubContacts::default();
        let list_probe = stub.last_list.clone();
        let sync_probe = stub.last_sync.clone();
        let create_probe = stub.last_create.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(ContactsServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        // `contact list`: proves the CLI is a real gRPC client of
        // `Contacts/ListContacts` -- the exact account supplied reaches the
        // daemon over the wire, and the daemon's real returned contacts
        // (NOT a fabricated "0 contacts found") appear in the CLI's output.
        let contact_list = runner
            .execute_command(
                &Commands::Contact {
                    action: ContactSubcommand::List {
                        account: "acct-stub-1".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(contact_list.contains("ct-stub-1"));
        assert!(contact_list.contains("Alice Stub"));
        let recorded_list = list_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a list_contacts request");
        assert_eq!(recorded_list.account_id, "acct-stub-1");

        // `contact search`: proves the CLI filters the daemon's real
        // `ListContacts` result client-side by query, matching only the
        // contact whose organization contains "Kof22".
        let contact_search = runner
            .execute_command(
                &Commands::Contact {
                    action: ContactSubcommand::Search {
                        account: "acct-stub-1".to_string(),
                        query: "kof22".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(contact_search.contains("ct-stub-1"));
        assert!(!contact_search.contains("ct-stub-2"));

        // `contact add`: proves the CLI is a real gRPC client of
        // `Contacts/CreateContact` and reports the daemon's real persisted
        // contact, rather than fabricating a throwaway in-memory contact
        // the way this command used to.
        let contact_add = runner
            .execute_command(
                &Commands::Contact {
                    action: ContactSubcommand::Add {
                        account: "acct-stub-1".to_string(),
                        name: "New Person".to_string(),
                        email: "new.person@nuncio.mx".to_string(),
                        org: Some("Acme".to_string()),
                    },
                },
                true,
            )
            .await;
        assert!(contact_add.contains(r#""status":"contact_created""#));
        assert!(contact_add.contains("ct-stub-new"));
        let recorded_create = create_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a create_contact request");
        assert_eq!(recorded_create.account_id, "acct-stub-1");
        assert_eq!(recorded_create.display_name, "New Person");
        assert_eq!(recorded_create.organization.as_deref(), Some("Acme"));

        // `contact sync`: proves the CLI is a real gRPC client of
        // `Contacts/Sync` and reports the daemon's real `synced_count`.
        let contact_sync = runner
            .execute_command(
                &Commands::Contact {
                    action: ContactSubcommand::Sync {
                        account: "acct-stub-1".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(contact_sync.contains(r#""status":"contacts_sync_complete""#));
        assert!(contact_sync.contains(r#""synced_count":2"#));
        let recorded_sync = sync_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a sync request");
        assert_eq!(recorded_sync.account_id, "acct-stub-1");
    }

    /// Reference-client proof: boots a stub `nuncio.v1.Filters` gRPC server
    /// (mirroring the `Accounts`/`Mail` stub pattern above) and drives the
    /// real `HeadlessRunner`'s
    /// `filter list`/`filter create`/`filter delete`/`filter validate`/
    /// `filter test` gRPC client paths against it.
    ///
    /// Real persistence, `ArcSwap` engine reload, and honest
    /// validation/preview semantics are proven by `nunciod`'s own
    /// `grpc::tests`; this test exists purely to prove the CLI's connect +
    /// call + JSON-format happy path, and its honest invalid-argument error
    /// handling.
    #[tokio::test]
    async fn filter_rpcs_round_trip_over_grpc_to_a_stub_daemon() {
        use nuncio_proto::v1::filters_server::{Filters as FiltersService, FiltersServer};
        use nuncio_proto::v1::{
            CreateRuleRequest, CreateRuleResponse, DeleteRuleRequest, DeleteRuleResponse,
            ExportRulesRequest, ExportRulesResponse, FilterExecutionLog as FilterExecutionLogProto,
            FilterRule as FilterRuleProto, GetExecutionLogsRequest, GetExecutionLogsResponse,
            ImportRulesRequest, ImportRulesResponse, ListRulesRequest, ListRulesResponse,
            PreviewRuleRequest, PreviewRuleResponse, TriageProgress, TriageRequest,
            UpdateRuleRequest, UpdateRuleResponse, ValidateRuleRequest, ValidateRuleResponse,
        };
        use std::sync::Mutex;

        /// Minimal test-only stub of `nuncio.v1.Filters`: records the last
        /// `DeleteRuleRequest`/`UpdateRuleRequest`/`ImportRulesRequest` it
        /// received (so this test can assert on exactly what the CLI sent
        /// over the wire) and otherwise returns fixed responses.
        #[derive(Default)]
        struct StubFilters {
            last_delete: Arc<Mutex<Option<DeleteRuleRequest>>>,
            last_update: Arc<Mutex<Option<UpdateRuleRequest>>>,
            last_import: Arc<Mutex<Option<ImportRulesRequest>>>,
        }

        #[tonic::async_trait]
        impl FiltersService for StubFilters {
            async fn create_rule(
                &self,
                request: tonic::Request<CreateRuleRequest>,
            ) -> Result<tonic::Response<CreateRuleResponse>, tonic::Status> {
                let req = request.into_inner();
                if req.nsql.contains("BROKEN") {
                    return Err(tonic::Status::invalid_argument(
                        "NSQL syntax error: stub rejects BROKEN rules",
                    ));
                }
                Ok(tonic::Response::new(CreateRuleResponse {
                    rule: Some(FilterRuleProto {
                        id: "rule-stub-1".to_string(),
                        name: req.name,
                        target_account: "*".to_string(),
                        priority: req.priority,
                        enabled: true,
                        nsql_text: req.nsql,
                        actions: vec!["MARK READ".to_string()],
                        created_at: Some(nuncio_proto::time::timestamp_from_unix_secs(
                            1_700_000_000,
                        )),
                        updated_at: Some(nuncio_proto::time::timestamp_from_unix_secs(
                            1_700_000_000,
                        )),
                    }),
                }))
            }

            async fn list_rules(
                &self,
                _request: tonic::Request<ListRulesRequest>,
            ) -> Result<tonic::Response<ListRulesResponse>, tonic::Status> {
                Ok(tonic::Response::new(ListRulesResponse {
                    rules: vec![FilterRuleProto {
                        id: "rule-stub-1".to_string(),
                        name: "Stub Rule".to_string(),
                        target_account: "*".to_string(),
                        priority: 5,
                        enabled: true,
                        nsql_text: "WHERE subject CONTAINS 'Urgent' ACTION MARK READ".to_string(),
                        actions: vec!["MARK READ".to_string()],
                        created_at: Some(nuncio_proto::time::timestamp_from_unix_secs(
                            1_700_000_000,
                        )),
                        updated_at: Some(nuncio_proto::time::timestamp_from_unix_secs(
                            1_700_000_000,
                        )),
                    }],
                    next_page_token: String::new(),
                }))
            }

            async fn delete_rule(
                &self,
                request: tonic::Request<DeleteRuleRequest>,
            ) -> Result<tonic::Response<DeleteRuleResponse>, tonic::Status> {
                let req = request.into_inner();
                *self.last_delete.lock().unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(DeleteRuleResponse {}))
            }

            async fn validate_rule(
                &self,
                request: tonic::Request<ValidateRuleRequest>,
            ) -> Result<tonic::Response<ValidateRuleResponse>, tonic::Status> {
                let req = request.into_inner();
                if req.nsql.contains("BROKEN") {
                    Ok(tonic::Response::new(ValidateRuleResponse {
                        valid: false,
                        error: "NSQL syntax error: stub rejects BROKEN rules".to_string(),
                    }))
                } else {
                    Ok(tonic::Response::new(ValidateRuleResponse {
                        valid: true,
                        error: String::new(),
                    }))
                }
            }

            async fn preview_rule(
                &self,
                _request: tonic::Request<PreviewRuleRequest>,
            ) -> Result<tonic::Response<PreviewRuleResponse>, tonic::Status> {
                Ok(tonic::Response::new(PreviewRuleResponse {
                    message_id: "msg-stub-1".to_string(),
                    matched: true,
                    matched_rule_id: Some("rule-stub-1".to_string()),
                    matched_rule_name: Some("Stub Rule".to_string()),
                    actions_evaluated: vec!["MARK READ".to_string()],
                    execution_time: Some(nuncio_proto::time::duration_from_micros(42)),
                    condition_traces: vec!["Rule 'Stub Rule': MATCH".to_string()],
                }))
            }

            async fn update_rule(
                &self,
                request: tonic::Request<UpdateRuleRequest>,
            ) -> Result<tonic::Response<UpdateRuleResponse>, tonic::Status> {
                let req = request.into_inner();
                if req.rule_id != "rule-stub-1" {
                    return Err(tonic::Status::not_found("no such rule"));
                }
                *self.last_update.lock().unwrap_or_else(|e| e.into_inner()) = Some(req.clone());
                Ok(tonic::Response::new(UpdateRuleResponse {
                    rule: Some(FilterRuleProto {
                        id: req.rule_id,
                        name: req.name.unwrap_or_else(|| "Stub Rule".to_string()),
                        target_account: "*".to_string(),
                        priority: req.priority.unwrap_or(5),
                        enabled: true,
                        nsql_text: req
                            .nsql
                            .unwrap_or_else(|| "WHERE subject CONTAINS 'Urgent'".to_string()),
                        actions: vec!["MARK READ".to_string()],
                        created_at: Some(nuncio_proto::time::timestamp_from_unix_secs(
                            1_700_000_000,
                        )),
                        updated_at: Some(nuncio_proto::time::timestamp_from_unix_secs(
                            1_700_000_001,
                        )),
                    }),
                }))
            }

            async fn export_rules(
                &self,
                request: tonic::Request<ExportRulesRequest>,
            ) -> Result<tonic::Response<ExportRulesResponse>, tonic::Status> {
                let req = request.into_inner();
                let content = if req.format() == nuncio_proto::v1::RuleExportFormat::Json {
                    r#"[{"id":"rule-stub-1"}]"#.to_string()
                } else {
                    "SELECT * FROM emails WHERE subject CONTAINS 'Urgent' ACTION MARK READ"
                        .to_string()
                };
                Ok(tonic::Response::new(ExportRulesResponse { content }))
            }

            async fn import_rules(
                &self,
                request: tonic::Request<ImportRulesRequest>,
            ) -> Result<tonic::Response<ImportRulesResponse>, tonic::Status> {
                let req = request.into_inner();
                *self.last_import.lock().unwrap_or_else(|e| e.into_inner()) = Some(req.clone());
                Ok(tonic::Response::new(ImportRulesResponse {
                    imported_count: 1,
                    errors: vec!["failed to parse 'garbage': stub parse error".to_string()],
                }))
            }

            async fn get_execution_logs(
                &self,
                _request: tonic::Request<GetExecutionLogsRequest>,
            ) -> Result<tonic::Response<GetExecutionLogsResponse>, tonic::Status> {
                Ok(tonic::Response::new(GetExecutionLogsResponse {
                    logs: vec![FilterExecutionLogProto {
                        id: 1,
                        rule_id: "rule-stub-1".to_string(),
                        message_id: "msg-stub-1".to_string(),
                        action_taken: "MARK READ".to_string(),
                        matched_at: Some(nuncio_proto::time::timestamp_from_unix_secs(
                            1_700_000_002,
                        )),
                        prev_hash: "prev-hash".to_string(),
                        hash: "hash".to_string(),
                    }],
                }))
            }

            type TriageStream = std::pin::Pin<
                Box<
                    dyn tokio_stream::Stream<Item = Result<TriageProgress, tonic::Status>>
                        + Send
                        + 'static,
                >,
            >;

            async fn triage(
                &self,
                _request: tonic::Request<TriageRequest>,
            ) -> Result<tonic::Response<Self::TriageStream>, tonic::Status> {
                let updates = vec![
                    Ok(TriageProgress {
                        scanned_count: 2,
                        matched_count: 1,
                        actions_applied_count: 1,
                        last_message_id: "msg-stub-2".to_string(),
                        done: false,
                    }),
                    Ok(TriageProgress {
                        scanned_count: 3,
                        matched_count: 2,
                        actions_applied_count: 2,
                        last_message_id: "msg-stub-3".to_string(),
                        done: true,
                    }),
                ];
                Ok(tonic::Response::new(Box::pin(tokio_stream::iter(updates))))
            }
        }

        let stub = StubFilters::default();
        let delete_probe = stub.last_delete.clone();
        let update_probe = stub.last_update.clone();
        let import_probe = stub.last_import.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(FiltersServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        // `filter create` happy path.
        let create_out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Create {
                        name: "My Rule".to_string(),
                        sql: "WHERE subject CONTAINS 'Urgent' ACTION MARK READ".to_string(),
                        priority: 5,
                    },
                },
                true,
            )
            .await;
        assert!(create_out.contains("rule-stub-1"));
        assert!(create_out.contains("My Rule"));

        // `filter create` honest invalid-argument error.
        let create_err = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Create {
                        name: "Broken".to_string(),
                        sql: "BROKEN NSQL".to_string(),
                        priority: 0,
                    },
                },
                true,
            )
            .await;
        assert!(create_err.contains(r#""status":"error""#));
        assert!(create_err.contains("stub rejects BROKEN rules"));

        // `filter list`.
        let list_out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::List,
                },
                true,
            )
            .await;
        assert!(list_out.contains("rule-stub-1"));
        assert!(list_out.contains("Stub Rule"));

        // `filter delete`.
        let delete_out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Delete {
                        id: "rule-stub-1".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(delete_out.contains(r#""status":"deleted""#));
        let recorded_delete = delete_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a delete_rule request");
        assert_eq!(recorded_delete.rule_id, "rule-stub-1");

        // `filter validate` valid + invalid.
        let validate_ok = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Validate {
                        sql: "WHERE subject CONTAINS 'Urgent' ACTION MARK READ".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(validate_ok.contains(r#""valid":true"#));

        let validate_err = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Validate {
                        sql: "BROKEN NSQL".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(validate_err.contains(r#""valid":false"#));
        assert!(validate_err.contains("stub rejects BROKEN rules"));

        // `filter test` (PreviewRule) dry-run.
        let test_out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Test {
                        sql: "WHERE subject CONTAINS 'Urgent' ACTION MARK READ".to_string(),
                        message_id: Some("msg-stub-1".to_string()),
                    },
                },
                true,
            )
            .await;
        assert!(test_out.contains(r#""matched":true"#));
        assert!(test_out.contains("msg-stub-1"));

        // `filter edit`: proves the CLI forwards only the supplied
        // overrides and renders the daemon's updated rule, and that a
        // not-found `id` surfaces the daemon's honest `NotFound` error.
        let edit_out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Edit {
                        id: "rule-stub-1".to_string(),
                        name: Some("Renamed Rule".to_string()),
                        sql: None,
                        priority: Some(9),
                    },
                },
                true,
            )
            .await;
        assert!(edit_out.contains("Renamed Rule"));
        assert!(edit_out.contains(r#""priority":9"#));
        let recorded_update = update_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received an update_rule request");
        assert_eq!(recorded_update.rule_id, "rule-stub-1");
        assert_eq!(recorded_update.name.as_deref(), Some("Renamed Rule"));
        assert_eq!(recorded_update.nsql, None);
        assert_eq!(recorded_update.priority, Some(9));

        let edit_not_found = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Edit {
                        id: "no-such-rule".to_string(),
                        name: None,
                        sql: None,
                        priority: None,
                    },
                },
                true,
            )
            .await;
        assert!(edit_not_found.contains(r#""status":"error""#));

        // `filter export`: proves the CLI forwards `format` and renders the
        // daemon's rendered content as-is.
        let export_sql = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Export {
                        format: "sql".to_string(),
                    },
                },
                false,
            )
            .await;
        assert!(export_sql.contains("SELECT * FROM emails"));

        let export_json = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Export {
                        format: "json".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(export_json.contains("rule-stub-1"));

        // `filter import`: proves the CLI reads the local file and forwards
        // its raw content, and surfaces the daemon's per-line errors
        // instead of silently dropping them.
        let import_file = std::env::temp_dir().join(format!(
            "nuncio-cli-filter-import-test-{}.sql",
            std::process::id()
        ));
        std::fs::write(&import_file, "WHERE subject CONTAINS 'Urgent'\ngarbage\n")
            .expect("write temp import file");
        let import_out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Import {
                        file: import_file.to_string_lossy().to_string(),
                    },
                },
                true,
            )
            .await;
        let _ = std::fs::remove_file(&import_file);
        assert!(import_out.contains(r#""imported_count":1"#));
        assert!(import_out.contains("stub parse error"));
        let recorded_import = import_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received an import_rules request");
        assert!(recorded_import
            .content
            .contains("WHERE subject CONTAINS 'Urgent'"));
        assert!(recorded_import.content.contains("garbage"));

        // `filter logs`: proves the CLI reads the daemon's real execution
        // log ledger, not any local/ephemeral state.
        let logs_out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Logs { limit: 10 },
                },
                true,
            )
            .await;
        assert!(logs_out.contains("msg-stub-1"));
        assert!(logs_out.contains(r#""action_taken":"MARK READ""#));

        // `filter triage`: proves the CLI drains the server-streaming
        // `Triage` RPC (its first real streaming-RPC consumer) and reports
        // the final cumulative progress rather than only the first chunk.
        let triage_out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::Triage {
                        rule_id: None,
                        chunk_size: Some(2),
                    },
                },
                true,
            )
            .await;
        assert!(triage_out.contains(r#""scanned_count":3"#));
        assert!(triage_out.contains(r#""matched_count":2"#));
        assert!(triage_out.contains(r#""actions_applied_count":2"#));
        assert!(triage_out.contains(r#""done":true"#));
    }

    #[tokio::test]
    async fn filter_commands_report_honest_error_when_daemon_unreachable() {
        let addr = reserve_unreachable_addr().await;
        let runner = HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr)
            .await
            .expect("ephemeral runner initializes");

        let out = runner
            .execute_command(
                &Commands::Filter {
                    action: FilterSubcommand::List,
                },
                true,
            )
            .await;
        assert!(out.contains(r#""status":"error""#));
        assert!(out.contains("unreachable"));
    }

    /// Reference-client proof: boots a stub `nuncio.v1.Export` gRPC server
    /// (mirroring
    /// `system_status_reports_live_daemon_status_over_grpc_when_reachable`'s
    /// `StubSystem` pattern above), recording the exact `ExportRequest` it
    /// received, and drives the real `HeadlessRunner`'s `mail export` gRPC
    /// client path against it. Real message/byte counts and file writing
    /// are proven end-to-end by `nunciod`'s own `grpc::tests`; this test
    /// exists purely to prove the CLI's connect + call + scope-mapping +
    /// JSON-format happy path.
    #[tokio::test]
    async fn mail_export_round_trips_over_grpc_to_a_stub_daemon() {
        use nuncio_proto::v1::export_server::{Export as ExportService, ExportServer};
        use nuncio_proto::v1::{ExportRequest, ExportResponse};
        use std::sync::Mutex;

        #[derive(Default)]
        struct StubExport {
            last_request: Arc<Mutex<Option<ExportRequest>>>,
        }

        #[tonic::async_trait]
        impl ExportService for StubExport {
            async fn export_mailbox(
                &self,
                request: tonic::Request<ExportRequest>,
            ) -> Result<tonic::Response<ExportResponse>, tonic::Status> {
                let req = request.into_inner();
                let output_path = req.output_path.clone();
                *self.last_request.lock().unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(ExportResponse {
                    output_path,
                    message_count: 7,
                    bytes_written: 4096,
                }))
            }
        }

        let last_request = Arc::new(Mutex::new(None));
        let stub = StubExport {
            last_request: last_request.clone(),
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(ExportServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        let out = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Export {
                        format: "jsonl".to_string(),
                        out: "/tmp/stub-export.jsonl".to_string(),
                        account: Some("acct-stub-1".to_string()),
                        folder: None,
                    },
                },
                true,
            )
            .await;
        assert!(out.contains(r#""message_count":7"#));
        assert!(out.contains(r#""bytes_written":4096"#));
        assert!(out.contains("stub-export.jsonl"));

        let recorded = last_request
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received an export_mailbox request");
        assert_eq!(recorded.output_path, "/tmp/stub-export.jsonl");
        assert_eq!(recorded.format(), nuncio_proto::v1::ExportFormat::Jsonl);
        assert_eq!(
            recorded.scope,
            Some(nuncio_proto::v1::export_request::Scope::AccountId(
                "acct-stub-1".to_string()
            ))
        );

        let text_out = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Export {
                        format: "mbox".to_string(),
                        out: "/tmp/stub-export-2.mbox".to_string(),
                        account: None,
                        folder: Some("inbox".to_string()),
                    },
                },
                false,
            )
            .await;
        assert!(text_out.contains("Exported 7 message(s)"));
        let recorded2 = last_request
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("stub daemon received a second export_mailbox request");
        assert_eq!(
            recorded2.scope,
            Some(nuncio_proto::v1::export_request::Scope::FolderId(
                "inbox".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn mail_export_rejects_an_unknown_format_before_dialing_the_daemon() {
        // An unreachable address deliberately proves the format is
        // validated BEFORE the daemon is ever dialed -- if this connected
        // first, the error would be a transport failure instead of the
        // expected format-parsing error.
        let addr = reserve_unreachable_addr().await;
        let runner = HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr)
            .await
            .expect("ephemeral runner initializes");

        let out = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Export {
                        format: "not-a-real-format".to_string(),
                        out: "/tmp/out.json".to_string(),
                        account: None,
                        folder: None,
                    },
                },
                true,
            )
            .await;
        assert!(out.contains(r#""status":"error""#));
        assert!(out.contains("Unknown export format"));
    }

    /// Reference-client proof: boots a stub `nuncio.v1.Audit` gRPC server
    /// and drives the real `HeadlessRunner`'s
    /// `system audit list` / `system audit verify` gRPC client paths
    /// against it. Real ledger seeding, chain verification, and the
    /// tampered-chain case are proven end-to-end by `nunciod`'s own
    /// `grpc::tests`; this test exists purely to prove the CLI's connect +
    /// call + JSON-format happy path.
    #[tokio::test]
    async fn system_audit_list_and_verify_round_trip_over_grpc_to_a_stub_daemon() {
        use nuncio_proto::v1::audit_server::{Audit as AuditService, AuditServer};
        use nuncio_proto::v1::{
            AuditRecord, ListRecordsRequest, ListRecordsResponse, VerifyChainRequest,
            VerifyChainResponse,
        };

        struct StubAudit;

        #[tonic::async_trait]
        impl AuditService for StubAudit {
            async fn list_records(
                &self,
                _request: tonic::Request<ListRecordsRequest>,
            ) -> Result<tonic::Response<ListRecordsResponse>, tonic::Status> {
                Ok(tonic::Response::new(ListRecordsResponse {
                    records: vec![AuditRecord {
                        sequence: 1,
                        timestamp: Some(nuncio_proto::time::timestamp_from_unix_nanos(
                            1_700_000_000_000_000_000,
                        )),
                        actor: "system.test".to_string(),
                        action: "data.export".to_string(),
                        data_hash: "deadbeef".to_string(),
                        previous_block_hash: "GENESIS".to_string(),
                        record_hmac: "cafebabe".to_string(),
                    }],
                    next_page_token: String::new(),
                }))
            }

            async fn verify_chain(
                &self,
                _request: tonic::Request<VerifyChainRequest>,
            ) -> Result<tonic::Response<VerifyChainResponse>, tonic::Status> {
                Ok(tonic::Response::new(VerifyChainResponse {
                    valid: false,
                    record_count: 3,
                    first_broken_seq: Some(2),
                }))
            }
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(AuditServer::new(StubAudit))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let runner =
            HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr.to_string())
                .await
                .expect("ephemeral runner initializes");

        let list_out = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Audit {
                        action: AuditSubcommand::List { page_size: 0 },
                    },
                },
                true,
            )
            .await;
        assert!(list_out.contains(r#""action":"data.export""#));
        assert!(list_out.contains(r#""sequence":1"#));

        let verify_out = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Audit {
                        action: AuditSubcommand::Verify,
                    },
                },
                true,
            )
            .await;
        assert!(verify_out.contains(r#""valid":false"#));
        assert!(verify_out.contains(r#""first_broken_seq":2"#));

        let verify_text = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Audit {
                        action: AuditSubcommand::Verify,
                    },
                },
                false,
            )
            .await;
        assert!(verify_text.contains("TAMPERED"));
        assert!(verify_text.contains("sequence 2"));
    }

    #[tokio::test]
    async fn mail_export_and_system_audit_report_honest_error_when_daemon_unreachable() {
        let addr = reserve_unreachable_addr().await;
        let runner = HeadlessRunner::ephemeral_with(Arc::new(SecretManager::mock()), addr)
            .await
            .expect("ephemeral runner initializes");

        let export_out = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Export {
                        format: "json".to_string(),
                        out: "/tmp/out.json".to_string(),
                        account: None,
                        folder: None,
                    },
                },
                true,
            )
            .await;
        assert!(export_out.contains(r#""status":"error""#));
        assert!(export_out.contains("unreachable"));

        let audit_list_out = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Audit {
                        action: AuditSubcommand::List { page_size: 0 },
                    },
                },
                true,
            )
            .await;
        assert!(audit_list_out.contains(r#""status":"error""#));
        assert!(audit_list_out.contains("unreachable"));

        let audit_verify_out = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Audit {
                        action: AuditSubcommand::Verify,
                    },
                },
                true,
            )
            .await;
        assert!(audit_verify_out.contains(r#""status":"error""#));
        assert!(audit_verify_out.contains("unreachable"));
    }

    #[test]
    fn runner_error_display() {
        let err = RunnerError::InitFailed("failed to open database".to_string());
        assert_eq!(
            err.to_string(),
            "failed to initialize engine: failed to open database"
        );
    }
}
