use clap::{Parser, Subcommand};
use nuncio_core::NuncioEngine;

#[derive(Parser)]
#[command(name = "nuncio")]
#[command(author = "James Maes <james@kof22.com>")]
#[command(version = "0.1.0")]
#[command(about = "Cross-platform scriptable mail and calendar CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output JSON formatted response for Unix pipelines
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Mail management subcommands
    Mail {
        #[command(subcommand)]
        action: MailCommands,
    },
    /// Calendar management subcommands
    Cal {
        #[command(subcommand)]
        action: CalCommands,
    },
    /// Trigger background sync across mail and calendar providers
    Sync,
    /// Display system and account status
    Status,
}

#[derive(Subcommand)]
enum MailCommands {
    /// List messages in a folder
    List {
        #[arg(short, long, default_value = "inbox")]
        folder: String,
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    /// Read a specific message by ID
    Read {
        id: String,
    },
    /// Send an outgoing email
    Send {
        #[arg(long)]
        to: String,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        body: String,
    },
}

#[derive(Subcommand)]
enum CalCommands {
    /// List upcoming calendar events
    List {
        #[arg(long)]
        today: bool,
    },
    /// Create a new calendar event
    Add {
        #[arg(long)]
        summary: String,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let engine = NuncioEngine::new();

    match cli.command {
        Commands::Mail { action } => match action {
            MailCommands::List { folder, limit } => {
                if cli.json {
                    println!("{{\"engine\":\"{}\",\"action\":\"mail_list\",\"folder\":\"{}\",\"limit\":{}}}", engine.name, folder, limit);
                } else {
                    println!("Listing {} messages in folder '{}' (limit: {})", engine.name, folder, limit);
                }
            }
            MailCommands::Read { id } => {
                println!("Reading message {}", id);
            }
            MailCommands::Send { to, subject, body: _ } => {
                println!("Sending message to {} with subject '{}'", to, subject);
            }
        },
        Commands::Cal { action } => match action {
            CalCommands::List { today } => {
                println!("Listing calendar events (today: {})", today);
            }
            CalCommands::Add { summary, start, end } => {
                println!("Adding event '{}' from {} to {}", summary, start, end);
            }
        },
        Commands::Sync => {
            println!("Triggering account sync...");
        }
        Commands::Status => {
            println!("{} Engine ({}): Operational", engine.name, engine.domain);
        }
    }

    Ok(())
}
