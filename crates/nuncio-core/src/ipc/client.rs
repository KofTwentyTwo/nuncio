//! IPC Client used by presentation shells to connect to running `nunciod` daemon.

use crate::ipc::framing::{read_frame, write_frame};
use crate::ipc::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::CoreCommand;
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Errors emitted by `IpcClient`.
#[derive(Error, Debug)]
pub enum IpcClientError {
    /// Transport I/O failure.
    #[error("I/O transport error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization error.
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    /// Failed to spawn or connect to background `nunciod` daemon after max retries.
    #[error("Failed to connect or spawn daemon after max retries")]
    SpawnFailed,
    /// RPC execution error returned by server.
    #[error("RPC error ({code}): {message}")]
    Rpc {
        /// RPC error code.
        code: i32,
        /// RPC error message.
        message: String,
    },
}

/// Client endpoint managing communication with `IpcDaemonServer`.
#[derive(Clone)]
pub struct IpcClient {
    server_addr: String,
    request_id_counter: std::sync::Arc<AtomicU64>,
}

impl IpcClient {
    /// Create a new `IpcClient` pointing to target server address.
    pub fn new(server_addr: impl Into<String>) -> Self {
        Self {
            server_addr: server_addr.into(),
            request_id_counter: std::sync::Arc::new(AtomicU64::new(1)),
        }
    }

    /// Connect to target socket or transparently spawn background `nunciod` process.
    pub async fn connect_or_spawn(&self) -> Result<TcpStream, IpcClientError> {
        if let Ok(stream) = TcpStream::connect(&self.server_addr).await {
            return Ok(stream);
        }

        let _ = self.spawn_daemon();

        let mut backoff = Duration::from_millis(50);
        for _ in 0..5 {
            tokio::time::sleep(backoff).await;
            if let Ok(stream) = TcpStream::connect(&self.server_addr).await {
                return Ok(stream);
            }
            backoff *= 2;
        }

        Err(IpcClientError::SpawnFailed)
    }

    /// Spawn `nunciod` daemon in background.
    fn spawn_daemon(&self) -> Result<(), IpcClientError> {
        let exe_path = std::env::current_exe()?
            .parent()
            .map(|p| p.join("nuncio-cli"))
            .unwrap_or_else(|| std::path::PathBuf::from("nuncio-cli"));

        let mut cmd = Command::new(exe_path);
        cmd.arg("daemon");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let _ = cmd.spawn();
        Ok(())
    }

    /// Execute a JSON-RPC request against running daemon and return result payload.
    pub async fn call_rpc(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, IpcClientError> {
        let mut stream = self.connect_or_spawn().await?;
        let req_id = self.request_id_counter.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest::new(req_id, method, params);
        let payload = serde_json::to_vec(&req)?;

        write_frame(&mut stream, &payload).await?;
        let resp_bytes = read_frame(&mut stream).await?;
        let resp: JsonRpcResponse = serde_json::from_slice(&resp_bytes)?;

        if let Some(err) = resp.error {
            return Err(IpcClientError::Rpc {
                code: err.code,
                message: err.message,
            });
        }

        Ok(resp.result.unwrap_or(Value::Null))
    }

    /// Execute a `ping` health check against running daemon.
    pub async fn ping(&self) -> Result<bool, IpcClientError> {
        let result = self.call_rpc("system.ping", json!({})).await?;
        Ok(result.as_str() == Some("pong"))
    }

    /// Dispatch a core domain command to running daemon.
    pub async fn send_command(&self, cmd: CoreCommand) -> Result<Value, IpcClientError> {
        match cmd {
            CoreCommand::SyncAll => self.call_rpc("mail.sync_all", json!({})).await,
            CoreCommand::MarkRead { message_id, read } => {
                self.call_rpc("mail.mark_read", json!({ "message_id": message_id, "read": read })).await
            }
            _ => self.call_rpc("mail.sync_all", json!({})).await,
        }
    }

    /// Fetch application state snapshot from daemon.
    pub async fn get_state(&self) -> Result<Value, IpcClientError> {
        self.call_rpc("system.state", json!({})).await
    }

    /// List filter rules from running daemon.
    pub async fn filter_list(&self) -> Result<Value, IpcClientError> {
        self.call_rpc("filter.list", json!({})).await
    }

    /// Create a new filter rule in running daemon.
    pub async fn filter_create(&self, name: &str, nsql: &str, priority: i32) -> Result<Value, IpcClientError> {
        self.call_rpc(
            "filter.create",
            json!({ "name": name, "nsql": nsql, "priority": priority }),
        )
        .await
    }

    /// Edit an existing filter rule in running daemon.
    pub async fn filter_edit(&self, id: &str, name: &str, nsql: &str, priority: i32) -> Result<Value, IpcClientError> {
        self.call_rpc(
            "filter.edit",
            json!({ "id": id, "name": name, "nsql": nsql, "priority": priority }),
        )
        .await
    }

    /// Delete a filter rule in running daemon.
    pub async fn filter_delete(&self, id: &str) -> Result<Value, IpcClientError> {
        self.call_rpc("filter.delete", json!({ "id": id })).await
    }

    /// Dry-run preview evaluation of an email against filter rules.
    pub async fn filter_preview(&self, email: &crate::model::Email) -> Result<Value, IpcClientError> {
        let email_json = serde_json::to_value(email)?;
        self.call_rpc("filter.preview", json!({ "email": email_json })).await
    }

    /// Query filter execution logs from running daemon.
    pub async fn filter_logs(&self, limit: usize) -> Result<Value, IpcClientError> {
        self.call_rpc("filter.logs", json!({ "limit": limit })).await
    }

    /// Subscribe to real-time `events.notify` server push stream.
    pub async fn subscribe_events(&self) -> Result<mpsc::Receiver<JsonRpcNotification>, IpcClientError> {
        let stream = self.connect_or_spawn().await?;
        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            let (mut reader, _writer) = tokio::io::split(stream);
            loop {
                let frame_bytes = match read_frame(&mut reader).await {
                    Ok(bytes) => bytes,
                    Err(_) => break,
                };

                if let Ok(notification) = serde_json::from_slice::<JsonRpcNotification>(&frame_bytes) {
                    if tx.send(notification).await.is_err() {
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }
}
