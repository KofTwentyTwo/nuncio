//! Command-line argument hierarchy and Clap subcommand parsing.

use clap::{Parser, Subcommand};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Carries an account password credential from an interactive `rpassword`
/// prompt (see `nuncio-cli`'s `main`) through to `Accounts/AddAccount`
/// without ever being reachable as a Clap CLI flag, printed by `Debug`, or
/// carried by value through `Serialize`/`Deserialize`.
///
/// `AccountSubcommand::Add`'s `password` field is `#[arg(skip)]`, so Clap
/// never parses it from argv; `main` populates it exactly once, right after
/// `Cli::parse()`, by prompting without echo. `Debug`, `Serialize` are
/// hand-implemented below to redact the value on principle -- defense in
/// depth in case a future caller logs or serializes a `Commands` value --
/// and `Deserialize` always yields an empty password, since a password
/// must never round-trip through a serialized command payload.
#[derive(Clone, Default)]
pub struct PasswordArg(pub String);

impl std::fmt::Debug for PasswordArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PasswordArg(\"[REDACTED]\")")
    }
}

impl PartialEq for PasswordArg {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for PasswordArg {}

impl Serialize for PasswordArg {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[REDACTED]")
    }
}

impl<'de> Deserialize<'de> for PasswordArg {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Discard whatever was serialized (see the redacting `Serialize`
        // impl above) and always yield an empty password rather than ever
        // reconstructing a real credential from a deserialized payload.
        let _ = String::deserialize(deserializer);
        Ok(PasswordArg(String::new()))
    }
}

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
}

/// Contact subcommands (`nuncio contact <verb>`).
///
/// Every verb is scoped to an `--account`, mirroring `CalSubcommand`: the
/// daemon's real contact store (`DatabaseEngine::list_contacts`/
/// `get_contact`/`save_contact`) is itself account-scoped, so there is no
/// honest way to list/search/add a contact without naming which account's
/// address book it belongs to.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContactSubcommand {
    /// List contacts in an account's address book.
    List {
        /// Account identifier owning the address book.
        #[arg(short, long, help = "Account identifier")]
        account: String,
    },
    /// Search contacts by name, email, or organization.
    ///
    /// There is no server-side search RPC in the `Contacts` gRPC surface;
    /// this filters the account's full `ListContacts` result client-side.
    Search {
        /// Account identifier owning the address book.
        #[arg(short, long, help = "Account identifier")]
        account: String,
        /// Search query string.
        #[arg(short, long, help = "Search query string")]
        query: String,
    },
    /// Add a new contact entry to an account's address book.
    Add {
        /// Account identifier owning the address book.
        #[arg(short, long, help = "Account identifier")]
        account: String,
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
    /// Synchronize an account's local contact cache with its remote CardDAV
    /// server.
    Sync {
        /// Account identifier owning the address book.
        #[arg(short, long, help = "Account identifier")]
        account: String,
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
        /// Account password credential. Never a CLI flag -- always read
        /// interactively without echo (via `rpassword`) in `main` and
        /// injected here immediately before dispatch to the daemon's
        /// `Accounts/AddAccount` RPC. See [`PasswordArg`].
        #[arg(skip)]
        password: PasswordArg,
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
    /// Mark a message read or unread.
    Mark {
        /// Unique message identifier.
        #[arg(short, long, help = "Unique message identifier")]
        id: String,
        /// Mark the message as read.
        #[arg(long, help = "Mark the message as read", conflicts_with = "unread")]
        read: bool,
        /// Mark the message as unread.
        #[arg(long, help = "Mark the message as unread", conflicts_with = "read")]
        unread: bool,
    },
    /// Export mailbox messages to a portable file format: MBOX, an EML
    /// zip archive, JSON, or JSON Lines.
    Export {
        /// Export format (mbox, eml, json, or jsonl).
        #[arg(
            short = 'f',
            long,
            default_value = "mbox",
            help = "Export format (mbox, eml, json, jsonl)"
        )]
        format: String,
        /// Destination output file path, on the daemon's host filesystem.
        #[arg(short = 'o', long, help = "Destination output file path")]
        out: String,
        /// Restrict the export to a single account's messages.
        #[arg(
            long,
            help = "Restrict export to a single account ID",
            conflicts_with = "folder"
        )]
        account: Option<String>,
        /// Restrict the export to a single folder's messages.
        #[arg(
            long,
            help = "Restrict export to a single folder ID",
            conflicts_with = "account"
        )]
        folder: Option<String>,
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
///
/// `account`/`calendar` currently have no persisted CalDAV account/collection
/// concept to look up defaults from (see `nunciod::calendar_sync`'s doc
/// comment), so both are plain flags rather than resolved from configured
/// account state; `start`/`end` default to a window spanning "everything"
/// (`0` .. `i64::MAX` unix seconds) so `list`/`sync` are usable with no flags
/// at all once an account/calendar id is supplied.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CalSubcommand {
    /// List calendar events for an account within an optional time window.
    List {
        /// Account identifier owning the calendar.
        #[arg(short, long, help = "Account identifier")]
        account: String,
        /// Calendar collection identifier.
        #[arg(
            short,
            long,
            default_value = "default",
            help = "Calendar collection identifier"
        )]
        calendar: String,
        /// Window start (unix seconds, inclusive).
        #[arg(long, default_value_t = 0, help = "Window start (unix seconds)")]
        start: i64,
        /// Window end (unix seconds, inclusive).
        #[arg(long, default_value_t = i64::MAX, help = "Window end (unix seconds)")]
        end: i64,
    },
    /// Synchronize local calendar cache with remote CalDAV server.
    Sync {
        /// Account identifier owning the calendar.
        #[arg(short, long, help = "Account identifier")]
        account: String,
        /// Calendar collection identifier.
        #[arg(
            short,
            long,
            default_value = "default",
            help = "Calendar collection identifier"
        )]
        calendar: String,
        /// Window start (unix seconds, inclusive).
        #[arg(long, default_value_t = 0, help = "Window start (unix seconds)")]
        start: i64,
        /// Window end (unix seconds, inclusive).
        #[arg(long, default_value_t = i64::MAX, help = "Window end (unix seconds)")]
        end: i64,
    },
}

/// System subcommands (`nuncio system <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemSubcommand {
    /// Display system, daemon, and event bus status.
    Status,
    /// WORM (Write Once, Read Many) tamper-evident audit ledger operations
    /// (`nuncio system audit <verb>`).
    Audit {
        #[command(subcommand)]
        action: AuditSubcommand,
    },
}

/// Audit subcommands (`nuncio system audit <verb>`).
#[derive(Subcommand, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditSubcommand {
    /// List persisted WORM audit ledger records, sequence ascending.
    List {
        /// Max records to fetch (0 = server default).
        #[arg(short, long, default_value_t = 50, help = "Max records to fetch")]
        limit: u32,
        /// Pagination offset.
        #[arg(short, long, default_value_t = 0, help = "Pagination offset")]
        offset: u32,
    },
    /// Verify the WORM audit ledger's cryptographic HMAC hash-chain
    /// integrity.
    Verify,
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
    /// Validate an NSQL query's syntax and semantics without persisting it.
    Validate {
        /// NSQL query string to validate.
        #[arg(short, long, help = "NSQL query string")]
        sql: String,
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
    /// Retroactively apply the current rule set to every stored message,
    /// streaming cumulative progress as the scan runs.
    Triage {
        /// Restrict the scan's reporting to a single rule ID. Not yet
        /// honored by the daemon (see `nuncio.v1.Filters.Triage`) -- the
        /// scan always evaluates every active rule.
        #[arg(short, long, help = "Rule identifier (not yet honored)")]
        rule_id: Option<String>,
        /// Messages fetched per streamed progress update.
        #[arg(short, long, help = "Messages scanned per chunk")]
        chunk_size: Option<u32>,
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
                    password: PasswordArg::default(),
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

        let cli_mark_read =
            Cli::parse_from(["nuncio", "mail", "mark", "--id", "msg-123", "--read"]);
        assert_eq!(
            cli_mark_read.command,
            Commands::Mail {
                action: MailSubcommand::Mark {
                    id: "msg-123".to_string(),
                    read: true,
                    unread: false,
                }
            }
        );

        let cli_mark_unread =
            Cli::parse_from(["nuncio", "mail", "mark", "--id", "msg-123", "--unread"]);
        assert_eq!(
            cli_mark_unread.command,
            Commands::Mail {
                action: MailSubcommand::Mark {
                    id: "msg-123".to_string(),
                    read: false,
                    unread: true,
                }
            }
        );

        // `--read` and `--unread` are mutually exclusive at the Clap level.
        let conflict = Cli::try_parse_from([
            "nuncio", "mail", "mark", "--id", "msg-123", "--read", "--unread",
        ]);
        assert!(conflict.is_err());
    }

    #[test]
    fn parse_pure_noun_verb_mail_export_command() {
        let cli_export = Cli::parse_from([
            "nuncio",
            "mail",
            "export",
            "--format",
            "json",
            "--out",
            "/tmp/out.json",
        ]);
        assert_eq!(
            cli_export.command,
            Commands::Mail {
                action: MailSubcommand::Export {
                    format: "json".to_string(),
                    out: "/tmp/out.json".to_string(),
                    account: None,
                    folder: None,
                }
            }
        );

        let cli_export_scoped = Cli::parse_from([
            "nuncio",
            "mail",
            "export",
            "--format",
            "jsonl",
            "--out",
            "/tmp/scoped.jsonl",
            "--account",
            "acct-1",
        ]);
        assert_eq!(
            cli_export_scoped.command,
            Commands::Mail {
                action: MailSubcommand::Export {
                    format: "jsonl".to_string(),
                    out: "/tmp/scoped.jsonl".to_string(),
                    account: Some("acct-1".to_string()),
                    folder: None,
                }
            }
        );

        // `--account` and `--folder` are mutually exclusive at the Clap
        // level -- an export scope is either "one account", "one folder",
        // or (neither flag) "everything".
        let conflict = Cli::try_parse_from([
            "nuncio",
            "mail",
            "export",
            "--out",
            "/tmp/out.json",
            "--account",
            "acct-1",
            "--folder",
            "inbox",
        ]);
        assert!(conflict.is_err());
    }

    #[test]
    fn parse_pure_noun_verb_system_audit_commands() {
        let cli_list = Cli::parse_from(["nuncio", "system", "audit", "list"]);
        assert_eq!(
            cli_list.command,
            Commands::System {
                action: SystemSubcommand::Audit {
                    action: AuditSubcommand::List {
                        limit: 50,
                        offset: 0,
                    }
                }
            }
        );

        let cli_list_paged = Cli::parse_from([
            "nuncio", "system", "audit", "list", "--limit", "10", "--offset", "5",
        ]);
        assert_eq!(
            cli_list_paged.command,
            Commands::System {
                action: SystemSubcommand::Audit {
                    action: AuditSubcommand::List {
                        limit: 10,
                        offset: 5,
                    }
                }
            }
        );

        let cli_verify = Cli::parse_from(["nuncio", "system", "audit", "verify"]);
        assert_eq!(
            cli_verify.command,
            Commands::System {
                action: SystemSubcommand::Audit {
                    action: AuditSubcommand::Verify
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

    #[test]
    fn account_add_password_is_never_parsed_from_argv() {
        // `--password` is deliberately not a recognized flag: the password
        // MUST always come from an interactive, non-echoing prompt in
        // `main`, never from argv (which would land it in shell history and
        // process listings).
        let cli_add = Cli::parse_from([
            "nuncio",
            "account",
            "add",
            "--email",
            "a@b.com",
            "--imap-host",
            "imap.b.com",
        ]);
        match cli_add.command {
            Commands::Account {
                action: AccountSubcommand::Add { password, .. },
            } => assert_eq!(password, PasswordArg::default()),
            other => panic!("expected Account::Add, got {other:?}"),
        }
    }

    #[test]
    fn password_arg_debug_and_serialize_always_redact_the_value() {
        let secret = PasswordArg("super-secret-value".to_string());
        assert_eq!(format!("{secret:?}"), "PasswordArg(\"[REDACTED]\")");

        let json = serde_json::to_string(&secret).expect("serializes");
        assert_eq!(json, "\"[REDACTED]\"");
        assert!(!json.contains("super-secret-value"));
    }

    #[test]
    fn password_arg_deserialize_never_reconstructs_a_real_credential() {
        let deserialized: PasswordArg =
            serde_json::from_str("\"super-secret-value\"").expect("deserializes");
        assert_eq!(deserialized, PasswordArg::default());
        assert_eq!(deserialized.0, "");
    }

    #[test]
    fn password_arg_equality_and_default() {
        assert_eq!(PasswordArg::default(), PasswordArg(String::new()));
        assert_ne!(PasswordArg("a".to_string()), PasswordArg("b".to_string()));
        let cloned = PasswordArg("clone-me".to_string()).clone();
        assert_eq!(cloned.0, "clone-me");
    }
}
