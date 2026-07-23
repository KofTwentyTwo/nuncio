//! Nuncio CLI main application entry point.

mod args;
mod output;

use args::{Cli, Commands};
use clap::Parser;
use output::format_json;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Sync => {
            if cli.json {
                println!("{}", format_json(&json!({"action": "sync"})));
            } else {
                println!("Triggering account synchronization...");
            }
        }
        Commands::List { folder } => {
            if cli.json {
                println!("{}", format_json(&json!({"action": "list", "folder": folder})));
            } else {
                println!("Listing messages in folder '{}'", folder);
            }
        }
        Commands::Send { to, subject, body: _ } => {
            if cli.json {
                println!("{}", format_json(&json!({"action": "send", "to": to, "subject": subject})));
            } else {
                println!("Sending message to {} ('{}')", to, subject);
            }
        }
        Commands::Search { query } => {
            if cli.json {
                println!("{}", format_json(&json!({"action": "search", "query": query})));
            } else {
                println!("Searching for '{}'", query);
            }
        }
        Commands::Config => {
            if cli.json {
                println!("{}", format_json(&json!({"action": "config"})));
            } else {
                println!("Nuncio CLI Configuration Operational");
            }
        }
    }

    Ok(())
}
