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
    /// Local data profile used by the daemon; one profile can contain multiple accounts.
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
    /// Add accounts or manage one saved account by its ID.
    #[command(
        arg_required_else_help = true,
        after_help = "Work on one account:\n  1. Run 'nuncio-cli account list' and find the matching address or display_name.\n  2. Copy that entry's id and pass it as --account ACCOUNT_ID after the action.\n     Use the local ID, not the email address or display name.\n  3. Use the same --profile as your daemon (for example --profile laptop-qa).\n     A profile can hold multiple accounts; each command selects its account.\n\nExamples (replace ACCOUNT_ID and VERSION with values from list/show):\n  nuncio-cli account list\n  nuncio-cli account show --account ACCOUNT_ID\n  nuncio-cli account edit --account ACCOUNT_ID --name 'Personal' --version VERSION\n  nuncio-cli account pause --account ACCOUNT_ID\n  nuncio-cli account resume --account ACCOUNT_ID\n  nuncio-cli account remove --account ACCOUNT_ID\n  nuncio-cli mail list --account ACCOUNT_ID\n\nRemoval archives local data by default. Use restore to recover an archived account.\nRun 'nuncio-cli account ACTION --help' for the options for any action."
    )]
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
    /// Add an account with guided prompts and hidden password entry.
    Add {
        /// Developer override for the build's Google Desktop registration.
        #[arg(long)]
        client_config: Option<PathBuf>,
        /// Print the Google consent URL instead of opening a browser.
        #[arg(long)]
        no_browser: bool,
    },
    /// Probe and save public IMAP/SMTP settings for the same IMAP principal.
    EditImap {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
        /// Current configuration version from account show.
        #[arg(long)]
        version: u64,
        /// Public IMAP/SMTP settings JSON; passwords are supplied separately.
        #[arg(long)]
        config: PathBuf,
        /// Optionally replace passwords using protected stdin input.
        #[arg(long)]
        credentials_stdin: bool,
    },
    /// Add a Google account and wait for browser consent by default.
    AddGoogle {
        #[arg(long)]
        client_config: Option<PathBuf>,
        #[arg(long)]
        login_hint: Option<String>,
        #[arg(long)]
        no_browser: bool,
        #[arg(long)]
        no_wait: bool,
    },
    /// Reauthenticate exactly this saved Google identity.
    ReauthGoogle {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
        #[arg(long)]
        client_config: Option<PathBuf>,
        #[arg(long)]
        no_browser: bool,
        #[arg(long)]
        no_wait: bool,
    },
    /// Replace passwords for the saved IMAP/SMTP configuration.
    ReauthImap {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
        #[arg(long, required = true)]
        credentials_stdin: bool,
    },
    /// Wait for terminal browser-consent status; pending consent returns a timeout error.
    AuthWait {
        #[arg(long)]
        session: String,
        #[arg(long, default_value_t = 300, value_parser=clap::value_parser!(u64).range(1..=300))]
        timeout_seconds: u64,
    },
    /// Read saved identity, display name, lifecycle and configuration version.
    Show {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Change the local display name using the version from account show.
    Edit {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
        /// New local display name; the provider login stays the same.
        #[arg(long)]
        name: String,
        /// Current version from account show; protects against overwriting a newer edit.
        #[arg(long)]
        version: u64,
    },
    /// Stop provider work while retaining credentials and downloaded data.
    Pause {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Resume background synchronization for a paused account.
    Resume {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Archive locally and disconnect credentials; downloaded data is retained.
    #[command(visible_alias = "archive")]
    Remove {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Unarchive a saved account. Reauthenticate separately to reconnect.
    Restore {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Permanently delete an archived account from this profile; remote data and backups remain.
    Purge {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
        #[arg(long, conflicts_with = "confirm")]
        dry_run: bool,
        /// Exact account ID. Required unless --dry-run is supplied.
        #[arg(long, required_unless_present = "dry_run")]
        confirm: Option<String>,
    },
    /// Cancel a pending Google browser sign-in session.
    AuthCancel {
        #[arg(long)]
        session: String,
    },
    /// Authenticate IMAP and SMTP with required TLS; passwords arrive only on stdin.
    #[command(visible_alias = "add-imap")]
    ConnectImap {
        /// Version1 public configuration JSON; no passwords.
        #[arg(long)]
        config: PathBuf,
        /// Read a bounded credential JSON object from a pipe (terminal input is refused).
        #[arg(long, required = true)]
        credentials_stdin: bool,
        /// Reconnect this exact saved IMAP endpoint and username.
        /// Use the local ID from account list, not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: Option<String>,
    },
    /// Read saved public configuration and the last authenticated capability snapshot.
    ImapConfig {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Start Google sign-in using a Desktop registration file; add-google waits by default.
    ConnectGoogle {
        /// Downloaded Desktop OAuth registration JSON, readable only by its owner.
        #[arg(long)]
        client_config: PathBuf,
        #[arg(long)]
        login_hint: Option<String>,
        /// Reconnect this saved identity; choosing a different Google user fails.
        /// Use the local ID from account list, not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: Option<String>,
        /// Return the browser URL without opening it automatically.
        #[arg(long)]
        no_browser: bool,
        /// Wait up to five minutes for consent; add-google waits by default.
        #[arg(long)]
        wait: bool,
    },
    /// Inspect a Google browser sign-in session by its session ID.
    AuthStatus {
        #[arg(long)]
        session: String,
    },
    /// List saved accounts and their IDs, addresses, names and states (local read).
    #[command(display_order = 0)]
    List {
        /// Also show archived accounts so they can be restored or permanently deleted.
        #[arg(long)]
        include_archived: bool,
    },
    /// Remove saved credentials and stop provider work; keep the local account and data.
    Disconnect {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Explicit online credential/identity check. Account list reads local state.
    Check {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
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
