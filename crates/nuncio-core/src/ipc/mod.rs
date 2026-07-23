//! Local IPC Client-Server Daemon Engine for Nuncio.
//! Provides centralized JSON-RPC 2.0 length-prefixed communication over local sockets so that
//! CLI, TUI, GUI, and MCP shells all talk to the SAME running `nunciod` background server.

pub mod client;
pub mod framing;
pub mod protocol;
pub mod server;

pub use client::{IpcClient, IpcClientError};
pub use framing::{read_frame, write_frame, MAX_FRAME_SIZE};
pub use protocol::{JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
pub use server::IpcDaemonServer;
