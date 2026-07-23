//! Native Model Context Protocol (MCP) Server for Nuncio.
//! Exposes a 4th presentation shell ("Native LLM Agent UI") allowing AI models
//! full access to email, calendar, search, and storage features.

pub mod resources;
pub mod server;
pub mod tools;

pub use resources::{McpResourceDefinition, McpResourceHandler};
pub use server::McpServer;
pub use tools::{McpToolDefinition, McpToolHandler};
