//! Command-line argument hierarchy and Clap subcommand parsing.

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

/// Nuncio CLI — Pure Noun + Verb Developer-First Mail & Calendar Automation.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(
    name = "nuncio",
    author,
    version,
    about = "Pure Noun + Verb CLI for mail & calendar operations",
    long_about = "Nuncio CLI provides a structured, scriptable interface using pure <Noun> <Verb> [Flags] syntax."
)]
pub struct Cli {
    /// Output machine-readable JSON payloads to stdout.
    #[arg(
        long,
        global = true,
        help = "Output machine-readable JSON payloads to stdout"
    )]
    pub json: bool,

    /// Target specific account ID.
    #[arg(long, global = true, help = "Target a specific configured account ID")]
    pub account: Option<String>,

    /// Enable verbose log output to stderr.
    #[arg(
        short,
        long,
        global = true,
        help = "Enable detailed verbose execution logs on stderr"
    )]
    pub verbose: bool,

    /// Subcommand resource noun.
    #[command(subcommand)]
    pub command: Commands,
}

/// Available resource Nouns for `nuncio-cli`.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Commands {
    /// Account management operations (`nuncio account <verb>`).
    Account {
        #[command(subcommand)]
        action: AccountSubcommand,
    },
    /// Email operations (`nuncio mail <verb>`).
    Mail {
        #[command(subcommand)]
        action: MailSubcommand,
    },
    /// Mailbox folder operations (`nuncio folder <verb>`).
    Folder {
        #[command(subcommand)]
        action: FolderSubcommand,
    },
    /// Print official Nuncio ASCII splash screen & brand banner.
    Banner,
    /// Calendar event operations (`nuncio cal <verb>`).
    Cal {
        #[command(subcommand)]
        action: CalSubcommand,
    },
    /// System and environment management (`nuncio system <verb>`).
    System {
        #[command(subcommand)]
        action: SystemSubcommand,
    },
    /// Launch centralized local background server daemon (`nuncio daemon`).
    Daemon {
        /// TCP port to bind IPC daemon server to (default: 9422).
        #[arg(long, default_value = "9422")]
        port: u16,
    },
}

/// Account subcommands (`nuncio account <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountSubcommand {
    /// List all configured mail and calendar accounts.
    List,
    /// Add and configure a new mail account securely.
    Add {
        /// Email address or account username.
        #[arg(short, long, help = "Email address or account username")]
        email: String,
        /// IMAP server hostname.
        #[arg(long, default_value = "mail.kof22.com", help = "IMAP server hostname")]
        imap_host: String,
        /// IMAP server port (SSL).
        #[arg(long, default_value_t = 993, help = "IMAP server port (default: 993)")]
        imap_port: u16,
        /// SMTP server hostname.
        #[arg(long, default_value = "mail.kof22.com", help = "SMTP server hostname")]
        smtp_host: String,
        /// SMTP server port (SSL).
        #[arg(long, default_value_t = 465, help = "SMTP server port (default: 465)")]
        smtp_port: u16,
        /// IMAP connection transport mode (implicit_tls, start_tls, plain).
        #[arg(long, default_value = "implicit_tls", help = "IMAP transport mode")]
        imap_mode: String,
        /// SMTP connection transport mode (implicit_tls, start_tls, plain).
        #[arg(long, default_value = "implicit_tls", help = "SMTP transport mode")]
        smtp_mode: String,
    },
    /// Display details for a specific configured account.
    Show {
        /// Unique account identifier.
        #[arg(short, long, help = "Unique account identifier")]
        id: String,
    },
}

/// Mail subcommands (`nuncio mail <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MailSubcommand {
    /// Synchronize local email cache with remote IMAP/JMAP server.
    Sync,
    /// List email messages in a specified folder.
    List {
        /// Mailbox folder name (default: "inbox").
        #[arg(short, long, default_value = "inbox", help = "Mailbox folder name")]
        folder: String,
    },
    /// Read a specific email message by ID.
    Read {
        /// Unique message identifier.
        #[arg(short, long, help = "Unique message identifier")]
        id: String,
    },
    /// Compose and send an email message via SMTP.
    Send {
        /// Recipient email address.
        #[arg(short, long, help = "Recipient email address")]
        to: String,
        /// Message subject line.
        #[arg(short, long, help = "Message subject line")]
        subject: String,
        /// Message body text content.
        #[arg(short, long, help = "Message body text content")]
        body: String,
    },
    /// Full-text search across all cached email messages.
    Search {
        /// Search query string.
        #[arg(short, long, help = "Search query string")]
        query: String,
    },
}

/// Folder subcommands (`nuncio folder <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FolderSubcommand {
    /// List available mailbox folders.
    List,
}

/// Calendar subcommands (`nuncio cal <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CalSubcommand {
    /// List upcoming calendar events.
    List,
    /// Synchronize local calendar cache with remote CalDAV server.
    Sync,
}

/// System subcommands (`nuncio system <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemSubcommand {
    /// Display system, daemon, and event bus status.
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pure_noun_verb_account_commands() {
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
    fn parse_pure_noun_verb_mail_commands() {
        let cli_sync = Cli::parse_from(["nuncio", "--json", "--verbose", "mail", "sync"]);
        assert!(cli_sync.json);
        assert!(cli_sync.verbose);
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
}
