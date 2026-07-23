//! Command-line argument hierarchy and Clap subcommand parsing.

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

/// Nuncio CLI — Developer-first cross-platform mail & calendar automation.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(name = "nuncio", author, version, about, long_about = None)]
pub struct Cli {
    /// Output machine-readable JSON payloads to stdout.
    #[arg(long, global = true)]
    pub json: bool,

    /// Target specific account ID.
    #[arg(long, global = true)]
    pub account: Option<String>,

    /// Enable verbose log output to stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Subcommand action to perform.
    #[command(subcommand)]
    pub command: Commands,
}

/// Available subcommands for `nuncio-cli`.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Commands {
    /// Trigger mail & calendar synchronization.
    Sync,
    /// List messages in a folder.
    List {
        /// Mailbox folder name (default: "inbox").
        #[arg(short, long, default_value = "inbox")]
        folder: String,
    },
    /// Send an email message.
    Send {
        /// Recipient email address.
        #[arg(short, long)]
        to: String,
        /// Message subject line.
        #[arg(short, long)]
        subject: String,
        /// Message body content.
        #[arg(short, long)]
        body: String,
    },
    /// Perform full-text search across messages and calendar events.
    Search {
        /// Query string.
        #[arg(short, long)]
        query: String,
    },
    /// List available local mailbox folders.
    Folders,
    /// Read a specific message by ID.
    Read {
        /// Unique message identifier.
        #[arg(short, long)]
        id: String,
    },
    /// Display or validate CLI account configuration.
    Config,

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sync_command_flags() {
        let cli = Cli::parse_from(["nuncio", "--json", "sync"]);
        assert!(cli.json);
        assert_eq!(cli.command, Commands::Sync);
    }

    #[test]
    fn parse_list_command_with_folder() {
        let cli = Cli::parse_from(["nuncio", "--account", "acct-1", "list", "--folder", "Sent"]);
        assert_eq!(cli.account, Some("acct-1".to_string()));
        assert_eq!(
            cli.command,
            Commands::List {
                folder: "Sent".to_string()
            }
        );
    }

    #[test]
    fn parse_send_command() {
        let cli = Cli::parse_from([
            "nuncio",
            "send",
            "--to",
            "bob@nuncio.mx",
            "--subject",
            "Meeting",
            "--body",
            "Hello Bob",
        ]);
        assert_eq!(
            cli.command,
            Commands::Send {
                to: "bob@nuncio.mx".to_string(),
                subject: "Meeting".to_string(),
                body: "Hello Bob".to_string(),
            }
        );
    }

    #[test]
    fn parse_search_command() {
        let cli = Cli::parse_from(["nuncio", "search", "--query", "Architecture"]);
        assert_eq!(
            cli.command,
            Commands::Search {
                query: "Architecture".to_string()
            }
        );
    }

    #[test]
    fn parse_config_command() {
        let cli = Cli::parse_from(["nuncio", "config"]);
        assert_eq!(cli.command, Commands::Config);
    }

    #[test]
    fn parse_folders_and_read_commands() {
        let cli_folders = Cli::parse_from(["nuncio", "folders"]);
        assert_eq!(cli_folders.command, Commands::Folders);

        let cli_read = Cli::parse_from(["nuncio", "read", "--id", "msg-123"]);
        assert_eq!(
            cli_read.command,
            Commands::Read {
                id: "msg-123".to_string()
            }
        );
    }
}
