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
    /// Account operations (`nuncio account <verb>`).
    Account {
        #[command(subcommand)]
        action: AccountSubcommand,
    },
    /// Mail operations (`nuncio mail <verb>`).
    Mail {
        #[command(subcommand)]
        action: MailSubcommand,
    },
    /// Mailbox folder operations (`nuncio folder <verb>`).
    Folder {
        #[command(subcommand)]
        action: FolderSubcommand,
    },
    /// Calendar operations (`nuncio cal <verb>`).
    Cal {
        #[command(subcommand)]
        action: CalSubcommand,
    },
    /// System & Configuration operations (`nuncio system <verb>`).
    System {
        #[command(subcommand)]
        action: SystemSubcommand,
    },

    // Short-cut top-level aliases for backwards compatibility
    /// Shortcut: Trigger mail & calendar synchronization.
    Sync,
    /// Shortcut: List messages in inbox folder.
    List {
        /// Mailbox folder name (default: "inbox").
        #[arg(short, long, default_value = "inbox")]
        folder: String,
    },
    /// Shortcut: Send an email message.
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
    /// Shortcut: Perform full-text search across messages and calendar events.
    Search {
        /// Query string.
        #[arg(short, long)]
        query: String,
    },
    /// Shortcut: List available local mailbox folders.
    Folders,
    /// Shortcut: Read a specific message by ID.
    Read {
        /// Unique message identifier.
        #[arg(short, long)]
        id: String,
    },
    /// Shortcut: Display or validate CLI account configuration.
    Config,
    /// Shortcut: List all configured accounts.
    Accounts,
    /// Shortcut: Add a new mail account.
    AddAccount {
        /// Email address or username.
        #[arg(short, long)]
        email: String,
        /// IMAP server hostname.
        #[arg(long, default_value = "mail.kof22.com")]
        imap_host: String,
        /// IMAP server port (SSL).
        #[arg(long, default_value_t = 993)]
        imap_port: u16,
        /// SMTP server hostname.
        #[arg(long, default_value = "mail.kof22.com")]
        smtp_host: String,
        /// SMTP server port (SSL).
        #[arg(long, default_value_t = 465)]
        smtp_port: u16,
        /// IMAP connection transport mode (implicit_tls, start_tls, plain).
        #[arg(long, default_value = "implicit_tls")]
        imap_mode: String,
        /// SMTP connection transport mode (implicit_tls, start_tls, plain).
        #[arg(long, default_value = "implicit_tls")]
        smtp_mode: String,
    },
}

/// Account subcommands (`nuncio account <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountSubcommand {
    /// List all configured accounts (`nuncio account list`).
    List,
    /// Add a new mail account (`nuncio account add`).
    Add {
        /// Email address or username.
        #[arg(short, long)]
        email: String,
        /// IMAP server hostname.
        #[arg(long, default_value = "mail.kof22.com")]
        imap_host: String,
        /// IMAP server port (SSL).
        #[arg(long, default_value_t = 993)]
        imap_port: u16,
        /// SMTP server hostname.
        #[arg(long, default_value = "mail.kof22.com")]
        smtp_host: String,
        /// SMTP server port (SSL).
        #[arg(long, default_value_t = 465)]
        smtp_port: u16,
        /// IMAP connection transport mode.
        #[arg(long, default_value = "implicit_tls")]
        imap_mode: String,
        /// SMTP connection transport mode.
        #[arg(long, default_value = "implicit_tls")]
        smtp_mode: String,
    },
    /// Show account details (`nuncio account show`).
    Show {
        /// Unique account identifier.
        #[arg(short, long)]
        id: String,
    },
}

/// Mail subcommands (`nuncio mail <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MailSubcommand {
    /// Trigger mail synchronization (`nuncio mail sync`).
    Sync,
    /// List emails in a folder (`nuncio mail list`).
    List {
        /// Mailbox folder name (default: "inbox").
        #[arg(short, long, default_value = "inbox")]
        folder: String,
    },
    /// Read a specific email message (`nuncio mail read`).
    Read {
        /// Unique message identifier.
        #[arg(short, long)]
        id: String,
    },
    /// Send an email message (`nuncio mail send`).
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
    /// Search emails (`nuncio mail search`).
    Search {
        /// Query string.
        #[arg(short, long)]
        query: String,
    },
}

/// Folder subcommands (`nuncio folder <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FolderSubcommand {
    /// List mailbox folders (`nuncio folder list`).
    List,
}

/// Calendar subcommands (`nuncio cal <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CalSubcommand {
    /// List calendar events (`nuncio cal list`).
    List,
    /// Sync calendar events (`nuncio cal sync`).
    Sync,
}

/// System subcommands (`nuncio system <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemSubcommand {
    /// Show system and daemon status (`nuncio system status`).
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_noun_verb_account_commands() {
        let cli_list = Cli::parse_from(["nuncio", "account", "list"]);
        assert_eq!(
            cli_list.command,
            Commands::Account {
                action: AccountSubcommand::List
            }
        );

        let cli_add = Cli::parse_from([
            "nuncio",
            "account",
            "add",
            "--email",
            "james.maes@kof22.com",
            "--imap-host",
            "mail.kof22.com",
        ]);
        assert_eq!(
            cli_add.command,
            Commands::Account {
                action: AccountSubcommand::Add {
                    email: "james.maes@kof22.com".to_string(),
                    imap_host: "mail.kof22.com".to_string(),
                    imap_port: 993,
                    smtp_host: "mail.kof22.com".to_string(),
                    smtp_port: 465,
                    imap_mode: "implicit_tls".to_string(),
                    smtp_mode: "implicit_tls".to_string(),
                }
            }
        );
    }

    #[test]
    fn parse_noun_verb_mail_commands() {
        let cli_sync = Cli::parse_from(["nuncio", "mail", "sync"]);
        assert_eq!(
            cli_sync.command,
            Commands::Mail {
                action: MailSubcommand::Sync
            }
        );

        let cli_read = Cli::parse_from(["nuncio", "mail", "read", "--id", "msg-123"]);
        assert_eq!(
            cli_read.command,
            Commands::Mail {
                action: MailSubcommand::Read {
                    id: "msg-123".to_string()
                }
            }
        );
    }

    #[test]
    fn parse_legacy_shortcut_commands() {
        let cli_sync = Cli::parse_from(["nuncio", "--json", "sync"]);
        assert!(cli_sync.json);
        assert_eq!(cli_sync.command, Commands::Sync);

        let cli_accounts = Cli::parse_from(["nuncio", "accounts"]);
        assert_eq!(cli_accounts.command, Commands::Accounts);
    }
}
