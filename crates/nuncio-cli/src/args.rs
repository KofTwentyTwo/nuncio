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
    /// Display open-source third-party library licenses and acknowledgments.
    Licenses,
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
    /// Filter rule operations (`nuncio filter <verb>`).
    Filter {
        #[command(subcommand)]
        action: FilterSubcommand,
    },
    /// Software update operations (`nuncio update <verb>`).
    Update {
        #[command(subcommand)]
        action: UpdateSubcommand,
    },
    /// Contact & address book operations (`nuncio contact <verb>`).
    Contact {
        #[command(subcommand)]
        action: ContactSubcommand,
    },
    /// Launch centralized local background server daemon (`nuncio daemon`).
    Daemon {
        /// TCP port to bind IPC daemon server to (default: 9422).
        #[arg(long, default_value = "9422")]
        port: u16,
    },
}

/// Contact subcommands (`nuncio contact <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContactSubcommand {
    /// List contacts from local address book.
    List,
    /// Search contacts by name, email, or organization.
    Search {
        /// Search query string.
        #[arg(short, long, help = "Search query string")]
        query: String,
    },
    /// Add a new contact entry to address book.
    Add {
        /// Contact display name.
        #[arg(short, long, help = "Display name")]
        name: String,
        /// Primary email address.
        #[arg(short, long, help = "Primary email address")]
        email: String,
        /// Optional organization.
        #[arg(short, long, help = "Organization")]
        org: Option<String>,
    },
}

/// Software update subcommands (`nuncio update <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateSubcommand {
    /// Check for available software updates on GitHub Releases.
    Check,
    /// Download, verify SHA256 checksum, and apply latest software update.
    Apply,
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
    /// Edit an existing account profile configuration.
    Edit {
        /// Unique account identifier.
        #[arg(short, long, help = "Unique account identifier")]
        id: String,
        /// Updated email address.
        #[arg(short, long, help = "Updated email address")]
        email: Option<String>,
        /// Updated IMAP server hostname.
        #[arg(long, help = "Updated IMAP server hostname")]
        imap_host: Option<String>,
        /// Updated IMAP server port.
        #[arg(long, help = "Updated IMAP server port")]
        imap_port: Option<u16>,
        /// Updated SMTP server hostname.
        #[arg(long, help = "Updated SMTP server hostname")]
        smtp_host: Option<String>,
        /// Updated SMTP server port.
        #[arg(long, help = "Updated SMTP server port")]
        smtp_port: Option<u16>,
    },
    /// Remove a configured account profile.
    Delete {
        /// Unique account identifier.
        #[arg(short, long, help = "Unique account identifier")]
        id: String,
    },
    /// Test TLS connection and credential authentication for a configured account.
    Test {
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

/// Filter subcommands (`nuncio filter <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilterSubcommand {
    /// List all configured NSQL filter rules.
    List,
    /// Create a new NSQL filter rule.
    Create {
        /// Display name for rule.
        #[arg(short, long, help = "Display name for rule")]
        name: String,
        /// NSQL query string.
        #[arg(short, long, help = "NSQL query string")]
        sql: String,
        /// Execution priority (lower = higher priority).
        #[arg(short, long, default_value_t = 0, help = "Rule priority")]
        priority: i32,
    },
    /// Edit an existing filter rule.
    Edit {
        /// Rule ID.
        #[arg(short, long, help = "Rule identifier")]
        id: String,
        /// Updated rule display name.
        #[arg(short, long, help = "Updated rule display name")]
        name: Option<String>,
        /// Updated NSQL query string.
        #[arg(short, long, help = "Updated NSQL query string")]
        sql: Option<String>,
        /// Updated rule priority.
        #[arg(short, long, help = "Updated rule priority")]
        priority: Option<i32>,
    },
    /// Delete a filter rule by ID.
    Delete {
        /// Rule ID.
        #[arg(short, long, help = "Rule identifier")]
        id: String,
    },
    /// Test / dry-run NSQL query against an email.
    Test {
        /// NSQL query string to test.
        #[arg(short, long, help = "NSQL query string")]
        sql: String,
        /// Optional message ID to evaluate.
        #[arg(short, long, help = "Message ID to evaluate")]
        message_id: Option<String>,
    },
    /// Export filter rules to file format (sql or json).
    Export {
        /// Export format (sql or json).
        #[arg(short, long, default_value = "sql", help = "Export format (sql/json)")]
        format: String,
    },
    /// Import filter rules from file.
    Import {
        /// File path containing rules to import.
        #[arg(short, long, help = "File path")]
        file: String,
    },
    /// View filter execution logs.
    Logs {
        /// Max log records to fetch.
        #[arg(short, long, default_value_t = 50, help = "Log limit")]
        limit: usize,
    },
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

    #[test]
    fn parse_pure_noun_verb_update_commands() {
        let cli_check = Cli::parse_from(["nuncio", "update", "check"]);
        assert_eq!(
            cli_check.command,
            Commands::Update {
                action: UpdateSubcommand::Check
            }
        );

        let cli_apply = Cli::parse_from(["nuncio", "update", "apply"]);
        assert_eq!(
            cli_apply.command,
            Commands::Update {
                action: UpdateSubcommand::Apply
            }
        );
    }
}

