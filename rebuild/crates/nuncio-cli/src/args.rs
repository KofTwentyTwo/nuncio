use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Reference client for the local Nuncio engine")]
pub struct Args {
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true, default_value = "http://127.0.0.1:9421")]
    pub endpoint: String,
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
    #[arg(long, global = true, default_value = "default")]
    pub profile: String,
    #[cfg(feature = "test-harness")]
    #[arg(long, global = true)]
    pub test_secrets_file: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Rebuild a provider projection while preserving drafts and operation history.
    Repair(RepairArgs),
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    #[command(name = "operation", visible_alias = "operations")]
    Operations {
        #[command(subcommand)]
        command: OperationsCommand,
    },
    Calendar {
        #[command(subcommand)]
        command: CalendarCommand,
    },
    Sync {
        #[arg(long)]
        account: String,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        wait: bool,
    },
    Mail {
        #[command(subcommand)]
        command: MailCommand,
    },
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    System {
        #[command(subcommand)]
        command: SystemCommand,
    },
}

#[derive(clap::Args)]
pub struct RepairArgs {
    #[arg(long)]
    pub account: String,
    #[arg(long, value_enum)]
    pub scope: RepairScope,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long, conflicts_with = "dry_run")]
    pub wait: bool,
    #[arg(long, requires = "to")]
    pub from: Option<String>,
    #[arg(long, requires = "from")]
    pub to: Option<String>,
}
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum RepairScope {
    Mail,
    Calendar,
}

#[derive(Subcommand)]
pub enum BackupCommand {
    /// Create an encrypted snapshot; read recovery-passphrase JSON from protected stdin.
    Create {
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect a backup; read recovery-passphrase JSON from protected stdin.
    Inspect {
        #[arg(long)]
        file: PathBuf,
    },
    /// Restore into a new sibling profile; all pending writes remain held.
    Restore {
        #[arg(long)]
        file: PathBuf,
        #[arg(long,value_parser=parse_backup_profile)]
        new_profile: String,
    },
}
fn parse_backup_profile(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("Profile names use 1–64 ASCII letters, digits, hyphens or underscores".into());
    }
    Ok(value.into())
}

#[derive(Subcommand)]
pub enum CalendarCommand {
    /// Explicit online availability query; reports per-calendar coverage and errors.
    FreeBusy {
        #[arg(long)]
        account: String,
        /// Version1 query JSON with RFC3339 bounds, time zone, and provider calendar IDs.
        #[arg(long)]
        file: PathBuf,
    },
    Change {
        #[arg(long)]
        account: String,
        #[arg(long)]
        calendar: String,
        #[arg(long)]
        request_id: String,
        /// Version1 Calendar action JSON with explicit scope and notification policy.
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        wait: bool,
    },
    List {
        #[arg(long)]
        account: String,
        #[arg(long, default_value_t = 100)]
        page_size: u32,
        #[arg(long)]
        page_token: Option<String>,
    },
    Get {
        #[arg(long)]
        account: String,
        #[arg(long)]
        calendar: String,
        #[arg(long)]
        event: String,
    },
    Agenda {
        #[arg(long)]
        account: String,
        #[arg(long)]
        from: String,
        /// Exclusive upper date, interpreted in each calendar's time zone.
        #[arg(long)]
        to: String,
        #[arg(long, default_value_t = 100)]
        page_size: u32,
        #[arg(long)]
        page_token: Option<String>,
    },
    Refresh {
        #[arg(long)]
        account: String,
        #[arg(long, requires = "to")]
        from: Option<String>,
        #[arg(long, requires = "from")]
        to: Option<String>,
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Subcommand)]
pub enum AccountCommand {
    /// Authenticate IMAP and SMTP with required TLS; passwords arrive only on stdin.
    ConnectImap {
        /// Version1 public configuration JSON; no passwords.
        #[arg(long)]
        config: PathBuf,
        /// Read a bounded credential JSON object from a pipe (terminal input is refused).
        #[arg(long, required = true)]
        credentials_stdin: bool,
        /// Reconnect this exact saved IMAP endpoint and username.
        #[arg(long)]
        account: Option<String>,
    },
    /// Read saved public configuration and the last authenticated capability snapshot.
    ImapConfig {
        #[arg(long)]
        account: String,
    },
    ConnectGoogle {
        /// Downloaded Desktop OAuth registration JSON, readable only by its owner.
        #[arg(long)]
        client_config: PathBuf,
        #[arg(long)]
        login_hint: Option<String>,
        /// Reconnect this saved identity; choosing a different Google user fails.
        #[arg(long)]
        account: Option<String>,
        /// Return the browser URL without opening it automatically.
        #[arg(long)]
        no_browser: bool,
    },
    AuthStatus {
        #[arg(long)]
        session: String,
    },
    List,
    Disconnect {
        #[arg(long)]
        account: String,
    },
    /// Explicit online credential/identity check. Account list reads local state.
    Check {
        #[arg(long)]
        account: String,
    },
}

#[derive(Subcommand)]
pub enum SystemCommand {
    /// Committed changes as JSONL; save the last revision to resume later.
    Watch {
        #[arg(long)]
        after: u64,
        /// Stop successfully after this many changes; otherwise watch until interrupted.
        #[arg(long,value_parser=clap::value_parser!(u32).range(1..))]
        limit: Option<u32>,
    },
    Status,
    Shutdown,
    SyncStatus {
        #[arg(long)]
        account: String,
        #[arg(long)]
        run: String,
    },
    CancelSync {
        #[arg(long)]
        account: String,
        #[arg(long)]
        run: String,
    },
}
#[derive(clap::Args)]
pub struct MailQueryArgs {
    #[arg(long)]
    pub account: String,
    #[arg(long)]
    pub collection: Option<String>,
    #[arg(long, default_value_t = 100)]
    pub page_size: u32,
    #[arg(long)]
    pub page_token: Option<String>,
}
#[derive(Subcommand)]
pub enum MailCommand {
    /// Report supported actions and provider placement semantics.
    Capabilities {
        #[arg(long)]
        account: String,
    },
    /// Durably request an explicit read/star/archive/trash/label state from a JSON file.
    Change {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        wait: bool,
    },
    /// Durably enqueue an immutable submission; inspect its operation for delivery.
    Send {
        #[arg(long)]
        account: String,
        #[arg(long)]
        draft: String,
        #[arg(long)]
        request_id: String,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: Option<u64>,
        #[arg(long)]
        wait: bool,
    },
    /// Create a local reply draft; sending is a separate explicit command.
    Reply {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        body_file: PathBuf,
    },
    /// Reply to all visible recipients, excluding this account's address.
    ReplyAll {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        body_file: PathBuf,
    },
    /// Create a local forward with original bodies and attachments.
    Forward {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
        #[arg(long, required = true)]
        to: Vec<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
    },
    Draft {
        #[command(subcommand)]
        command: DraftCommand,
    },
    List {
        #[command(flatten)]
        args: MailQueryArgs,
    },
    Search {
        #[command(flatten)]
        args: MailQueryArgs,
        #[arg(long)]
        query: String,
    },
    Collections {
        #[arg(long)]
        account: String,
    },
    Read {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
    },
    Fetch {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        wait: bool,
    },
    Raw {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        output: PathBuf,
    },
    Attachment {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
        #[arg(long)]
        attachment: String,
        #[arg(long)]
        output: PathBuf,
    },
    Body {
        #[arg(long)]
        account: String,
        #[arg(long)]
        message: String,
        #[arg(long,value_parser=["text","html"])]
        kind: String,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum DraftCommand {
    Attach {
        #[arg(long)]
        account: String,
        #[arg(long)]
        draft: String,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: u64,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value = "application/octet-stream")]
        mime_type: String,
        #[arg(long)]
        charset: Option<String>,
        #[arg(long)]
        content_id: Option<String>,
        #[arg(long)]
        inline: bool,
    },
    Save {
        #[arg(long)]
        account: String,
        #[arg(long, requires = "version")]
        draft: Option<String>,
        #[arg(long,requires="draft",value_parser=clap::value_parser!(u64).range(1..))]
        version: Option<u64>,
        #[arg(long)]
        file: PathBuf,
    },
    Show {
        #[arg(long)]
        account: String,
        #[arg(long)]
        draft: String,
    },
    List {
        #[arg(long)]
        account: String,
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        #[arg(long)]
        page_token: Option<String>,
    },
    Delete {
        #[arg(long)]
        account: String,
        #[arg(long)]
        draft: String,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: u64,
    },
}
#[derive(Subcommand)]
pub enum OperationsCommand {
    /// Inspect provider evidence; optionally resume only proven safe work.
    Reconcile {
        #[arg(long)]
        account: String,
        #[arg(long)]
        operation: String,
        #[arg(long)]
        request_id: String,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: u64,
        /// Permit safe desired-state retries or finishing an already-proven copy.
        /// This never authorizes an ambiguous send or COPY replay.
        #[arg(long)]
        resume_safe: bool,
        #[arg(long)]
        wait: bool,
    },
    /// Wait up to 30 seconds for provider acceptance or an inspectable outcome.
    Wait {
        #[arg(long)]
        account: String,
        #[arg(long)]
        operation: String,
    },
    /// Audit a decision about uncertain work. Resend can duplicate delivery.
    Resolve {
        #[arg(long)]
        account: String,
        #[arg(long)]
        operation: String,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: u64,
        #[arg(long)]
        file: std::path::PathBuf,
        /// Required for resend: a prior submission may already have been delivered.
        #[arg(long)]
        accept_duplicate_risk: bool,
    },
    List {
        #[arg(long)]
        account: String,
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Durable attempts and their evidence, newest first.
    Attempts {
        #[arg(long)]
        account: String,
        #[arg(long)]
        operation: String,
        #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        #[arg(long)]
        page_token: Option<String>,
    },
    Show {
        #[arg(long)]
        account: String,
        #[arg(long)]
        operation: String,
    },
    Cancel {
        #[arg(long)]
        account: String,
        #[arg(long)]
        operation: String,
    },
}
