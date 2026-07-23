//! Centralized Standalone Background Daemon Server Binary (`nunciod`).
//! Owns storage persistence, background sync loops, protocol connections,
//! and multi-client IPC socket distribution.

use nuncio_core::ipc::IpcDaemonServer;
use nuncio_core::EventBus;
use nuncio_store::db::DatabaseEngine;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Nuncio Central Daemon Service (nunciod)...");

    let (db_engine, _dir) = DatabaseEngine::connect_ephemeral().await?;
    let _db = Arc::new(db_engine);
    let event_bus = Arc::new(EventBus::new());

    let addr = std::env::var("NUNCIO_IPC_ADDR").unwrap_or_else(|_| "127.0.0.1:9422".to_string());
    let server = IpcDaemonServer::new(event_bus.clone(), &addr);

    tracing::info!("nunciod listening on {}", addr);
    server.run_server().await?;

    Ok(())
}
