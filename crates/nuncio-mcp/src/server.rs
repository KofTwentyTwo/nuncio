//! Model Context Protocol (MCP) JSON-RPC 2.0 Stdio Server for Nuncio.

use crate::resources::McpResourceHandler;
use crate::tools::McpToolHandler;
use nuncio_store::db::DatabaseEngine;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// MCP Server running JSON-RPC 2.0 protocol over stdio.
pub struct McpServer {
    tools: McpToolHandler,
    resources: McpResourceHandler,
    _tempdir: Option<Arc<TempDir>>,
}

impl McpServer {
    /// Create a new `McpServer` with ephemeral SQLite database.
    pub async fn ephemeral() -> Result<Self, Box<dyn std::error::Error>> {
        let (db_engine, dir) = DatabaseEngine::connect_ephemeral().await?;
        let db = Arc::new(db_engine);
        Ok(Self {
            tools: McpToolHandler::new(db.clone()),
            resources: McpResourceHandler::new(db),
            _tempdir: Some(Arc::new(dir)),
        })
    }

    /// Create a new `McpServer` wrapping shared `DatabaseEngine`.
    pub fn new(db: Arc<DatabaseEngine>) -> Self {
        Self {
            tools: McpToolHandler::new(db.clone()),
            resources: McpResourceHandler::new(db),
            _tempdir: None,
        }
    }

    /// Process a single incoming JSON-RPC 2.0 request string and produce response.
    pub async fn handle_request_json(&self, raw_json: &str) -> Option<Value> {
        let req: Value = match serde_json::from_str(raw_json) {
            Ok(v) => v,
            Err(_) => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32700, "message": "Parse error" },
                    "id": Value::Null
                }));
            }
        };

        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = match req.get("method").and_then(|m| m.as_str()) {
            Some(m) => m,
            None => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32600, "message": "Invalid Request" },
                    "id": id
                }));
            }
        };

        let params = req.get("params").cloned().unwrap_or(json!({}));

        match method {
            "initialize" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                        "resources": {},
                        "prompts": {}
                    },
                    "serverInfo": {
                        "name": "nuncio-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            })),
            "tools/list" => {
                let tool_list = self.tools.list_tools();
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "tools": tool_list }
                }))
            }
            "tools/call" => {
                let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let tool_args = params.get("arguments").cloned().unwrap_or(json!({}));

                match self.tools.call_tool(tool_name, tool_args).await {
                    Ok(res) => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "content": [{ "type": "text", "text": serde_json::to_string_pretty(&res).unwrap_or_default() }] }
                    })),
                    Err(err) => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32603, "message": err }
                    })),
                }
            }
            "resources/list" => {
                let resource_list = self.resources.list_resources();
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "resources": resource_list }
                }))
            }
            "resources/read" => {
                let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                match self.resources.read_resource(uri).await {
                    Ok(res) => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "contents": [{ "uri": uri, "mimeType": "application/json", "text": serde_json::to_string_pretty(&res).unwrap_or_default() }] }
                    })),
                    Err(err) => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32602, "message": err }
                    })),
                }
            }
            "prompts/list" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "prompts": [
                        {
                            "name": "triage_inbox",
                            "description": "Triage and summarize unread email messages in Nuncio Inbox."
                        },
                        {
                            "name": "prepare_daily_briefing",
                            "description": "Generate a unified daily schedule and email briefing for today."
                        }
                    ]
                }
            })),
            "notifications/initialized" => None,
            _ => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {}", method) }
            })),
        }
    }

    /// Run stdio loop reading JSON-RPC lines from `stdin` and outputting responses to `stdout`.
    pub async fn run_stdio_loop(&self) -> Result<(), Box<dyn std::error::Error>> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();
        let mut stdout = tokio::io::stdout();

        while let Some(line) = reader.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(resp) = self.handle_request_json(&line).await {
                let out_bytes = serde_json::to_vec(&resp)?;
                stdout.write_all(&out_bytes).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }

        Ok(())
    }
}
