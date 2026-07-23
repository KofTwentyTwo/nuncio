//! Binary entrypoint for Nuncio MCP Server.

use nuncio_mcp::McpServer;
use nuncio_store::db::DatabaseEngine;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (db_engine, _dir) = DatabaseEngine::connect_ephemeral().await?;
    let db = Arc::new(db_engine);
    let server = McpServer::new(db);
    server.run_stdio_loop().await?;
    Ok(())
}
