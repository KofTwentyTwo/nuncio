//! Model Context Protocol (MCP) Tool definitions for Nuncio.

use nuncio_core::model::{CalendarEvent, Email};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::search::SearchEngine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// Definition of an MCP tool exposed to LLM agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    /// Unique name of the tool (e.g. `nuncio_mail_list`).
    pub name: String,
    /// Detailed description for LLM capability selection.
    pub description: String,
    /// JSON Schema for tool input arguments.
    pub input_schema: Value,
}

/// Handler managing tool registration and execution over `DatabaseEngine`.
pub struct McpToolHandler {
    db: Arc<DatabaseEngine>,
}

impl McpToolHandler {
    /// Create a new `McpToolHandler` wrapping shared `DatabaseEngine`.
    pub fn new(db: Arc<DatabaseEngine>) -> Self {
        Self { db }
    }

    /// List all available MCP tools exposed by Nuncio.
    pub fn list_tools(&self) -> Vec<McpToolDefinition> {
        vec![
            McpToolDefinition {
                name: "nuncio_mail_list".to_string(),
                description: "List email messages from Nuncio local storage with optional folder filtering.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "folder_id": { "type": "string", "description": "Folder ID (e.g. INBOX, Sent)" },
                        "limit": { "type": "integer", "description": "Maximum number of messages to return (default 20)" }
                    }
                }),
            },
            McpToolDefinition {
                name: "nuncio_mail_send".to_string(),
                description: "Send an email message via configured Nuncio SMTP transport.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": { "type": "string", "description": "Account ID to send from" },
                        "recipient": { "type": "string", "description": "Recipient email address" },
                        "subject": { "type": "string", "description": "Email subject line" },
                        "body": { "type": "string", "description": "Plain text body content" }
                    },
                    "required": ["account_id", "recipient", "subject", "body"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_mail_search".to_string(),
                description: "Perform local zero-latency FTS5 full-text search over indexed email bodies and subjects.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query terms (e.g. roadmap, invoice)" }
                    },
                    "required": ["query"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_cal_list_events".to_string(),
                description: "List calendar events stored in Nuncio database.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "calendar_id": { "type": "string", "description": "Calendar ID filter" }
                    }
                }),
            },
            McpToolDefinition {
                name: "nuncio_cal_create_event".to_string(),
                description: "Create a new calendar event in Nuncio database.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": { "type": "string", "description": "Account ID owning the event" },
                        "calendar_id": { "type": "string", "description": "Calendar ID to insert event into" },
                        "summary": { "type": "string", "description": "Event title/summary" },
                        "start_time": { "type": "integer", "description": "Start timestamp (epoch seconds)" },
                        "end_time": { "type": "integer", "description": "End timestamp (epoch seconds)" }
                    },
                    "required": ["account_id", "calendar_id", "summary", "start_time", "end_time"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_account_list".to_string(),
                description: "List configured email and calendar account profiles.".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
        ]
    }

    /// Call an MCP tool by name with parsed JSON arguments.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "nuncio_mail_list" => {
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                let folder_id = args
                    .get("folder_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("INBOX");
                let messages = self
                    .db
                    .list_messages(folder_id, limit)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "messages": messages }))
            }
            "nuncio_mail_send" => {
                let recipient = args
                    .get("recipient")
                    .and_then(|v| v.as_str())
                    .ok_or("missing recipient")?;
                let subject = args
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .ok_or("missing subject")?;
                let body = args
                    .get("body")
                    .and_then(|v| v.as_str())
                    .ok_or("missing body")?;
                let account_id = args
                    .get("account_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing account_id")?;

                let email = Email {
                    id: format!("mcp-outbound-{}", chrono::Utc::now().timestamp_millis()),
                    account_id: account_id.to_string(),
                    folder_id: "Sent".to_string(),
                    subject: subject.to_string(),
                    sender: "mcp-agent@nuncio.mx".to_string(),
                    recipient: recipient.to_string(),
                    received_at: chrono::Utc::now().timestamp(),
                    read: true,
                    body_plain: Some(body.to_string()),
                    body_html: None,
                    attachments: Vec::new(),
                };

                let _ = self.db.save_email(&email).await;
                Ok(json!({ "status": "queued_and_saved", "email_id": email.id }))
            }
            "nuncio_mail_search" => {
                let query = args.get("query").and_then(|v| v.as_str()).ok_or("missing query")?;
                let search_engine = SearchEngine::new(&self.db);
                let _ = search_engine.setup_fts_tables().await;
                let results = search_engine.search_messages(query).await.map_err(|e| e.to_string())?;
                Ok(json!({ "results": results }))
            }
            "nuncio_cal_list_events" => {
                let calendar_id = args.get("calendar_id").and_then(|v| v.as_str()).unwrap_or("work");
                let rows: Vec<(String, String, String, String, i64, i64, Option<String>)> = sqlx::query_as(
                    "SELECT id, account_id, calendar_id, summary, start_time, end_time, location FROM calendar_events WHERE calendar_id = ?"
                )
                .bind(calendar_id)
                .fetch_all(self.db.pool())
                .await
                .map_err(|e| e.to_string())?;
                Ok(json!({ "events": rows }))
            }
            "nuncio_cal_create_event" => {
                let account_id = args
                    .get("account_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("acct-1");
                let calendar_id = args
                    .get("calendar_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing calendar_id")?;
                let summary = args
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .ok_or("missing summary")?;
                let start_time = args
                    .get("start_time")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing start_time")?;
                let end_time = args
                    .get("end_time")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing end_time")?;

                let event = CalendarEvent {
                    id: format!("mcp-evt-{}", chrono::Utc::now().timestamp_millis()),
                    account_id: account_id.to_string(),
                    calendar_id: calendar_id.to_string(),
                    summary: summary.to_string(),
                    start_time,
                    end_time,
                    rrule: None,
                    location: Some("Virtual MCP Room".to_string()),
                };

                let _ = sqlx::query("INSERT INTO calendar_events (id, account_id, calendar_id, summary, start_time, end_time, location) VALUES (?, ?, ?, ?, ?, ?, ?)")
                    .bind(&event.id)
                    .bind(&event.account_id)
                    .bind(&event.calendar_id)
                    .bind(&event.summary)
                    .bind(event.start_time)
                    .bind(event.end_time)
                    .bind(&event.location)
                    .execute(self.db.pool())
                    .await;

                Ok(json!({ "status": "created", "event": event }))
            }
            "nuncio_account_list" => {
                let accounts = self.db.list_accounts().await.map_err(|e| e.to_string())?;
                Ok(json!({ "accounts": accounts }))
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}
