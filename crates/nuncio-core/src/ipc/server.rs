//! Local IPC Daemon Server handling multi-client socket communication over `EventBus`.

use crate::ipc::framing::{read_frame, write_frame};
use crate::ipc::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::{CoreCommand, EventBus};
use futures::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

/// Async handler callback signature for custom RPC extensions (e.g. `filter.*` handlers).
pub type CustomRpcHandler = Arc<
    dyn Fn(
            &str,
            serde_json::Value,
        ) -> BoxFuture<'static, Option<Result<serde_json::Value, String>>>
        + Send
        + Sync,
>;

/// Centralized IPC Daemon Server managing presentation shell clients over `EventBus`.
pub struct IpcDaemonServer {
    event_bus: Arc<EventBus>,
    bind_addr: String,
    custom_handler: Option<CustomRpcHandler>,
}

impl IpcDaemonServer {
    /// Default TCP loopback bind address (`127.0.0.1:9422`).
    pub const DEFAULT_ADDR: &'static str = "127.0.0.1:9422";

    /// Create a new `IpcDaemonServer` wrapping shared `EventBus`.
    pub fn new(event_bus: Arc<EventBus>, bind_addr: impl Into<String>) -> Self {
        Self {
            event_bus,
            bind_addr: bind_addr.into(),
            custom_handler: None,
        }
    }

    /// Create a new `IpcDaemonServer` with custom RPC handler extension.
    pub fn with_handler(
        event_bus: Arc<EventBus>,
        bind_addr: impl Into<String>,
        handler: CustomRpcHandler,
    ) -> Self {
        Self {
            event_bus,
            bind_addr: bind_addr.into(),
            custom_handler: Some(handler),
        }
    }

    /// Attach a custom RPC handler callback.
    pub fn set_handler(&mut self, handler: CustomRpcHandler) {
        self.custom_handler = Some(handler);
    }

    /// Return configured bind address.
    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    /// Start listening for client socket connections and processing JSON-RPC requests.
    pub async fn run_server(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = TcpListener::bind(&self.bind_addr).await?;
        tracing::info!("Nuncio IPC Daemon listening on {}", self.bind_addr);

        loop {
            let (stream, _addr) = listener.accept().await?;
            let event_bus = self.event_bus.clone();
            let handler = self.custom_handler.clone();
            tokio::spawn(async move {
                let _ = Self::handle_stream_with_handler(stream, event_bus, handler).await;
            });
        }
    }

    /// Handle stream without custom handler.
    pub async fn handle_stream<S>(
        stream: S,
        event_bus: Arc<EventBus>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::handle_stream_with_handler(stream, event_bus, None).await
    }

    /// Handle bi-directional request processing and event streaming over an async byte stream.
    pub async fn handle_stream_with_handler<S>(
        stream: S,
        event_bus: Arc<EventBus>,
        custom_handler: Option<CustomRpcHandler>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (push_tx, mut push_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

        // Task to write push notifications and RPC responses to stream
        let writer_task = tokio::spawn(async move {
            while let Some(bytes) = push_rx.recv().await {
                if write_frame(&mut writer, &bytes).await.is_err() {
                    break;
                }
            }
        });

        // Event listener broadcasting CoreEvents over push channel
        let mut core_event_rx = event_bus.subscribe_events();
        let push_tx_events = push_tx.clone();
        let event_listener_task = tokio::spawn(async move {
            while let Ok(evt) = core_event_rx.recv().await {
                let notification = JsonRpcNotification::new(
                    "events.notify",
                    json!({ "event": format!("{:?}", evt) }),
                );
                if let Ok(bytes) = serde_json::to_vec(&notification) {
                    if push_tx_events.send(bytes).await.is_err() {
                        break;
                    }
                }
            }
        });

        // Read loop processing RPC requests from client
        loop {
            let frame_bytes = match read_frame(&mut reader).await {
                Ok(bytes) => bytes,
                Err(_) => break,
            };

            let req: JsonRpcRequest = match serde_json::from_slice(&frame_bytes) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let response = match req.method.as_str() {
                "system.ping" => JsonRpcResponse::success(req.id, json!("pong")),
                "system.state" => {
                    let state = event_bus.current_state();
                    JsonRpcResponse::success(
                        req.id,
                        json!({
                            "status": format!("{:?}", state.status),
                            "accounts_loaded": state.accounts_loaded,
                            "unread_count": state.unread_count,
                            "last_error": state.last_error,
                        }),
                    )
                }
                "mail.sync_all" => {
                    event_bus.process_command(CoreCommand::SyncAll);
                    JsonRpcResponse::success(req.id, json!({ "status": "dispatched" }))
                }
                "mail.mark_read" => {
                    if let Some(msg_id) = req.params.get("message_id").and_then(|v| v.as_str()) {
                        let read = req
                            .params
                            .get("read")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        self_mark_read(&event_bus, msg_id, read);
                        JsonRpcResponse::success(req.id, json!({ "status": "marked" }))
                    } else {
                        JsonRpcResponse::error(req.id, -32602, "missing message_id")
                    }
                }
                other_method => {
                    if let Some(handler) = &custom_handler {
                        if let Some(res) = handler(other_method, req.params.clone()).await {
                            match res {
                                Ok(val) => JsonRpcResponse::success(req.id, val),
                                Err(err_msg) => JsonRpcResponse::error(req.id, -32603, err_msg),
                            }
                        } else {
                            JsonRpcResponse::error(req.id, -32601, "Method not found")
                        }
                    } else {
                        JsonRpcResponse::error(req.id, -32601, "Method not found")
                    }
                }
            };

            if let Ok(resp_bytes) = serde_json::to_vec(&response) {
                if push_tx.send(resp_bytes).await.is_err() {
                    break;
                }
            }
        }

        event_listener_task.abort();
        writer_task.abort();

        Ok(())
    }
}

fn self_mark_read(event_bus: &EventBus, message_id: &str, read: bool) {
    event_bus.process_command(CoreCommand::MarkRead {
        message_id: message_id.to_string(),
        read,
    });
}
