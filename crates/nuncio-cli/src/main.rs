//! Nuncio CLI main application entry point.

mod args;

use args::{Cli, Commands};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Sync => {
            if cli.json {
                println!(r#"{{"status":"ok","action":"sync"}}"#);
            } else {
                println!("Triggering account synchronization...");
            }
        }
        Commands::List { folder } => {
            if cli.json {
                println!(r#"{{"status":"ok","action":"list","folder":"{}"}}"#, folder);
            } else {
                println!("Listing messages in folder '{}'", folder);
            }
        }
        Commands::Send { to, subject, body: _ } => {
            if cli.json {
                println!(r#"{{"status":"ok","action":"send","to":"{}"}}"#, to);
            } else {
                println!("Sending message to {} ('{}')", to, subject);
            }
        }
        Commands::Search { query } => {
            if cli.json {
                println!(r#"{{"status":"ok","action":"search","query":"{}"}}"#, query);
            } else {
                println!("Searching for '{}'", query);
            }
        }
        Commands::Config => {
            if cli.json {
                println!(r#"{{"status":"ok","action":"config"}}"#);
            } else {
                println!("Nuncio CLI Configuration Operational");
            }
        }
    }

    Ok(())
}
