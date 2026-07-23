//! Model Context Protocol (MCP) Resource URI resolvers for Nuncio.

use nuncio_store::db::DatabaseEngine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// MCP Resource Definition exposed to LLM agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceDefinition {
    /// URI scheme identifier (e.g. `nuncio://mail/inbox`).
    pub uri: String,
    /// Human-readable name.
    pub name: String,
    /// Resource description.
    pub description: String,
    /// MIME type of payload (e.g. `application/json`).
    pub mime_type: String,
}

/// Handler resolving MCP Resource URIs.
pub struct McpResourceHandler {
    db: Arc<DatabaseEngine>,
}

impl McpResourceHandler {
    /// Create a new `McpResourceHandler`.
    pub fn new(db: Arc<DatabaseEngine>) -> Self {
        Self { db }
    }

    /// List available MCP resources exposed by Nuncio.
    pub fn list_resources(&self) -> Vec<McpResourceDefinition> {
        vec![
            McpResourceDefinition {
                uri: "nuncio://mail/inbox".to_string(),
                name: "Inbox Mail Messages".to_string(),
                description: "Live inbox emails stored in Nuncio local SQLite database."
                    .to_string(),
                mime_type: "application/json".to_string(),
            },
            McpResourceDefinition {
                uri: "nuncio://accounts".to_string(),
                name: "Registered Accounts".to_string(),
                description: "List of configured mail and calendar accounts.".to_string(),
                mime_type: "application/json".to_string(),
            },
        ]
    }

    /// Read content for a target resource URI.
    pub async fn read_resource(&self, uri: &str) -> Result<Value, String> {
        match uri {
            "nuncio://mail/inbox" => {
                let messages = self
                    .db
                    .list_messages("INBOX", 50)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "uri": uri, "content": messages }))
            }
            "nuncio://accounts" => {
                let accounts = self.db.list_accounts().await.map_err(|e| e.to_string())?;
                Ok(json!({ "uri": uri, "content": accounts }))
            }
            _ => Err(format!("Unknown resource URI: {}", uri)),
        }
    }
}
