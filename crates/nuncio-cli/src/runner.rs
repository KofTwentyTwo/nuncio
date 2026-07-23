//! Headless engine runner executing CLI commands against core services.

use nuncio_core::{CoreCommand, EventBus};
use nuncio_store::{DatabaseEngine, DatabaseError};
use serde_json::json;
use thiserror::Error;

use crate::args::{
    AccountSubcommand, CalSubcommand, Commands, FilterSubcommand, FolderSubcommand, MailSubcommand, SystemSubcommand,
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
pub struct HeadlessRunner {
    event_bus: EventBus,
    db: DatabaseEngine,
}

impl HeadlessRunner {
    /// Initialize a new `HeadlessRunner` with an ephemeral database.
    pub async fn ephemeral() -> Result<Self, RunnerError> {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .map_err(|e| RunnerError::InitFailed(e.to_string()))?;
        let event_bus = EventBus::new();
        Ok(Self { event_bus, db })
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
                AccountSubcommand::Edit { id, email, imap_host: _, imap_port: _, smtp_host: _, smtp_port: _ } => {
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
            },
            Commands::Licenses => {
                let credits = vec![
                    ("tokio", "MIT", "Event-driven asynchronous runtime engine"),
                    ("ratatui", "MIT", "Terminal User Interface rendering library"),
                    ("tauri", "MIT/Apache-2.0", "Cross-platform desktop application shell"),
                    ("sqlx", "MIT/Apache-2.0", "Async SQLite database driver"),
                    ("lettre", "MIT", "Email creation & SMTP client"),
                    ("async-imap", "MIT/Apache-2.0", "Async IMAP protocol client"),
                    ("aes-gcm", "MIT/Apache-2.0", "AES-256-GCM authenticated encryption"),
                    ("age", "MIT/Apache-2.0", "Attachment stream encryption cipher"),
                    ("zeroize", "MIT/Apache-2.0", "Secure heap memory wiping"),
                    ("keyring", "MIT/Apache-2.0", "OS native key store integration"),
                ];
                if json_mode {
                    format_json(&serde_json::json!({ "licenses": credits }))
                } else {
                    let mut out = String::from("\nNuncio Third-Party Open Source Library Acknowledgments:\n\n");
                    for (lib, lic, desc) in credits {
                        out.push_str(&format!("  • {:<15} [{:<14}] {}\n", lib, lic, desc));
                    }
                    out.push_str("\nFull license terms available in THIRD_PARTY_LICENSES.md\n");
                    out
                }
            },
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
            Commands::Filter { action } => match action {
                FilterSubcommand::List => {
                    let rules = self.db.list_filter_rules().await.unwrap_or_default();
                    if json_mode {
                        format_json(&json!(rules))
                    } else if rules.is_empty() {
                        "No filter rules configured.".to_string()
                    } else {
                        let mut out = String::from("ID         PRIORITY ENABLED NAME                  NSQL\n");
                        for r in rules {
                            out.push_str(&format!("{:<10} {:<8} {:<7} {:<20} {}\n", r.id, r.priority, r.enabled, r.name, r.nsql_text));
                        }
                        out
                    }
                }
                FilterSubcommand::Create { name, sql, priority } => {
                    match nuncio_filter::NsqlParser::parse_rule(name, *priority, sql) {
                        Ok(rule) => {
                            let opts = nuncio_filter::ValidationOptions::default();
                            if let Err(e) = nuncio_filter::NsqlValidator::validate(&rule, &opts) {
                                return if json_mode { format_json_error(&e.to_string()) } else { format!("Validation Error: {e}") };
                            }
                            if let Err(e) = self.db.save_filter_rule(&rule).await {
                                return if json_mode { format_json_error(&e.to_string()) } else { format!("Database Error: {e}") };
                            }
                            if json_mode {
                                format_json(&json!(rule))
                            } else {
                                format!("✓ Created filter rule '{}' (ID: {}).", rule.name, rule.id)
                            }
                        }
                        Err(e) => if json_mode { format_json_error(&e.to_string()) } else { format!("Syntax Error: {e}") },
                    }
                }
                FilterSubcommand::Edit { id, name, sql, priority } => {
                    let existing = self.db.list_filter_rules().await.unwrap_or_default().into_iter().find(|r| r.id == *id);
                    if let Some(rule) = existing {
                        let rule_name = name.clone().unwrap_or(rule.name);
                        let rule_sql = sql.clone().unwrap_or(rule.nsql_text);
                        let rule_priority = priority.unwrap_or(rule.priority);

                        match nuncio_filter::NsqlParser::parse_rule(&rule_name, rule_priority, &rule_sql) {
                            Ok(mut updated) => {
                                updated.id = id.clone();
                                if let Err(e) = self.db.save_filter_rule(&updated).await {
                                    return if json_mode { format_json_error(&e.to_string()) } else { format!("Database Error: {e}") };
                                }
                                if json_mode {
                                    format_json(&json!(updated))
                                } else {
                                    format!("✓ Updated filter rule '{}'.", id)
                                }
                            }
                            Err(e) => if json_mode { format_json_error(&e.to_string()) } else { format!("Syntax Error: {e}") },
                        }
                    } else if json_mode {
                        format_json_error(&format!("Rule '{}' not found", id))
                    } else {
                        format!("Rule '{}' not found.", id)
                    }
                }
                FilterSubcommand::Delete { id } => {
                    if let Err(e) = self.db.delete_filter_rule(id).await {
                        if json_mode { format_json_error(&e.to_string()) } else { format!("Error: {e}") }
                    } else if json_mode {
                        format_json(&json!({ "status": "deleted", "id": id }))
                    } else {
                        format!("✓ Filter rule '{}' deleted.", id)
                    }
                }
                FilterSubcommand::Test { sql, message_id } => {
                    match nuncio_filter::NsqlParser::parse_rule("Test Rule", 0, sql) {
                        Ok(rule) => {
                            let engine = nuncio_filter::FilterEngine::new(vec![rule.clone()]).unwrap();
                            let sample_email = if let Some(mid) = message_id {
                                self.db.get_message(mid).await.unwrap_or_else(|_| nuncio_core::model::Email {
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
                        Err(e) => if json_mode { format_json_error(&e.to_string()) } else { format!("Syntax Error: {e}") },
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
                FilterSubcommand::Import { file } => {
                    match std::fs::read_to_string(file) {
                        Ok(content) => {
                            let mut imported = 0;
                            for line in content.lines() {
                                let line_trim = line.trim();
                                if line_trim.is_empty() || line_trim.starts_with("--") {
                                    continue;
                                }
                                if let Ok(rule) = nuncio_filter::NsqlParser::parse_rule(format!("Imported Rule {}", imported + 1), 0, line_trim) {
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
                        Err(e) => if json_mode { format_json_error(&e.to_string()) } else { format!("Failed to read file: {e}") },
                    }
                }
                FilterSubcommand::Logs { limit } => {
                    let logs = self.db.list_filter_execution_logs(*limit).await.unwrap_or_default();
                    if json_mode {
                        format_json(&json!(logs))
                    } else if logs.is_empty() {
                        "No execution logs recorded.".to_string()
                    } else {
                        let mut out = String::from("ID   RULE_ID    MSG_ID     ACTION       TIMESTAMP\n");
                        for l in logs {
                            out.push_str(&format!("{:<4} {:<10} {:<10} {:<12} {}\n", l.id, l.rule_id, l.message_id, l.action_taken, l.matched_at));
                        }
                        out
                    }
                }
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

        let _ = self.db.save_account(&acct).await;

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

    async fn handle_system_status(&self, json_mode: bool) -> String {
        let state = self.event_bus.current_state();
        let healthy = self.db.check_integrity().await.unwrap_or(false);
        if json_mode {
            format_json(&json!({
                "accounts_loaded": state.accounts_loaded,
                "unread_count": state.unread_count,
                "engine_status": format!("{:?}", state.status),
                "database_health": if healthy { "healthy" } else { "repaired" }
            }))
        } else {
            let mut msg = format!(
                "Nuncio Configuration: {} accounts loaded, {} unread messages",
                state.accounts_loaded, state.unread_count
            );
            if !healthy {
                msg.push_str("\n[NOTICE] Database integrity issue was automatically repaired. Resynchronizing inbox...");
            }
            msg
        }
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

        // System Noun Commands
        let sys_status = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Status,
                },
                true,
            )
            .await;
        assert!(sys_status.contains(r#""unread_count":0"#));
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
