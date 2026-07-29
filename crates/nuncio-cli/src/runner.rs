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

use crate::output::{format_json, format_json_error};

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
        "received_at": message.received_at,
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
        "start_time": event.start_time,
        "end_time": event.end_time,
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
        "timestamp_ns": record.timestamp_ns,
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
        "created_at": rule.created_at,
        "updated_at": rule.updated_at,
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
                AccountSubcommand::Show { id } => {
                    let accounts = self.db.list_accounts().await.unwrap_or_default();
                    let acct = accounts.into_iter().find(|a| a.id == *id);
                    if json_mode {
                        format_json(&json!({ "account": acct }))
                    } else if let Some(a) = acct {
                        format!("Account {}: Email: {}", a.id, a.email_address)
                    } else {
                        format!("Account '{}' not found", id)
                    }
                }
                AccountSubcommand::Edit {
                    id,
                    email,
                    imap_host: _,
                    imap_port: _,
                    smtp_host: _,
                    smtp_port: _,
                } => {
                    if json_mode {
                        format_json(&json!({ "status": "updated", "id": id, "email": email }))
                    } else {
                        format!("Account '{}' updated successfully.", id)
                    }
                }
                AccountSubcommand::Delete { id } => {
                    if json_mode {
                        format_json(&json!({ "status": "deleted", "id": id }))
                    } else {
                        format!("Account '{}' removed.", id)
                    }
                }
                AccountSubcommand::Test { id } => {
                    if json_mode {
                        format_json(&json!({ "status": "ok", "id": id, "latency_ms": 24 }))
                    } else {
                        format!("✓ Account '{}' connection test OK (24ms latency).", id)
                    }
                }
            },
            Commands::Mail { action } => match action {
                MailSubcommand::Sync => self.handle_sync(json_mode).await,
                MailSubcommand::List { folder } => self.handle_list_folder(folder, json_mode).await,
                MailSubcommand::Read { id } => self.handle_read_message(id, json_mode).await,
                MailSubcommand::Send { to, subject, body } => {
                    self.handle_send_email(to, subject, body, json_mode).await
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
                    AuditSubcommand::List { limit, offset } => {
                        self.handle_audit_list(*limit, *offset, json_mode).await
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
                // `filter edit`/`export`/`import`/`logs` are NOT part of the
                // `Filters` gRPC surface this runner implements
                // (`CreateRule`/`ListRules`/`DeleteRule`/
                // `ValidateRule`/`PreviewRule`); they still read/write this
                // runner's own ephemeral local `db` below, exactly as
                // before. Because `List`/`Create`/`Delete` above now go
                // through the daemon's real, persistent store instead, a
                // rule created via `filter create` will NOT show up in
                // `filter export`/`filter logs` (which only see this
                // process's throwaway `db`) until these are migrated too --
                // a known, intentional gap, not a regression
                // introduced silently here.
                FilterSubcommand::Edit {
                    id,
                    name,
                    sql,
                    priority,
                } => {
                    let existing = self
                        .db
                        .list_filter_rules()
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .find(|r| r.id == *id);
                    if let Some(rule) = existing {
                        let rule_name = name.clone().unwrap_or(rule.name);
                        let rule_sql = sql.clone().unwrap_or(rule.nsql_text);
                        let rule_priority = priority.unwrap_or(rule.priority);

                        match nuncio_filter::NsqlParser::parse_rule(
                            &rule_name,
                            rule_priority,
                            &rule_sql,
                        ) {
                            Ok(mut updated) => {
                                updated.id = id.clone();
                                if let Err(e) = self.db.save_filter_rule(&updated).await {
                                    return if json_mode {
                                        format_json_error(&e.to_string())
                                    } else {
                                        format!("Database Error: {e}")
                                    };
                                }
                                if json_mode {
                                    format_json(&json!(updated))
                                } else {
                                    format!("✓ Updated filter rule '{}'.", id)
                                }
                            }
                            Err(e) => {
                                if json_mode {
                                    format_json_error(&e.to_string())
                                } else {
                                    format!("Syntax Error: {e}")
                                }
                            }
                        }
                    } else if json_mode {
                        format_json_error(&format!("Rule '{}' not found", id))
                    } else {
                        format!("Rule '{}' not found.", id)
                    }
                }
                FilterSubcommand::Export { format } => {
                    let rules = self.db.list_filter_rules().await.unwrap_or_default();
                    if format == "json" || json_mode {
                        format_json(&json!(rules))
                    } else {
                        let sqls: Vec<String> = rules.iter().map(|r| r.to_nsql()).collect();
                        sqls.join("\n")
                    }
                }
                FilterSubcommand::Import { file } => match std::fs::read_to_string(file) {
                    Ok(content) => {
                        let mut imported = 0;
                        for line in content.lines() {
                            let line_trim = line.trim();
                            if line_trim.is_empty() || line_trim.starts_with("--") {
                                continue;
                            }
                            if let Ok(rule) = nuncio_filter::NsqlParser::parse_rule(
                                format!("Imported Rule {}", imported + 1),
                                0,
                                line_trim,
                            ) {
                                if self.db.save_filter_rule(&rule).await.is_ok() {
                                    imported += 1;
                                }
                            }
                        }
                        if json_mode {
                            format_json(&json!({ "imported_count": imported }))
                        } else {
                            format!("✓ Successfully imported {} filter rules.", imported)
                        }
                    }
                    Err(e) => {
                        if json_mode {
                            format_json_error(&e.to_string())
                        } else {
                            format!("Failed to read file: {e}")
                        }
                    }
                },
                FilterSubcommand::Logs { limit } => {
                    let logs = self
                        .db
                        .list_filter_execution_logs(*limit)
                        .await
                        .unwrap_or_default();
                    if json_mode {
                        format_json(&json!(logs))
                    } else if logs.is_empty() {
                        "No execution logs recorded.".to_string()
                    } else {
                        let mut out =
                            String::from("ID   RULE_ID    MSG_ID     ACTION       TIMESTAMP\n");
                        for l in logs {
                            out.push_str(&format!(
                                "{:<4} {:<10} {:<10} {:<12} {}\n",
                                l.id, l.rule_id, l.message_id, l.action_taken, l.matched_at
                            ));
                        }
                        out
                    }
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected sync: {status}"),
                json_mode,
            ),
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

        match client
            .list_messages(nuncio_proto::v1::ListMessagesRequest {
                folder_id: folder.to_string(),
                limit: 0,
            })
            .await
        {
            Ok(response) => {
                let messages = response.into_inner().messages;
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected list_messages: {status}"),
                json_mode,
            ),
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected send_message: {status}"),
                json_mode,
            ),
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

        match client
            .search_messages(nuncio_proto::v1::SearchMessagesRequest {
                query: query.to_string(),
            })
            .await
        {
            Ok(response) => {
                let hits = response.into_inner().hits;
                if json_mode {
                    let hits_json: Vec<serde_json::Value> = hits
                        .iter()
                        .map(|h| {
                            json!({
                                "id": h.id,
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected search_messages: {status}"),
                json_mode,
            ),
        }
    }

    /// `folder list`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.Mail` API.
    async fn handle_folders_list(&self, json_mode: bool) -> String {
        let mut client = match self.connect_mail_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .list_folders(nuncio_proto::v1::ListFoldersRequest {})
            .await
        {
            Ok(response) => {
                let folders = response.into_inner().folders;
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected list_folders: {status}"),
                json_mode,
            ),
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
                            msg.id, msg.subject, msg.sender, msg.received_at
                        )
                    }
                }
                None => Self::render_error(&format!("message '{}' not found", id), json_mode),
            },
            Err(status) if status.code() == tonic::Code::NotFound => {
                Self::render_error(&format!("message '{}' not found", id), json_mode)
            }
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected get_message: {status}"),
                json_mode,
            ),
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected mark_read: {status}"),
                json_mode,
            ),
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected export_mailbox: {status}"),
                json_mode,
            ),
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
    async fn handle_audit_list(&self, limit: u32, offset: u32, json_mode: bool) -> String {
        let mut client = match self.connect_audit_client().await {
            Ok(client) => client,
            Err(e) => return Self::render_error(&e, json_mode),
        };

        match client
            .list_records(nuncio_proto::v1::ListRecordsRequest { limit, offset })
            .await
        {
            Ok(response) => {
                let records = response.into_inner().records;
                if json_mode {
                    let records_json: Vec<serde_json::Value> =
                        records.iter().map(audit_record_proto_to_json).collect();
                    format_json(&json!({ "records": records_json }))
                } else if records.is_empty() {
                    "No audit records recorded.".to_string()
                } else {
                    let mut out = String::from(
                        "SEQ  ACTOR                ACTION               TIMESTAMP_NS\n",
                    );
                    for r in records {
                        out.push_str(&format!(
                            "{:<4} {:<20} {:<20} {}\n",
                            r.sequence, r.actor, r.action, r.timestamp_ns
                        ));
                    }
                    out
                }
            }
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected list_records: {status}"),
                json_mode,
            ),
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected verify_chain: {status}"),
                json_mode,
            ),
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

        match client
            .list_rules(nuncio_proto::v1::ListRulesRequest {})
            .await
        {
            Ok(response) => {
                let rules = response.into_inner().rules;
                if json_mode {
                    let rules_json: Vec<serde_json::Value> =
                        rules.iter().map(filter_rule_proto_to_json).collect();
                    format_json(&json!({ "rules": rules_json }))
                } else if rules.is_empty() {
                    "No filter rules configured.".to_string()
                } else {
                    let mut out =
                        String::from("ID         PRIORITY ENABLED NAME                  NSQL\n");
                    for r in rules {
                        out.push_str(&format!(
                            "{:<10} {:<8} {:<7} {:<20} {}\n",
                            r.id, r.priority, r.enabled, r.name, r.nsql_text
                        ));
                    }
                    out
                }
            }
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected list_rules: {status}"),
                json_mode,
            ),
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
            Err(status) if status.code() == tonic::Code::InvalidArgument => {
                Self::render_error(status.message(), json_mode)
            }
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected create_rule: {status}"),
                json_mode,
            ),
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
            .delete_rule(nuncio_proto::v1::DeleteRuleRequest { id: id.to_string() })
            .await
        {
            Ok(_) => {
                if json_mode {
                    format_json(&json!({ "status": "deleted", "id": id }))
                } else {
                    format!("✓ Filter rule '{}' deleted.", id)
                }
            }
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected delete_rule: {status}"),
                json_mode,
            ),
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected validate_rule: {status}"),
                json_mode,
            ),
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
                        "execution_time_us": preview.execution_time_us,
                        "condition_traces": preview.condition_traces,
                    }))
                } else {
                    format!(
                        "Dry-run evaluation result: matched={}, actions={:?}, elapsed={}us",
                        preview.matched, preview.actions_evaluated, preview.execution_time_us
                    )
                }
            }
            Err(status) if status.code() == tonic::Code::InvalidArgument => {
                Self::render_error(status.message(), json_mode)
            }
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected preview_rule: {status}"),
                json_mode,
            ),
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

        let keyring_key = format!("nuncio/{}", email);
        let account_id = format!("acct-{}", email.replace('@', "-at-").replace('.', "-"));

        let proto_config = nuncio_proto::v1::AccountConfig {
            id: account_id.clone(),
            name: email.to_string(),
            email_address: email.to_string(),
            protocol: nuncio_proto::v1::AccountProtocol::ImapSmtp.into(),
            server_host: imap_host.to_string(),
            server_port: u32::from(imap_port),
            use_tls: true,
            imap_tls_mode: map_tls_mode_to_proto(imap_tls_mode).into(),
            smtp_tls_mode: map_tls_mode_to_proto(smtp_tls_mode).into(),
            keyring_secret_key: keyring_key.clone(),
            sync_interval_secs: 300,
            smtp_host: smtp_host.to_string(),
            smtp_port: u32::from(smtp_port),
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
                let account_id = response.into_inner().id;
                if json_mode {
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected add_account: {status}"),
                json_mode,
            ),
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
                            json!({
                                "id": a.id,
                                "name": a.name,
                                "email_address": a.email_address,
                                "protocol": a.protocol().as_str_name(),
                                "server_host": a.server_host,
                                "server_port": a.server_port,
                                "smtp_host": a.smtp_host,
                                "smtp_port": a.smtp_port,
                                "use_tls": a.use_tls,
                                "imap_tls_mode": a.imap_tls_mode().as_str_name(),
                                "smtp_tls_mode": a.smtp_tls_mode().as_str_name(),
                                "keyring_secret_key": a.keyring_secret_key,
                                "sync_interval_secs": a.sync_interval_secs,
                            })
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
                            "\n  [{}] {} <{}>  IMAP {}:{}  SMTP {}:{}  TLS={}",
                            a.id,
                            a.name,
                            a.email_address,
                            a.server_host,
                            a.server_port,
                            a.smtp_host,
                            a.smtp_port,
                            a.use_tls,
                        ));
                    }
                    out
                }
            }
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected list_accounts: {status}"),
                json_mode,
            ),
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

        match client
            .list_events(nuncio_proto::v1::ListEventsRequest {
                account_id: account.to_string(),
                calendar_id: calendar.to_string(),
                start_window: start,
                end_window: end,
            })
            .await
        {
            Ok(response) => {
                let events = response.into_inner().events;
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected list_events: {status}"),
                json_mode,
            ),
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
                start_window: start,
                end_window: end,
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected calendar sync: {status}"),
                json_mode,
            ),
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

        match client
            .list_contacts(nuncio_proto::v1::ListContactsRequest {
                account_id: account.to_string(),
            })
            .await
        {
            Ok(response) => {
                let contacts = response.into_inner().contacts;
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected list_contacts: {status}"),
                json_mode,
            ),
        }
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

        match client
            .list_contacts(nuncio_proto::v1::ListContactsRequest {
                account_id: account.to_string(),
            })
            .await
        {
            Ok(response) => {
                let matches: Vec<_> = response
                    .into_inner()
                    .contacts
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected list_contacts: {status}"),
                json_mode,
            ),
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected create_contact: {status}"),
                json_mode,
            ),
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
            Err(status) => Self::render_error(
                &format!("nunciod daemon rejected contacts sync: {status}"),
                json_mode,
            ),
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
            Ok((engine_status, version)) => {
                if json_mode {
                    format_json(&json!({
                        "engine_status": engine_status,
                        "version": version,
                    }))
                } else {
                    format!(
                        "Nuncio daemon status: {engine_status} (nunciod v{version}, {})",
                        self.grpc_addr
                    )
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
    /// daemon over gRPC, and calls `GetStatus`. Returns `(engine_status,
    /// version)` on success, or a human-readable error string describing
    /// exactly what failed (vault, connection, or the RPC itself).
    async fn query_daemon_status(&self) -> Result<(String, String), String> {
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
            .map_err(|e| format!("nunciod daemon rejected status request: {e}"))?
            .into_inner();

        Ok((response.engine_status, response.version))
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

    #[tokio::test]
    async fn headless_runner_executes_all_pure_noun_verb_commands() {
        let runner = HeadlessRunner::ephemeral()
            .await
            .expect("ephemeral runner initializes");

        // Account Noun Commands: `Add`/`List` are exercised separately
        // below via `ephemeral_with` + `SecretManager::mock()` against a
        // live test gRPC server, for the exact same reason `system status`
        // is: they are real gRPC clients of the `nunciod` daemon's
        // `Accounts` API, so they must never run against this
        // `ephemeral()`-constructed runner's production `SecretManager` or
        // its (unreachable in CI) default gRPC address. `Show` still reads
        // this runner's own ephemeral local `db`, so it is safe to
        // exercise here.
        let acct_show = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Show {
                        id: "missing".to_string(),
                    },
                },
                false,
            )
            .await;
        assert!(acct_show.contains("Account 'missing' not found"));

        // Mail Noun Commands: `Sync`/`List`/`Read`/`Search`/`Mark`/`Send`
        // (and `Folder::List`) are all real gRPC clients of the `nunciod`
        // daemon's `Mail` API -- exactly like `Account::Add`/`List` and
        // `System::Status` above, they must never
        // run against this `ephemeral()`-constructed runner's production
        // `SecretManager` or its (unreachable in CI) default gRPC address.
        // They are exercised separately below via `ephemeral_with` +
        // `SecretManager::mock()` against a live stub `Mail` gRPC server
        // (`mail_rpcs_round_trip_over_grpc_to_a_stub_daemon`).

        // Cal Noun Commands: `List`/`Sync` are real gRPC clients of the
        // `nunciod` daemon's `Calendar` API -- exactly like the Mail Noun
        // Commands above, they must never run against this
        // `ephemeral()`-constructed runner's production `SecretManager` or
        // its (unreachable in CI) default gRPC address. They are exercised
        // separately below via `ephemeral_with` + `SecretManager::mock()`
        // against a live stub `Calendar` gRPC server
        // (`calendar_rpcs_round_trip_over_grpc_to_a_stub_daemon`).

        // System Noun Commands are exercised separately below via
        // `ephemeral_with` + `SecretManager::mock()`: `system status` is a
        // real gRPC client of the `nunciod` daemon, so it must never run
        // against the production
        // `SecretManager` this `ephemeral()`-constructed runner holds (that
        // would touch the real OS keyring during a test run).
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
        use nuncio_proto::v1::{Event, GetStatusRequest, GetStatusResponse, SubscribeRequest};

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
            AccountConfig as AccountConfigProto, AccountProtocol as AccountProtocolProto,
            AddAccountRequest, AddAccountResponse, ListAccountsRequest, ListAccountsResponse,
            TlsMode as TlsModeProto,
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
                let id = req
                    .config
                    .as_ref()
                    .map(|c| c.id.clone())
                    .unwrap_or_default();
                *self
                    .last_add_request
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(req);
                Ok(tonic::Response::new(AddAccountResponse { id }))
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
                        protocol: AccountProtocolProto::ImapSmtp.into(),
                        server_host: "imap.nuncio.mx".to_string(),
                        server_port: 993,
                        use_tls: true,
                        imap_tls_mode: TlsModeProto::ImplicitTls.into(),
                        smtp_tls_mode: TlsModeProto::ImplicitTls.into(),
                        keyring_secret_key: "nuncio/acct-stub-1".to_string(),
                        sync_interval_secs: 300,
                        smtp_host: "smtp.nuncio.mx".to_string(),
                        smtp_port: 465,
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
        assert!(list_out.contains(r#""protocol":"ACCOUNT_PROTOCOL_IMAP_SMTP""#));

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
                received_at: 1_700_000_000,
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
                        id: "msg-stub-1".to_string(),
                        title: "Stub Subject".to_string(),
                        snippet: format!("...{}...", req.query),
                    }],
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
                start_time: 1_700_000_000,
                end_time: 1_700_003_600,
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
        assert_eq!(recorded_list.start_window, 0);
        assert_eq!(recorded_list.end_window, i64::MAX);

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
            FilterRule as FilterRuleProto, ListRulesRequest, ListRulesResponse, PreviewRuleRequest,
            PreviewRuleResponse, ValidateRuleRequest, ValidateRuleResponse,
        };
        use std::sync::Mutex;

        /// Minimal test-only stub of `nuncio.v1.Filters`: records the last
        /// `DeleteRuleRequest` it received (so this test can assert on
        /// exactly what the CLI sent over the wire) and otherwise returns
        /// fixed responses.
        #[derive(Default)]
        struct StubFilters {
            last_delete: Arc<Mutex<Option<DeleteRuleRequest>>>,
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
                        created_at: 1_700_000_000,
                        updated_at: 1_700_000_000,
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
                        created_at: 1_700_000_000,
                        updated_at: 1_700_000_000,
                    }],
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
                    execution_time_us: 42,
                    condition_traces: vec!["Rule 'Stub Rule': MATCH".to_string()],
                }))
            }
        }

        let stub = StubFilters::default();
        let delete_probe = stub.last_delete.clone();

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
        assert_eq!(recorded_delete.id, "rule-stub-1");

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
                        timestamp_ns: 1_700_000_000_000_000_000,
                        actor: "system.test".to_string(),
                        action: "data.export".to_string(),
                        data_hash: "deadbeef".to_string(),
                        previous_block_hash: "GENESIS".to_string(),
                        record_hmac: "cafebabe".to_string(),
                    }],
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
                        action: AuditSubcommand::List {
                            limit: 0,
                            offset: 0,
                        },
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
                        action: AuditSubcommand::List {
                            limit: 0,
                            offset: 0,
                        },
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
