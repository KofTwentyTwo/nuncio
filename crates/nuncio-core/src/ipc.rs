//! Local IPC Client-Server Daemon Engine for Nuncio.
//! Provides centralized communication over local socket / named pipe so that
//! CLI, TUI, GUI, and MCP shells all talk to the SAME running `nunciod` background server.

use crate::{CoreCommand, EventBus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// IPC Message payload serialized over wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcMessage {
    /// Request/Response correlation ID.
    pub id: u64,
    /// Method name (e.g. `command`, `get_state`, `subscribe`).
    pub method: String,
    /// Request parameters payload.
    pub params: Value,
}

/// Centralized IPC Daemon Server managing multi-client socket communication.
pub struct IpcDaemonServer {
    event_bus: Arc<EventBus>,
    bind_addr: String,
}

impl IpcDaemonServer {
    /// Create a new `IpcDaemonServer` wrapping shared `EventBus`.
    pub fn new(event_bus: Arc<EventBus>, bind_addr: impl Into<String>) -> Self {
        Self {
            event_bus,
            bind_addr: bind_addr.into(),
        }
    }

    /// Default loopback bind address (`127.0.0.1:9422`).
    pub fn default_addr() -> &'static str {
        "127.0.0.1:9422"
    }

    /// Start listening for incoming connections from CLI, TUI, GUI, and MCP shells.
    pub async fn run_server(&self) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(&self.bind_addr).await?;
        tracing::info!("Nuncio IPC Daemon listening on {}", self.bind_addr);

        loop {
            let (stream, _addr) = listener.accept().await?;
            let event_bus = self.event_bus.clone();
            tokio::spawn(async move {
                let _ = Self::handle_client(stream, event_bus).await;
            });
        }
    }

    /// Process connection for a single connected presentation shell client.
    async fn handle_client(
        stream: TcpStream,
        event_bus: Arc<EventBus>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (reader_half, mut writer_half) = stream.into_split();
        let mut reader = BufReader::new(reader_half).lines();

        while let Some(line) = reader.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            let msg: IpcMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let response = match msg.method.as_str() {
                "sync_all" => {
                    let _ = event_bus.send_command(CoreCommand::SyncAll).await;
                    json!({ "id": msg.id, "result": { "status": "dispatched" } })
                }
                "get_state" => {
                    let state = event_bus.current_state();
                    json!({
                        "id": msg.id,
                        "result": {
                            "status": format!("{:?}", state.status),
                            "accounts_loaded": state.accounts_loaded,
                            "unread_count": state.unread_count,
                            "last_error": state.last_error
                        }
                    })
                }
                "ping" => json!({ "id": msg.id, "result": "pong" }),
                _ => json!({ "id": msg.id, "error": "unknown_method" }),
            };

            let out_bytes = serde_json::to_vec(&response)?;
            writer_half.write_all(&out_bytes).await?;
            writer_half.write_all(b"\n").await?;
            writer_half.flush().await?;
        }

        Ok(())
    }
}

/// IPC Client used by CLI, TUI, GUI, and MCP to connect to running Nuncio daemon.
pub struct IpcClient {
    server_addr: String,
}

impl IpcClient {
    /// Create an `IpcClient` pointing to target server address.
    pub fn new(server_addr: impl Into<String>) -> Self {
        Self {
            server_addr: server_addr.into(),
        }
    }

    /// Connect to running `nunciod` server and execute ping test.
    pub async fn ping(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let mut stream = TcpStream::connect(&self.server_addr).await?;
        let req = json!({ "id": 1, "method": "ping", "params": {} });
        let bytes = serde_json::to_vec(&req)?;
        stream.write_all(&bytes).await?;
        stream.write_all(b"\n").await?;
        stream.flush().await?;

        let mut reader = BufReader::new(stream).lines();
        if let Some(line) = reader.next_line().await? {
            let resp: Value = serde_json::from_str(&line)?;
            return Ok(resp.get("result").and_then(|r| r.as_str()) == Some("pong"));
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ipc_client_server_ping_pong_test() {
        let event_bus = Arc::new(EventBus::new());
        let addr = "127.0.0.1:19422";
        let server = IpcDaemonServer::new(event_bus, addr);

        tokio::spawn(async move {
            let _ = server.run_server().await;
        });

        // Give server 50ms to bind
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = IpcClient::new(addr);
        let ping_result = client.ping().await.expect("ping execution");
        assert!(ping_result);
    }
}
