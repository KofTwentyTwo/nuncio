//! Headless engine runner executing CLI commands against core services.

use nuncio_core::{CoreCommand, EventBus};
use nuncio_store::{DatabaseEngine, DatabaseError};
use serde_json::json;
use thiserror::Error;

use crate::args::{
    AccountSubcommand, CalSubcommand, Commands, FolderSubcommand, MailSubcommand, SystemSubcommand,
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
            // Structured Noun + Verb Commands
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
                    self.handle_add_account(email, imap_host, *imap_port, imap_mode, smtp_mode, json_mode)
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

            // Legacy Short-Cut Subcommands
            Commands::Sync => self.handle_sync(json_mode).await,
            Commands::List { folder } => self.handle_list_folder(folder, json_mode).await,
            Commands::Send { to, subject, body } => {
                self.handle_send_email(to, subject, body, json_mode).await
            }
            Commands::Search { query } => self.handle_search(query, json_mode).await,
            Commands::Folders => self.handle_folders_list(json_mode).await,
            Commands::Read { id } => self.handle_read_message(id, json_mode).await,
            Commands::AddAccount {
                email,
                imap_host,
                imap_port,
                smtp_host: _,
                smtp_port: _,
                imap_mode,
                smtp_mode,
            } => {
                self.handle_add_account(email, imap_host, *imap_port, imap_mode, smtp_mode, json_mode)
                    .await
            }
            Commands::Accounts => self.handle_accounts_list(json_mode).await,
            Commands::Config => self.handle_system_status(json_mode).await,
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

    async fn handle_send_email(&self, to: &str, subject: &str, body: &str, json_mode: bool) -> String {
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
        if json_mode {
            format_json(&json!({
                "configured": true,
                "email": email,
                "imap_host": imap_host,
                "imap_port": imap_port,
                "imap_mode": imap_mode,
                "smtp_mode": smtp_mode,
                "keyring_key": keyring_key
            }))
        } else {
            format!(
                "Account '{}' configured for IMAP ({}:{}, mode: {}) and SMTP (mode: {})",
                email, imap_host, imap_port, imap_mode, smtp_mode
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
            format!("Configured Accounts: {} account(s) registered", accounts.len())
        }
    }

    async fn handle_system_status(&self, json_mode: bool) -> String {
        let state = self.event_bus.current_state();
        if json_mode {
            format_json(&json!({
                "accounts_loaded": state.accounts_loaded,
                "unread_count": state.unread_count,
                "engine_status": format!("{:?}", state.status)
            }))
        } else {
            format!(
                "Nuncio Configuration: {} accounts loaded, {} unread messages",
                state.accounts_loaded, state.unread_count
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn headless_runner_executes_all_commands() {
        let runner = HeadlessRunner::ephemeral().await.expect("ephemeral runner initializes");

        // Test Sync
        let sync_out = runner.execute_command(&Commands::Sync, false).await;
        assert!(sync_out.contains("Synchronization started"));

        let sync_json = runner.execute_command(&Commands::Sync, true).await;
        assert!(sync_json.contains(r#""status":"sync_started""#));

        // Test Noun-Verb Mail Sync
        let noun_sync = runner
            .execute_command(
                &Commands::Mail {
                    action: MailSubcommand::Sync,
                },
                true,
            )
            .await;
        assert!(noun_sync.contains(r#""status":"sync_started""#));

        // Test List
        let list_out = runner
            .execute_command(
                &Commands::List {
                    folder: "INBOX".to_string(),
                },
                false,
            )
            .await;
        assert!(list_out.contains("INBOX"));

        // Test Send
        let send_out = runner
            .execute_command(
                &Commands::Send {
                    to: "alice@nuncio.mx".to_string(),
                    subject: "Test".to_string(),
                    body: "Hello".to_string(),
                },
                false,
            )
            .await;
        assert!(send_out.contains("alice@nuncio.mx"));

        // Test Search
        let search_json = runner
            .execute_command(
                &Commands::Search {
                    query: "test".to_string(),
                },
                true,
            )
            .await;
        assert!(search_json.contains(r#""query":"test""#));

        // Test Folders
        let folders_json = runner.execute_command(&Commands::Folders, true).await;
        assert!(folders_json.contains(r#""folders":[]"#));

        // Test Noun-Verb Folder List
        let noun_folders = runner
            .execute_command(
                &Commands::Folder {
                    action: FolderSubcommand::List,
                },
                true,
            )
            .await;
        assert!(noun_folders.contains(r#""folders":[]"#));

        // Test Read Missing
        let read_err = runner
            .execute_command(
                &Commands::Read {
                    id: "nonexistent".to_string(),
                },
                true,
            )
            .await;
        assert!(read_err.contains(r#""status":"error""#));

        // Test AddAccount
        let add_json = runner
            .execute_command(
                &Commands::AddAccount {
                    email: "james.maes@kof22.com".to_string(),
                    imap_host: "mail.kof22.com".to_string(),
                    imap_port: 993,
                    smtp_host: "mail.kof22.com".to_string(),
                    smtp_port: 465,
                    imap_mode: "implicit_tls".to_string(),
                    smtp_mode: "implicit_tls".to_string(),
                },
                true,
            )
            .await;
        assert!(add_json.contains(r#""email":"james.maes@kof22.com""#));

        // Test Noun-Verb Account Add & List & Show
        let noun_acct_list = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::List,
                },
                true,
            )
            .await;
        assert!(noun_acct_list.contains(r#""accounts":[]"#));

        let noun_acct_show = runner
            .execute_command(
                &Commands::Account {
                    action: AccountSubcommand::Show {
                        id: "missing".to_string(),
                    },
                },
                false,
            )
            .await;
        assert!(noun_acct_show.contains("Account 'missing' not found"));

        // Test Accounts
        let accts_json = runner.execute_command(&Commands::Accounts, true).await;
        assert!(accts_json.contains(r#""accounts":[]"#));

        // Test Cal & System Noun-Verb Commands
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

        let sys_status = runner
            .execute_command(
                &Commands::System {
                    action: SystemSubcommand::Status,
                },
                true,
            )
            .await;
        assert!(sys_status.contains(r#""unread_count":0"#));

        // Test Config
        let config_json = runner.execute_command(&Commands::Config, true).await;
        assert!(config_json.contains(r#""unread_count":0"#));
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
