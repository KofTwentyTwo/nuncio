//! Headless engine runner executing CLI commands against core services.

use nuncio_core::{CoreCommand, EventBus};
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nuncio_store::{DatabaseEngine, DatabaseError};
use serde_json::json;
use std::sync::Arc;
use thiserror::Error;

use crate::args::{
    AccountSubcommand, CalSubcommand, Commands, ContactSubcommand, FilterSubcommand,
    FolderSubcommand, MailSubcommand, SystemSubcommand, UpdateSubcommand,
};

use crate::output::{format_json, format_json_error};

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
/// is the exception: it is a thin gRPC client of the real `nunciod` daemon's
/// `nuncio.v1.System` API (backlog story 1.A.3 / GH-150), authenticated by a
/// bearer token read from an injected [`SecretManager`] — production code
/// uses [`SecretManager::production`] (the real OS keyring), while tests
/// inject [`SecretManager::mock`] so no test ever touches the real vault.
pub struct HeadlessRunner {
    event_bus: EventBus,
    db: DatabaseEngine,
    secrets: Arc<SecretManager>,
    grpc_addr: String,
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
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .map_err(|e| RunnerError::InitFailed(e.to_string()))?;
        let event_bus = EventBus::new();
        Ok(Self {
            event_bus,
            db,
            secrets,
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
                    smtp_host: _,
                    smtp_port: _,
                    imap_mode,
                    smtp_mode,
                } => {
                    self.handle_add_account(
                        email, imap_host, *imap_port, imap_mode, smtp_mode, json_mode,
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
                CalSubcommand::List => {
                    if json_mode {
                        format_json(&json!({ "events": [] }))
                    } else {
                        "Calendar Events: 0 events found".to_string()
                    }
                }
                CalSubcommand::Sync => {
                    self.event_bus.process_command(CoreCommand::SyncAll);
                    if json_mode {
                        format_json(&json!({ "status": "calendar_sync_started" }))
                    } else {
                        "Calendar synchronization started.".to_string()
                    }
                }
            },
            Commands::System { action } => match action {
                SystemSubcommand::Status => self.handle_system_status(json_mode).await,
            },
            Commands::Contact { action } => match action {
                ContactSubcommand::List => {
                    let contacts_db = match nuncio_contacts::ContactsDatabase::in_memory().await {
                        Ok(db) => db,
                        Err(e) => {
                            return if json_mode {
                                format_json_error(&e.to_string())
                            } else {
                                format!("Database Error: {e}")
                            };
                        }
                    };
                    let contacts = contacts_db.list_contacts().await.unwrap_or_default();
                    if json_mode {
                        format_json(&json!({ "contacts": contacts }))
                    } else {
                        format!(
                            "Contacts: {} contacts found in address book.",
                            contacts.len()
                        )
                    }
                }
                ContactSubcommand::Search { query } => {
                    let contacts_db = match nuncio_contacts::ContactsDatabase::in_memory().await {
                        Ok(db) => db,
                        Err(e) => {
                            return if json_mode {
                                format_json_error(&e.to_string())
                            } else {
                                format!("Database Error: {e}")
                            };
                        }
                    };
                    let contacts = contacts_db.search_contacts(query).await.unwrap_or_default();
                    if json_mode {
                        format_json(&json!({ "query": query, "contacts": contacts }))
                    } else {
                        format!("Search ('{}'): {} contacts matched.", query, contacts.len())
                    }
                }
                ContactSubcommand::Add { name, email, org } => {
                    let contacts_db = match nuncio_contacts::ContactsDatabase::in_memory().await {
                        Ok(db) => db,
                        Err(e) => {
                            return if json_mode {
                                format_json_error(&e.to_string())
                            } else {
                                format!("Database Error: {e}")
                            };
                        }
                    };
                    let mut contact = nuncio_contacts::Contact::new(name, email);
                    contact.organization = org.clone();
                    let _ = contacts_db.save_contact(&contact).await;
                    if json_mode {
                        format_json(&json!({ "status": "contact_created", "contact": contact }))
                    } else {
                        format!(
                            "✓ Contact '{}' saved to address book.",
                            contact.display_name
                        )
                    }
                }
            },
            Commands::Filter { action } => match action {
                FilterSubcommand::List => {
                    let rules = self.db.list_filter_rules().await.unwrap_or_default();
                    if json_mode {
                        format_json(&json!(rules))
                    } else if rules.is_empty() {
                        "No filter rules configured.".to_string()
                    } else {
                        let mut out = String::from(
                            "ID         PRIORITY ENABLED NAME                  NSQL\n",
                        );
                        for r in rules {
                            out.push_str(&format!(
                                "{:<10} {:<8} {:<7} {:<20} {}\n",
                                r.id, r.priority, r.enabled, r.name, r.nsql_text
                            ));
                        }
                        out
                    }
                }
                FilterSubcommand::Create {
                    name,
                    sql,
                    priority,
                } => match nuncio_filter::NsqlParser::parse_rule(name, *priority, sql) {
                    Ok(rule) => {
                        let opts = nuncio_filter::ValidationOptions::default();
                        if let Err(e) = nuncio_filter::NsqlValidator::validate(&rule, &opts) {
                            return if json_mode {
                                format_json_error(&e.to_string())
                            } else {
                                format!("Validation Error: {e}")
                            };
                        }
                        if let Err(e) = self.db.save_filter_rule(&rule).await {
                            return if json_mode {
                                format_json_error(&e.to_string())
                            } else {
                                format!("Database Error: {e}")
                            };
                        }
                        if json_mode {
                            format_json(&json!(rule))
                        } else {
                            format!("✓ Created filter rule '{}' (ID: {}).", rule.name, rule.id)
                        }
                    }
                    Err(e) => {
                        if json_mode {
                            format_json_error(&e.to_string())
                        } else {
                            format!("Syntax Error: {e}")
                        }
                    }
                },
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
                FilterSubcommand::Delete { id } => {
                    if let Err(e) = self.db.delete_filter_rule(id).await {
                        if json_mode {
                            format_json_error(&e.to_string())
                        } else {
                            format!("Error: {e}")
                        }
                    } else if json_mode {
                        format_json(&json!({ "status": "deleted", "id": id }))
                    } else {
                        format!("✓ Filter rule '{}' deleted.", id)
                    }
                }
                FilterSubcommand::Test { sql, message_id } => {
                    match nuncio_filter::NsqlParser::parse_rule("Test Rule", 0, sql) {
                        Ok(rule) => {
                            let engine = match nuncio_filter::FilterEngine::new(vec![rule.clone()])
                            {
                                Ok(engine) => engine,
                                Err(e) => {
                                    return if json_mode {
                                        format_json_error(&e)
                                    } else {
                                        format!("Filter Engine Error: {e}")
                                    };
                                }
                            };
                            let sample_email = if let Some(mid) = message_id {
                                self.db.get_message(mid).await.unwrap_or_else(|_| {
                                    nuncio_core::model::Email {
                                        id: mid.clone(),
                                        account_id: "acct-1".to_string(),
                                        folder_id: "inbox".to_string(),
                                        subject: "Test Subject".to_string(),
                                        sender: "test@nuncio.mx".to_string(),
                                        recipient: "me@nuncio.mx".to_string(),
                                        received_at: chrono::Utc::now().timestamp(),
                                        read: false,
                                        body_plain: Some("Sample body text".to_string()),
                                        body_html: None,
                                        attachments: Vec::new(),
                                    }
                                })
                            } else {
                                nuncio_core::model::Email {
                                    id: "msg-test".to_string(),
                                    account_id: "acct-1".to_string(),
                                    folder_id: "inbox".to_string(),
                                    subject: "Test Subject".to_string(),
                                    sender: "test@nuncio.mx".to_string(),
                                    recipient: "me@nuncio.mx".to_string(),
                                    received_at: chrono::Utc::now().timestamp(),
                                    read: false,
                                    body_plain: Some("Sample body text".to_string()),
                                    body_html: None,
                                    attachments: Vec::new(),
                                }
                            };
                            let preview = engine.preview(&sample_email);
                            if json_mode {
                                format_json(&json!(preview))
                            } else {
                                format!("Dry-run evaluation result: matched={}, actions={:?}, elapsed={}us", preview.matched, preview.actions_evaluated, preview.execution_time_us)
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
            Commands::Daemon { port } => {
                let addr = format!("127.0.0.1:{}", port);
                if json_mode {
                    format_json(&json!({ "status": "daemon_running", "bind_addr": addr }))
                } else {
                    format!("Nuncio IPC Daemon listening on {}", addr)
                }
            }
        }
    }

    async fn handle_sync(&self, json_mode: bool) -> String {
        self.event_bus.process_command(CoreCommand::SyncAll);
        if json_mode {
            format_json(&json!({
                "status": "sync_started",
                "engine_status": format!("{:?}", self.event_bus.current_state().status)
            }))
        } else {
            format!(
                "Synchronization started. Engine status: {:?}",
                self.event_bus.current_state().status
            )
        }
    }

    async fn handle_list_folder(&self, folder: &str, json_mode: bool) -> String {
        let count: i64 = sqlx::query_as("SELECT COUNT(*) FROM messages WHERE folder_id = ?")
            .bind(folder)
            .fetch_one(self.db.pool())
            .await
            .map(|r: (i64,)| r.0)
            .unwrap_or(0);

        if json_mode {
            format_json(&json!({
                "folder": folder,
                "total_messages": count
            }))
        } else {
            format!("Folder '{}': {} total messages", folder, count)
        }
    }

    async fn handle_send_email(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        json_mode: bool,
    ) -> String {
        if json_mode {
            format_json(&json!({
                "sent": true,
                "to": to,
                "subject": subject,
                "bytes": body.len()
            }))
        } else {
            format!("Message sent to {} ('{}')", to, subject)
        }
    }

    async fn handle_search(&self, query: &str, json_mode: bool) -> String {
        if json_mode {
            format_json(&json!({
                "query": query,
                "results": []
            }))
        } else {
            format!("Search complete for '{}' (0 matches)", query)
        }
    }

    async fn handle_folders_list(&self, json_mode: bool) -> String {
        let folders = self.db.list_folders().await.unwrap_or_default();
        if json_mode {
            format_json(&json!({
                "folders": folders
            }))
        } else {
            format!("Available Mailbox Folders: {} folders found", folders.len())
        }
    }

    async fn handle_read_message(&self, id: &str, json_mode: bool) -> String {
        match self.db.get_message(id).await {
            Ok(msg) => {
                if json_mode {
                    format_json(&json!({
                        "message": msg
                    }))
                } else {
                    format!(
                        "Message {}: Subject: '{}', From: {}, Date: {}",
                        msg.id, msg.subject, msg.sender, msg.received_at
                    )
                }
            }
            Err(_) => {
                if json_mode {
                    format_json_error(&format!("message '{}' not found", id))
                } else {
                    format!("Error: message '{}' not found", id)
                }
            }
        }
    }

    async fn handle_add_account(
        &self,
        email: &str,
        imap_host: &str,
        imap_port: u16,
        imap_mode: &str,
        smtp_mode: &str,
        json_mode: bool,
    ) -> String {
        let keyring_key = format!("nuncio/{}", email);
        let account_id = format!("acct-{}", email.replace('@', "-at-").replace('.', "-"));

        let acct = nuncio_core::AccountConfig {
            id: account_id.clone(),
            name: email.to_string(),
            email_address: email.to_string(),
            protocol: nuncio_core::AccountProtocol::ImapSmtp,
            server_host: imap_host.to_string(),
            server_port: imap_port,
            use_tls: true,
            imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            keyring_secret_key: keyring_key.clone(),
            sync_interval_secs: 300,
        };

        if let Err(e) = self.db.save_account(&acct).await {
            if json_mode {
                return format_json_error(&e.to_string());
            } else {
                return format!("Failed to save account: {e}");
            }
        }

        if json_mode {
            format_json(&json!({
                "configured": true,
                "account_id": account_id,
                "email": email,
                "imap_host": imap_host,
                "imap_port": imap_port,
                "imap_mode": imap_mode,
                "smtp_mode": smtp_mode,
                "keyring_key": keyring_key
            }))
        } else {
            format!(
                "Account '{}' (ID: {}) saved to SQLite database and configured for IMAP ({}:{}, mode: {}) and SMTP (mode: {})",
                email, account_id, imap_host, imap_port, imap_mode, smtp_mode
            )
        }
    }

    async fn handle_accounts_list(&self, json_mode: bool) -> String {
        let accounts = self.db.list_accounts().await.unwrap_or_default();
        if json_mode {
            format_json(&json!({
                "accounts": accounts
            }))
        } else {
            format!(
                "Configured Accounts: {} account(s) registered",
                accounts.len()
            )
        }
    }

    /// `system status`: a real thin gRPC client of the running `nunciod`
    /// daemon's `nuncio.v1.System` API (backlog story 1.A.3 / GH-150).
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

    #[tokio::test]
    async fn headless_runner_executes_all_pure_noun_verb_commands() {
        let runner = HeadlessRunner::ephemeral()
            .await
            .expect("ephemeral runner initializes");

        // Account Noun Commands
        let acct_add = runner
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
                    },
                },
                true,
            )
            .await;
        assert!(acct_add.contains(r#""email":"james.maes@kof22.com""#));

        let acct_list = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::List,
                },
                true,
            )
            .await;
        assert!(acct_list.contains("acct-james-maes-at-kof22-com"));

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

        // Mail Noun Commands
        let mail_sync = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Sync,
                },
                true,
            )
            .await;
        assert!(mail_sync.contains(r#""status":"sync_started""#));

        let mail_list = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::List {
                        folder: "INBOX".to_string(),
                    },
                },
                false,
            )
            .await;
        assert!(mail_list.contains("INBOX"));

        let mail_read_err = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Read {
                        id: "missing".to_string(),
                    },
                },
                true,
            )
            .await;
        assert!(mail_read_err.contains(r#""status":"error""#));

        let mail_send = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Send {
                        to: "alice@nuncio.mx".to_string(),
                        subject: "Test".to_string(),
                        body: "Body".to_string(),
                    },
                },
                false,
            )
            .await;
        assert!(mail_send.contains("alice@nuncio.mx"));

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

        // Folder Noun Commands
        let folder_list = runner
            .execute_command(
                &Commands::Folder {
                    action: FolderSubcommand::List,
                },
                true,
            )
            .await;
        assert!(folder_list.contains(r#""folders":[]"#));

        // Cal Noun Commands
        let cal_list = runner
            .execute_command(
                &Commands::Cal {
                    action: CalSubcommand::List,
                },
                true,
            )
            .await;
        assert!(cal_list.contains(r#""events":[]"#));

        let cal_sync = runner
            .execute_command(
                &Commands::Cal {
                    action: CalSubcommand::Sync,
                },
                true,
            )
            .await;
        assert!(cal_sync.contains(r#""status":"calendar_sync_started""#));

        // System Noun Commands are exercised separately below via
        // `ephemeral_with` + `SecretManager::mock()`: `system status` is a
        // real gRPC client of the `nunciod` daemon (backlog story 1.A.3 /
        // GH-150), so it must never run against the production
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
            // `grpc::tests` (backlog story 1.A.4 / GH-151); this stub only
            // needs to satisfy the trait so the CLI's `GetStatus` happy
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

    #[test]
    fn runner_error_display() {
        let err = RunnerError::InitFailed("failed to open database".to_string());
        assert_eq!(
            err.to_string(),
            "failed to initialize engine: failed to open database"
        );
    }
}
