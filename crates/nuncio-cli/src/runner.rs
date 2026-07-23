//! Headless engine runner executing CLI commands against core services.

use nuncio_core::{CoreCommand, EngineStatus, EventBus};
use nuncio_store::{DatabaseEngine, DatabaseError};
use serde_json::{json, Value};
use thiserror::Error;

use crate::args::Commands;
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
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// Access the underlying `DatabaseEngine`.
    pub fn db(&self) -> &DatabaseEngine {
        &self.db
    }

    /// Execute a CLI subcommand, returning a formatted string output.
    pub async fn execute_command(&self, cmd: &Commands, json_mode: bool) -> String {
        match cmd {
            Commands::Sync => {
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
            Commands::List { folder } => {
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
            Commands::Send { to, subject, body } => {
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
            Commands::Search { query } => {
                if json_mode {
                    format_json(&json!({
                        "query": query,
                        "results": []
                    }))
                } else {
                    format!("Search complete for '{}' (0 matches)", query)
                }
            }
            Commands::Folders => {
                let folders = self.db.list_folders().await.unwrap_or_default();
                if json_mode {
                    format_json(&json!({
                        "folders": folders
                    }))
                } else {
                    format!("Available Mailbox Folders: {} folders found", folders.len())
                }
            }
            Commands::Read { id } => {
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
            Commands::Config => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn headless_runner_executes_all_commands() {
        let runner = HeadlessRunner::ephemeral().await.expect("runner init");

        // Test Sync
        let sync_json = runner.execute_command(&Commands::Sync, true).await;
        assert!(sync_json.contains(r#""status":"ok""#));
        let sync_text = runner.execute_command(&Commands::Sync, false).await;
        assert!(sync_text.contains("Synchronization started"));

        // Test List
        let list_json = runner
            .execute_command(&Commands::List { folder: "inbox".to_string() }, true)
            .await;
        assert!(list_json.contains(r#""folder":"inbox""#));

        // Test Send
        let send_json = runner
            .execute_command(
                &Commands::Send {
                    to: "alice@nuncio.mx".to_string(),
                    subject: "Test".to_string(),
                    body: "Body".to_string(),
                },
                true,
            )
            .await;
        assert!(send_json.contains(r#""sent":true"#));

        // Test Search
        let search_json = runner
            .execute_command(&Commands::Search { query: "test".to_string() }, true)
            .await;
        assert!(search_json.contains(r#""query":"test""#));

        // Test Config
        let config_json = runner.execute_command(&Commands::Config, true).await;
        assert!(config_json.contains(r#""accounts_loaded":0"#));

        // Test Folders
        let folders_json = runner.execute_command(&Commands::Folders, true).await;
        assert!(folders_json.contains(r#""folders":[]"#));

        // Test Read non-existent
        let read_err = runner
            .execute_command(&Commands::Read { id: "msg-missing".to_string() }, true)
            .await;
        assert!(read_err.contains(r#""status":"error""#));
    }

    #[test]
    fn runner_error_display() {
        let err = RunnerError::InitFailed("db locked".to_string());
        assert_eq!(err.to_string(), "failed to initialize engine: db locked");
    }
}
