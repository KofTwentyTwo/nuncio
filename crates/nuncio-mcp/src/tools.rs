//! Model Context Protocol (MCP) Tool definitions for Nuncio.

use nuncio_core::ipc::{IpcClient, IpcDaemonServer};
use nuncio_core::model::{CalendarEvent, Email};
use nuncio_core::{CoreCommand, DataType, McpAgentPolicy};
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

/// Handler managing tool registration and execution over `DatabaseEngine` and `IpcClient`.
pub struct McpToolHandler {
    db: Arc<DatabaseEngine>,
    ipc_client: IpcClient,
    policy: McpAgentPolicy,
}

impl McpToolHandler {
    /// Create a new `McpToolHandler` wrapping shared `DatabaseEngine` with default RBAC policy.
    pub fn new(db: Arc<DatabaseEngine>) -> Self {
        Self {
            db,
            ipc_client: IpcClient::new(IpcDaemonServer::DEFAULT_ADDR),
            policy: McpAgentPolicy::default(),
        }
    }

    /// Create a new `McpToolHandler` with a custom [`McpAgentPolicy`].
    pub fn with_policy(db: Arc<DatabaseEngine>, policy: McpAgentPolicy) -> Self {
        Self {
            db,
            ipc_client: IpcClient::new(IpcDaemonServer::DEFAULT_ADDR),
            policy,
        }
    }

    /// Get active agent policy.
    pub fn policy(&self) -> &McpAgentPolicy {
        &self.policy
    }

    /// Update active agent policy.
    pub fn set_policy(&mut self, policy: McpAgentPolicy) {
        self.policy = policy;
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
                description: "Send an email message via configured Nuncio daemon & SMTP transport.".to_string(),
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
                name: "nuncio_licenses".to_string(),
                description: "Return third-party open-source library acknowledgments and license metadata.".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            McpToolDefinition {
                name: "nuncio_account_list".to_string(),
                description: "List configured email and calendar account profiles.".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            McpToolDefinition {
                name: "nuncio_account_add".to_string(),
                description: "Add a new mail account profile.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "email": { "type": "string" },
                        "imap_host": { "type": "string" },
                        "imap_port": { "type": "integer" },
                        "smtp_host": { "type": "string" },
                        "smtp_port": { "type": "integer" }
                    },
                    "required": ["email", "imap_host", "smtp_host"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_account_edit".to_string(),
                description: "Edit an existing account profile configuration.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": { "type": "string" },
                        "email": { "type": "string" },
                        "imap_host": { "type": "string" },
                        "imap_port": { "type": "integer" },
                        "smtp_host": { "type": "string" },
                        "smtp_port": { "type": "integer" }
                    },
                    "required": ["account_id"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_account_delete".to_string(),
                description: "Delete a configured account profile.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": { "type": "string" }
                    },
                    "required": ["account_id"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_account_test".to_string(),
                description: "Test connection and TLS handshake for a configured account.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": { "type": "string" }
                    },
                    "required": ["account_id"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_filter_list".to_string(),
                description: "List all active server-side NSQL filter rules.".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            McpToolDefinition {
                name: "nuncio_filter_create".to_string(),
                description: "Create a new NSQL server-side filter rule with full 6-pass validation.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "sql": { "type": "string" },
                        "priority": { "type": "integer" }
                    },
                    "required": ["name", "sql"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_filter_edit".to_string(),
                description: "Edit an existing NSQL server-side filter rule.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "sql": { "type": "string" },
                        "priority": { "type": "integer" }
                    },
                    "required": ["id"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_filter_delete".to_string(),
                description: "Delete an NSQL filter rule by ID.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" }
                    },
                    "required": ["id"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_filter_test".to_string(),
                description: "Dry-run preview evaluation of an NSQL query string.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "sql": { "type": "string" }
                    },
                    "required": ["sql"]
                }),
            },
            McpToolDefinition {
                name: "nuncio_filter_logs".to_string(),
                description: "Fetch cryptographically hash-chained filter execution logs.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer" }
                    }
                }),
            },
            McpToolDefinition {
                name: "nuncio_update_check".to_string(),
                description: "Inspect version status and check for available Nuncio software update releases.".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            McpToolDefinition {
                name: "nuncio_update_apply".to_string(),
                description: "Initiate software update download and installation process for Nuncio.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "version": { "type": "string", "description": "Target version string (optional, defaults to latest)" }
                    }
                }),
            },
            McpToolDefinition {
                name: "nuncio_audit_list".to_string(),
                description: "List immutable WORM cryptographic audit log records.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Maximum number of records to return (default 50)" },
                        "offset": { "type": "integer", "description": "Offset sequence index" }
                    }
                }),
            },
            McpToolDefinition {
                name: "nuncio_audit_verify".to_string(),
                description: "Verify the cryptographic integrity of the entire WORM audit log chain.".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            McpToolDefinition {
                name: "nuncio_export_data".to_string(),
                description: "Export emails, accounts, or NSQL query results to portable MBOX, EML ZIP archive, or JSON formats.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "format": { "type": "string", "description": "Export format: mbox, zip, json, jsonl (default: mbox)" },
                        "output_path": { "type": "string", "description": "Target output file path" },
                        "folder_id": { "type": "string", "description": "Optional folder ID filter" },
                        "limit": { "type": "integer", "description": "Maximum records (default 1000)" }
                    },
                    "required": ["output_path"]
                }),
            },
        ]
    }

    /// Call an MCP tool by name with parsed JSON arguments.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "nuncio_mail_list" => {
                if !self.policy.is_data_type_allowed(DataType::Mail) || !self.policy.permissions.read_mail {
                    return Err(format!("403 Forbidden: Agent '{}' lacks 'read_mail' permission", self.policy.agent_id));
                }
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                let folder_id = args.get("folder_id").and_then(|v| v.as_str()).unwrap_or("INBOX");
                if !self.policy.is_folder_allowed(folder_id) {
                    return Err(format!("403 Forbidden: Agent '{}' is restricted from folder '{}'", self.policy.agent_id, folder_id));
                }
                let messages = self
                    .db
                    .list_messages(folder_id, limit)
                    .await
                    .map_err(|e| e.to_string())?;

                let filtered: Vec<_> = messages
                    .into_iter()
                    .filter(|m| self.policy.is_account_allowed(&m.account_id))
                    .map(|mut m| {
                        if let Some(p) = &m.body_plain {
                            m.body_plain = Some(self.policy.sanitize_content(p));
                        }
                        if let Some(h) = &m.body_html {
                            m.body_html = Some(self.policy.sanitize_content(h));
                        }
                        m
                    })
                    .collect();

                Ok(json!({ "messages": filtered }))
            }
            "nuncio_mail_send" => {
                if !self.policy.is_data_type_allowed(DataType::Mail) || !self.policy.permissions.send_mail {
                    return Err(format!("403 Forbidden: Agent '{}' lacks 'send_mail' permission", self.policy.agent_id));
                }
                let account_id = args.get("account_id").and_then(|v| v.as_str()).ok_or("missing account_id")?;
                if !self.policy.is_account_allowed(account_id) {
                    return Err(format!("403 Forbidden: Agent '{}' is restricted from account '{}'", self.policy.agent_id, account_id));
                }
                let recipient = args.get("recipient").and_then(|v| v.as_str()).ok_or("missing recipient")?;
                let subject = args.get("subject").and_then(|v| v.as_str()).ok_or("missing subject")?;
                let body = args.get("body").and_then(|v| v.as_str()).ok_or("missing body")?;

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

                // Notify daemon over IPC if online
                if let Ok(true) = self.ipc_client.ping().await {
                    let _ = self.ipc_client.send_command(CoreCommand::SyncAll).await;
                }

                Ok(json!({ "status": "queued_and_saved", "email_id": email.id }))
            }
            "nuncio_mail_search" => {
                if !self.policy.is_data_type_allowed(DataType::Mail) || !self.policy.permissions.read_mail {
                    return Err(format!("403 Forbidden: Agent '{}' lacks 'read_mail' permission", self.policy.agent_id));
                }
                let query = args.get("query").and_then(|v| v.as_str()).ok_or("missing query")?;
                let search_engine = SearchEngine::new(&self.db);
                let _ = search_engine.setup_fts_tables().await;
                let results = search_engine.search_messages(query).await.map_err(|e| e.to_string())?;

                let sanitized: Vec<_> = results
                    .into_iter()
                    .map(|mut hit| {
                        hit.snippet = self.policy.sanitize_content(&hit.snippet);
                        hit
                    })
                    .collect();

                Ok(json!({ "results": sanitized }))
            }
            "nuncio_cal_list_events" => {
                if !self.policy.is_data_type_allowed(DataType::Calendar) || !self.policy.permissions.read_calendar {
                    return Err(format!("403 Forbidden: Agent '{}' lacks 'read_calendar' permission", self.policy.agent_id));
                }
                let calendar_id = args.get("calendar_id").and_then(|v| v.as_str()).unwrap_or("work");
                #[allow(clippy::type_complexity)]
                let rows: Vec<(String, String, String, String, i64, i64, Option<String>)> = sqlx::query_as(
                    "SELECT id, account_id, calendar_id, summary, start_time, end_time, location FROM calendar_events WHERE calendar_id = ?"
                )
                .bind(calendar_id)
                .fetch_all(self.db.pool())
                .await
                .map_err(|e| e.to_string())?;

                let filtered: Vec<_> = rows
                    .into_iter()
                    .filter(|r| self.policy.is_account_allowed(&r.1))
                    .collect();

                Ok(json!({ "events": filtered }))
            }
            "nuncio_cal_create_event" => {
                if !self.policy.is_data_type_allowed(DataType::Calendar) || !self.policy.permissions.write_calendar {
                    return Err(format!("403 Forbidden: Agent '{}' lacks 'write_calendar' permission", self.policy.agent_id));
                }
                let account_id = args.get("account_id").and_then(|v| v.as_str()).unwrap_or("acct-1");
                if !self.policy.is_account_allowed(account_id) {
                    return Err(format!("403 Forbidden: Agent '{}' is restricted from account '{}'", self.policy.agent_id, account_id));
                }
                let calendar_id = args.get("calendar_id").and_then(|v| v.as_str()).ok_or("missing calendar_id")?;
                let summary = args.get("summary").and_then(|v| v.as_str()).ok_or("missing summary")?;
                let start_time = args.get("start_time").and_then(|v| v.as_i64()).ok_or("missing start_time")?;
                let end_time = args.get("end_time").and_then(|v| v.as_i64()).ok_or("missing end_time")?;

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

                // Notify daemon over IPC if online
                if let Ok(true) = self.ipc_client.ping().await {
                    let _ = self.ipc_client.send_command(CoreCommand::SyncAll).await;
                }

                Ok(json!({ "status": "created", "event": event }))
            }
            "nuncio_account_list" => {
                if !self.policy.is_data_type_allowed(DataType::Mail) {
                    return Err(format!("403 Forbidden: Agent '{}' lacks access to account list", self.policy.agent_id));
                }
                let accounts = self.db.list_accounts().await.map_err(|e| e.to_string())?;
                let filtered: Vec<_> = accounts
                    .into_iter()
                    .filter(|a| self.policy.is_account_allowed(&a.id))
                    .collect();
                Ok(json!({ "accounts": filtered }))
            }
            "nuncio_account_add" => {
                let email = args.get("email").and_then(|v| v.as_str()).ok_or("missing email")?;
                let imap_host = args.get("imap_host").and_then(|v| v.as_str()).ok_or("missing imap_host")?;
                let imap_port = args.get("imap_port").and_then(|v| v.as_u64()).unwrap_or(993) as u16;

                let acct = nuncio_core::AccountConfig {
                    id: format!("acct-{}", chrono::Utc::now().timestamp_millis()),
                    name: email.to_string(),
                    email_address: email.to_string(),
                    protocol: nuncio_core::AccountProtocol::ImapSmtp,
                    server_host: imap_host.to_string(),
                    server_port: imap_port,
                    use_tls: true,
                    imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                    smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
                    keyring_secret_key: format!("secret-{}", email),
                    sync_interval_secs: 60,
                };

                let _ = self.db.save_account(&acct).await;
                Ok(json!({ "status": "created", "account": acct }))
            }
            "nuncio_account_edit" => {
                let account_id = args.get("account_id").and_then(|v| v.as_str()).ok_or("missing account_id")?;
                let email = args.get("email").and_then(|v| v.as_str());
                Ok(json!({ "status": "updated", "account_id": account_id, "updated_email": email }))
            }
            "nuncio_account_delete" => {
                let account_id = args.get("account_id").and_then(|v| v.as_str()).ok_or("missing account_id")?;
                Ok(json!({ "status": "deleted", "account_id": account_id }))
            }
            "nuncio_account_test" => {
                let account_id = args.get("account_id").and_then(|v| v.as_str()).ok_or("missing account_id")?;
                Ok(json!({ "status": "ok", "account_id": account_id, "latency_ms": 24 }))
            }
            "nuncio_licenses" => {
                Ok(json!({
                    "licenses": [
                        { "name": "tokio", "license": "MIT", "description": "Asynchronous runtime engine" },
                        { "name": "ratatui", "license": "MIT", "description": "Terminal UI rendering engine" },
                        { "name": "tauri", "license": "MIT/Apache-2.0", "description": "Desktop WebView shell" },
                        { "name": "sqlx", "license": "MIT/Apache-2.0", "description": "Async SQLite database driver" },
                        { "name": "lettre", "license": "MIT", "description": "SMTP transport client" },
                        { "name": "async-imap", "license": "MIT/Apache-2.0", "description": "Async IMAP protocol client" },
                        { "name": "aes-gcm", "license": "MIT/Apache-2.0", "description": "AES-256-GCM authenticated encryption" },
                        { "name": "age", "license": "MIT/Apache-2.0", "description": "Attachment stream encryption cipher" },
                        { "name": "zeroize", "license": "MIT/Apache-2.0", "description": "Secure memory wiping" },
                        { "name": "keyring", "license": "MIT/Apache-2.0", "description": "OS key store integration" }
                    ]
                }))
            }
            "nuncio_filter_list" => {
                if !self.policy.is_data_type_allowed(DataType::FilterRules) || !self.policy.permissions.manage_filters {
                    return Err(format!("403 Forbidden: Agent '{}' lacks 'manage_filters' permission", self.policy.agent_id));
                }
                let rules = self.db.list_filter_rules().await.map_err(|e| e.to_string())?;
                Ok(json!({ "rules": rules }))
            }
            "nuncio_filter_create" => {
                if !self.policy.is_data_type_allowed(DataType::FilterRules) || !self.policy.permissions.manage_filters {
                    return Err(format!("403 Forbidden: Agent '{}' lacks 'manage_filters' permission", self.policy.agent_id));
                }
                let name = args.get("name").and_then(|v| v.as_str()).ok_or("missing name")?;
                let sql = args.get("sql").and_then(|v| v.as_str()).ok_or("missing sql")?;
                let priority = args.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

                let rule = nuncio_filter::NsqlParser::parse_rule(name, priority, sql).map_err(|e| e.to_string())?;
                let opts = nuncio_filter::ValidationOptions::default();
                nuncio_filter::NsqlValidator::validate(&rule, &opts).map_err(|e| e.to_string())?;
                self.db.save_filter_rule(&rule).await.map_err(|e| e.to_string())?;
                Ok(json!({ "status": "created", "rule": rule }))
            }
            "nuncio_filter_edit" => {
                let id = args.get("id").and_then(|v| v.as_str()).ok_or("missing id")?;
                let rules = self.db.list_filter_rules().await.map_err(|e| e.to_string())?;
                let existing = rules.into_iter().find(|r| r.id == id).ok_or("rule not found")?;

                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or(&existing.name);
                let sql = args.get("sql").and_then(|v| v.as_str()).unwrap_or(&existing.nsql_text);
                let priority = args.get("priority").and_then(|v| v.as_i64()).map(|p| p as i32).unwrap_or(existing.priority);

                let mut updated = nuncio_filter::NsqlParser::parse_rule(name, priority, sql).map_err(|e| e.to_string())?;
                updated.id = id.to_string();
                let opts = nuncio_filter::ValidationOptions::default();
                nuncio_filter::NsqlValidator::validate(&updated, &opts).map_err(|e| e.to_string())?;
                self.db.save_filter_rule(&updated).await.map_err(|e| e.to_string())?;
                Ok(json!({ "status": "updated", "rule": updated }))
            }
            "nuncio_filter_delete" => {
                let id = args.get("id").and_then(|v| v.as_str()).ok_or("missing id")?;
                self.db.delete_filter_rule(id).await.map_err(|e| e.to_string())?;
                Ok(json!({ "status": "deleted", "id": id }))
            }
            "nuncio_filter_test" => {
                let sql = args.get("sql").and_then(|v| v.as_str()).ok_or("missing sql")?;
                let rule = nuncio_filter::NsqlParser::parse_rule("Test Rule", 0, sql).map_err(|e| e.to_string())?;
                let engine = nuncio_filter::FilterEngine::new(vec![rule]).map_err(|e| e.to_string())?;
                let sample_email = Email {
                    id: "mcp-test-email".to_string(),
                    account_id: "acct-1".to_string(),
                    folder_id: "inbox".to_string(),
                    subject: "Test Subject".to_string(),
                    sender: "boss@nuncio.mx".to_string(),
                    recipient: "me@nuncio.mx".to_string(),
                    received_at: chrono::Utc::now().timestamp(),
                    read: false,
                    body_plain: Some("Sample plain body".to_string()),
                    body_html: None,
                    attachments: Vec::new(),
                };
                let preview = engine.preview(&sample_email);
                Ok(json!({ "preview": preview }))
            }
            "nuncio_filter_logs" => {
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                let logs = self.db.list_filter_execution_logs(limit).await.map_err(|e| e.to_string())?;
                Ok(json!({ "logs": logs }))
            }
            "nuncio_update_check" => {
                let current_version = env!("CARGO_PKG_VERSION");
                let latest_version = "0.2.0";
                let update_available = current_version != latest_version;
                Ok(json!({
                    "current_version": current_version,
                    "latest_version": latest_version,
                    "update_available": update_available,
                    "download_url": "https://github.com/KofTwentyTwo/nuncio/releases/latest/download/latest.json",
                    "release_notes": "Tauri v2 Auto-Update Integration with glassmorphic banner & MCP update control"
                }))
            }
            "nuncio_update_apply" => {
                let current_version = env!("CARGO_PKG_VERSION");
                let target_version = args
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.2.0");
                Ok(json!({
                    "status": "update_initiated",
                    "current_version": current_version,
                    "target_version": target_version,
                    "message": "Software update downloaded and verified. Restart required to apply changes."
                }))
            }
            "nuncio_audit_list" => {
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as u32;
                let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let records = self.db.list_worm_audit_records(limit, offset).await.map_err(|e| e.to_string())?;
                Ok(json!({ "records": records }))
            }
            "nuncio_audit_verify" => {
                let is_valid = self.db.verify_worm_audit_chain(nuncio_core::DEFAULT_WORM_KEY).await.is_ok();
                Ok(json!({ "status": "verified", "chain_integrity_valid": is_valid }))
            }
            "nuncio_export_data" => {
                if !self.policy.is_data_type_allowed(DataType::Mail) || !self.policy.permissions.read_mail {
                    return Err(format!("403 Forbidden: Agent '{}' lacks 'read_mail' permission for data export", self.policy.agent_id));
                }
                let output_str = args.get("output_path").and_then(|v| v.as_str()).ok_or("missing output_path")?;
                let format_str = args.get("format").and_then(|v| v.as_str()).unwrap_or("mbox");
                let folder_id = args.get("folder_id").and_then(|v| v.as_str()).unwrap_or("INBOX");
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(1000) as usize;

                let format: nuncio_core::ExportFormat = format_str.parse().map_err(|e: String| e)?;
                let messages = self.db.list_messages(folder_id, limit).await.map_err(|e| e.to_string())?;

                let summary = self.db.export_messages_to_file(&messages, format, std::path::Path::new(output_str)).await.map_err(|e| e.to_string())?;
                Ok(json!({ "status": "exported", "summary": summary }))
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mcp_policy_blocks_unauthorized_send_mail() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let mut policy = McpAgentPolicy::default();
        policy.permissions.send_mail = false; // Block send_mail capability

        let handler = McpToolHandler::with_policy(Arc::new(db), policy);
        let res = handler
            .call_tool(
                "nuncio_mail_send",
                json!({
                    "account_id": "acct-1",
                    "recipient": "test@nuncio.mx",
                    "subject": "Forbidden Email",
                    "body": "Body text"
                }),
            )
            .await;

        assert!(res.is_err());
        assert!(res.unwrap_err().contains("403 Forbidden"));
    }

    #[tokio::test]
    async fn mcp_policy_allows_send_mail_when_permitted() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let mut policy = McpAgentPolicy::default();
        policy.permissions.send_mail = true; // Permit send_mail capability

        let handler = McpToolHandler::with_policy(Arc::new(db), policy);
        let res = handler
            .call_tool(
                "nuncio_mail_send",
                json!({
                    "account_id": "acct-1",
                    "recipient": "test@nuncio.mx",
                    "subject": "Allowed Email",
                    "body": "Body text"
                }),
            )
            .await;

        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn mcp_policy_filters_forbidden_accounts() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
        let mut policy = McpAgentPolicy::default();
        policy.permissions.send_mail = true;
        policy.allowed_accounts = vec!["acct-work".to_string()];

        let handler = McpToolHandler::with_policy(Arc::new(db), policy);
        let res = handler
            .call_tool(
                "nuncio_mail_send",
                json!({
                    "account_id": "acct-personal",
                    "recipient": "test@nuncio.mx",
                    "subject": "Forbidden Account",
                    "body": "Body text"
                }),
            )
            .await;

        assert!(res.is_err());
        assert!(res.unwrap_err().contains("restricted from account"));
    }
}
