use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Mail and calendar on your machine", arg_required_else_help = true, color = clap::ColorChoice::Never, after_help = crate::help_text::ROOT)]
pub struct Args {
    /// Output the versioned JSON result or error for scripts; default output is readable text.
    #[arg(long, global = true)]
    pub json: bool,
    /// Loopback HTTP address of the running daemon.
    #[arg(long, global = true, default_value = "http://127.0.0.1:9421")]
    pub endpoint: String,
    /// Use this explicit local data directory instead of the named profile default.
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
    /// Local data profile used by the daemon; one profile can contain multiple accounts.
    #[arg(long, global = true, default_value = "default")]
    pub profile: String,
    #[cfg(feature = "test-harness")]
    /// Synthetic keystore file for offline test harnesses; absent from production builds.
    #[arg(long, global = true)]
    pub test_secrets_file: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Rebuild a provider projection while preserving drafts and operation history.
    #[command(after_help = crate::help_text::REPAIR)]
    Repair(RepairArgs),
    /// Create, inspect or restore encrypted local backups.
    #[command(arg_required_else_help = true, after_help = crate::help_text::BACKUP)]
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    /// Inspect queued work, delivery outcomes and recovery decisions.
    #[command(name = "operation", visible_alias = "operations")]
    #[command(arg_required_else_help = true, after_help = crate::help_text::OPERATIONS)]
    Operations {
        #[command(subcommand)]
        command: OperationsCommand,
    },
    /// Read cached calendars and manage Google calendar events.
    #[command(arg_required_else_help = true, after_help = crate::help_text::CALENDAR)]
    Calendar {
        #[command(subcommand)]
        command: CalendarCommand,
    },
    /// Download mail changes for one account; returns a sync run unless --wait is used.
    #[command(after_help = crate::help_text::SYNC)]
    Sync {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Reconcile the complete remote mail listing instead of only incremental changes.
        #[arg(long)]
        full: bool,
        /// Wait for completion and report the final state; queued work remains inspectable after a timeout.
        #[arg(long)]
        wait: bool,
    },
    /// List, read, search, draft and send mail for one account.
    #[command(arg_required_else_help = true, after_help = crate::help_text::MAIL)]
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
    /// Check the daemon, follow changes, inspect sync runs or shut it down.
    #[command(arg_required_else_help = true, after_help = crate::help_text::SYSTEM)]
    System {
        #[command(subcommand)]
        command: SystemCommand,
    },
}

#[derive(clap::Args)]
pub struct RepairArgs {
    /// Local account ID from account list; not an email address or display name.
    #[arg(long)]
    #[arg(value_name = "ACCOUNT_ID")]
    pub account: String,
    /// Provider cache to rebuild: mail or calendar; local drafts and history are preserved.
    #[arg(long, value_enum)]
    pub scope: RepairScope,
    /// Preview the local change without applying it or contacting the provider.
    #[arg(long)]
    pub dry_run: bool,
    /// Wait for completion and report the final state; queued work remains inspectable after a timeout.
    #[arg(long, conflicts_with = "dry_run")]
    pub wait: bool,
    /// Inclusive start date, YYYY-MM-DD; use together with --to.
    #[arg(long, requires = "to")]
    pub from: Option<String>,
    /// Exclusive end date, YYYY-MM-DD; use together with --from.
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
    #[command(after_help = crate::help_text::BACKUP)]
    Create {
        /// New destination file on this machine; existing files are never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect a backup; read recovery-passphrase JSON from protected stdin.
    #[command(after_help = crate::help_text::BACKUP)]
    Inspect {
        /// Existing encrypted Nuncio backup file.
        #[arg(long)]
        file: PathBuf,
    },
    /// Restore into a new sibling profile; all pending writes remain held.
    #[command(after_help = crate::help_text::BACKUP)]
    Restore {
        /// Existing encrypted Nuncio backup file to restore into a new profile.
        #[arg(long)]
        file: PathBuf,
        /// New sibling profile name (1-64 letters, digits, hyphens or underscores); must not exist.
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
    #[command(after_help = crate::help_text::FREE_BUSY)]
    FreeBusy {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Version1 query JSON with RFC3339 bounds, time zone, and provider calendar IDs.
        #[arg(long)]
        file: PathBuf,
    },
    /// Create, update, delete or respond to an event using an explicit JSON action.
    #[command(after_help = crate::help_text::CALENDAR_CHANGE)]
    Change {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local calendar ID from calendar list.
        #[arg(long)]
        calendar: String,
        /// A UUID for this action (create once with uuidgen); reuse only for the identical retry.
        #[arg(long)]
        #[arg(value_parser = parse_request_id)]
        request_id: String,
        /// Version1 Calendar action JSON with explicit scope and notification policy.
        #[arg(long)]
        file: PathBuf,
        /// Wait for completion and report the final state; queued work remains inspectable after a timeout.
        #[arg(long)]
        wait: bool,
    },
    /// List cached calendars and their local IDs.
    List {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Number of items per page (1-100).
        #[arg(long, default_value_t = 100)]
        #[arg(value_parser = clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        /// Opaque next_page_token from the previous result; keep the same query and page size.
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Read one cached event by local calendar and event IDs.
    Get {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local calendar ID from calendar list.
        #[arg(long)]
        calendar: String,
        /// Local event ID from calendar agenda.
        #[arg(long)]
        event: String,
    },
    /// List cached occurrences in a date window; reports cache coverage.
    Agenda {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Inclusive start date, YYYY-MM-DD; use together with --to.
        #[arg(long)]
        from: String,
        /// Exclusive upper date, interpreted in each calendar's time zone.
        #[arg(long)]
        to: String,
        /// Number of items per page (1-100).
        #[arg(long, default_value_t = 100)]
        #[arg(value_parser = clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        /// Opaque next_page_token from the previous result; keep the same query and page size.
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Download calendars and events; optionally refresh a paired date window.
    Refresh {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Inclusive start date, YYYY-MM-DD; use together with --to.
        #[arg(long, requires = "to")]
        from: Option<String>,
        /// Exclusive end date, YYYY-MM-DD; use together with --from.
        #[arg(long, requires = "from")]
        to: Option<String>,
        /// Wait for completion and report the final state; queued work remains inspectable after a timeout.
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Subcommand)]
pub enum AccountCommand {
    /// Add an account with guided prompts and hidden password entry.
    #[command(
        after_help = "Run in an interactive terminal. This guided command does not accept --json.\nFor scripts, use add-google or connect-imap; see their --help for private input formats."
    )]
    Add {
        /// Private Google Desktop OAuth JSON; needed when the build has no bundled registration.
        #[arg(long)]
        client_config: Option<PathBuf>,
        /// Print the Google consent URL instead of opening a browser.
        #[arg(long)]
        no_browser: bool,
    },
    /// Probe and save public IMAP/SMTP settings for the same IMAP principal.
    #[command(after_help = crate::help_text::IMAP_SETUP)]
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
    #[command(after_help = crate::help_text::GOOGLE_SETUP)]
    AddGoogle {
        /// Private Google Desktop OAuth JSON; needed when the build has no bundled registration.
        #[arg(long)]
        client_config: Option<PathBuf>,
        /// Google email address to suggest in browser sign-in.
        #[arg(long)]
        login_hint: Option<String>,
        /// Print the consent URL so you can open it yourself.
        #[arg(long)]
        no_browser: bool,
        /// Return the sign-in session immediately; inspect it with account auth-status.
        #[arg(long)]
        no_wait: bool,
    },
    /// Reauthenticate exactly this saved Google identity.
    #[command(after_help = crate::help_text::GOOGLE_SETUP)]
    ReauthGoogle {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
        /// Private Google Desktop OAuth JSON; needed when the build has no bundled registration.
        #[arg(long)]
        client_config: Option<PathBuf>,
        /// Print the consent URL so you can open it yourself.
        #[arg(long)]
        no_browser: bool,
        /// Return the sign-in session immediately; inspect it with account auth-status.
        #[arg(long)]
        no_wait: bool,
    },
    /// Replace passwords for the saved IMAP/SMTP configuration.
    #[command(after_help = crate::help_text::IMAP_SETUP)]
    ReauthImap {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long, value_name = "ACCOUNT_ID")]
        account: String,
        /// Read credential JSON from a secure pipe; use account add for hidden interactive entry.
        #[arg(long, required = true)]
        credentials_stdin: bool,
    },
    /// Wait for terminal browser-consent status; pending consent returns a timeout error.
    AuthWait {
        /// Sign-in session_id returned by the Google connection command.
        #[arg(long)]
        session: String,
        /// Maximum seconds to wait for Google consent (1-300).
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
        /// Preview the local change without applying it or contacting the provider.
        #[arg(long, conflicts_with = "confirm")]
        dry_run: bool,
        /// Exact account ID. Required unless --dry-run is supplied.
        #[arg(long, required_unless_present = "dry_run")]
        confirm: Option<String>,
    },
    /// Cancel a pending Google browser sign-in session.
    AuthCancel {
        /// Sign-in session_id returned by the Google connection command.
        #[arg(long)]
        session: String,
    },
    /// Authenticate IMAP and SMTP with required TLS; passwords arrive only on stdin.
    #[command(visible_alias = "add-imap")]
    #[command(after_help = crate::help_text::IMAP_SETUP)]
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
    #[command(after_help = crate::help_text::GOOGLE_SETUP)]
    ConnectGoogle {
        /// Downloaded Desktop OAuth registration JSON, readable only by its owner.
        #[arg(long)]
        client_config: PathBuf,
        /// Google email address to suggest in browser sign-in.
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
        /// Sign-in session_id returned by the Google connection command.
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
        /// Last observed revision; get an initial revision from system status.
        #[arg(long)]
        after: u64,
        /// Stop successfully after this many changes; otherwise watch until interrupted.
        #[arg(long,value_parser=clap::value_parser!(u32).range(1..))]
        limit: Option<u32>,
    },
    /// Show daemon health, storage, account counts and background activity.
    Status,
    /// Gracefully stop the daemon after draining active workers.
    Shutdown,
    /// Inspect the state and error code of a saved synchronization run.
    SyncStatus {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Sync run ID returned by sync, mail fetch, calendar refresh or repair.
        #[arg(long)]
        run: String,
    },
    /// Cancel a queued or running sync; keep the last complete local cache.
    CancelSync {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Sync run ID returned by sync, mail fetch, calendar refresh or repair.
        #[arg(long)]
        run: String,
    },
}
#[derive(clap::Args)]
pub struct MailQueryArgs {
    /// Local account ID from account list; not an email address or display name.
    #[arg(long)]
    #[arg(value_name = "ACCOUNT_ID")]
    pub account: String,
    /// Local label or folder ID from mail collections; omit to search all cached mail.
    #[arg(long)]
    pub collection: Option<String>,
    /// Number of items per page (1-100).
    #[arg(long, default_value_t = 100)]
    #[arg(value_parser = clap::value_parser!(u32).range(1..=100))]
    pub page_size: u32,
    /// Opaque next_page_token from the previous result; keep the same query and page size.
    #[arg(long)]
    pub page_token: Option<String>,
}
#[derive(Subcommand)]
pub enum MailCommand {
    /// Report supported actions and provider placement semantics.
    Capabilities {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Durably request an explicit read/star/archive/trash/label state from a JSON file.
    #[command(after_help = crate::help_text::MAIL_CHANGE)]
    Change {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
        /// A UUID for this action (create once with uuidgen); reuse only for the identical retry.
        #[arg(long)]
        #[arg(value_parser = parse_request_id)]
        request_id: String,
        /// Action JSON with schema_version 1; see the example below.
        #[arg(long)]
        file: PathBuf,
        /// Wait for completion and report the final state; queued work remains inspectable after a timeout.
        #[arg(long)]
        wait: bool,
    },
    /// Durably enqueue an immutable submission; inspect its operation for delivery.
    Send {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local draft ID from mail draft list or mail draft show.
        #[arg(long)]
        draft: String,
        /// A UUID for this action (create once with uuidgen); reuse only for the identical retry.
        #[arg(long)]
        #[arg(value_parser = parse_request_id)]
        request_id: String,
        /// Current object version from show; protects against overwriting newer changes.
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: Option<u64>,
        /// Wait for completion and report the final state; queued work remains inspectable after a timeout.
        #[arg(long)]
        wait: bool,
    },
    /// Create a local reply draft; sending is a separate explicit command.
    Reply {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
        /// UTF-8 text file for the new message body.
        #[arg(long)]
        body_file: PathBuf,
    },
    /// Reply to all visible recipients, excluding this account's address.
    ReplyAll {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
        /// UTF-8 text file for the new message body.
        #[arg(long)]
        body_file: PathBuf,
    },
    /// Create a local forward with original bodies and attachments.
    Forward {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
        /// Recipient email address; repeat --to for multiple recipients.
        #[arg(long, required = true)]
        to: Vec<String>,
        /// UTF-8 text file for the new message body.
        #[arg(long)]
        body_file: Option<PathBuf>,
    },
    /// Create and manage local drafts and their attachments.
    #[command(arg_required_else_help = true, after_help = crate::help_text::DRAFT)]
    Draft {
        #[command(subcommand)]
        command: DraftCommand,
    },
    /// List downloaded message summaries and local message IDs.
    List {
        #[command(flatten)]
        args: MailQueryArgs,
    },
    /// Search the local mail cache; no provider request is made.
    Search {
        #[command(flatten)]
        args: MailQueryArgs,
        /// Words to find in the downloaded mail cache; quote multiple words.
        #[arg(long)]
        query: String,
    },
    /// List cached Gmail labels or IMAP folders and their local IDs.
    Collections {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
    },
    /// Show cached message metadata, body availability and attachment IDs.
    Read {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
    },
    /// Download the original message and attachments from the provider.
    Fetch {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
        /// Wait for completion and report the final state; queued work remains inspectable after a timeout.
        #[arg(long)]
        wait: bool,
    },
    /// Export the cached original message to a new EML file.
    Raw {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
        /// New destination file on this machine; existing files are never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
    /// Save a cached attachment to a new file; get its ID from mail read.
    Attachment {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
        /// Local attachment ID from mail read.
        #[arg(long)]
        attachment: String,
        /// New destination file on this machine; existing files are never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
    /// Save cached text or HTML to a new file without rendering it.
    Body {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local message ID from mail list or mail search.
        #[arg(long)]
        message: String,
        /// Body representation to export: text or inert HTML.
        #[arg(long,value_parser=["text","html"])]
        kind: String,
        /// New destination file on this machine; existing files are never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum DraftCommand {
    /// Add a local file to a draft using its current version.
    Attach {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local draft ID from mail draft list or mail draft show.
        #[arg(long)]
        draft: String,
        /// Current object version from show; protects against overwriting newer changes.
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: u64,
        /// Stable local attachment file, at most 64 MiB.
        #[arg(long)]
        file: PathBuf,
        /// Attachment media type, for example application/pdf.
        #[arg(long, default_value = "application/octet-stream")]
        mime_type: String,
        /// Optional MIME character set for a text attachment, for example utf-8.
        #[arg(long)]
        charset: Option<String>,
        /// Optional MIME Content-ID for referencing an inline attachment.
        #[arg(long)]
        content_id: Option<String>,
        /// Mark the attachment for inline display; it remains inert in the CLI.
        #[arg(long)]
        inline: bool,
    },
    /// Create or update a local draft from JSON; does not send it.
    #[command(after_help = crate::help_text::DRAFT_SAVE)]
    Save {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local draft ID from mail draft list or mail draft show.
        #[arg(long, requires = "version")]
        draft: Option<String>,
        /// Current object version from show; protects against overwriting newer changes.
        #[arg(long,requires="draft",value_parser=clap::value_parser!(u64).range(1..))]
        version: Option<u64>,
        /// Draft content JSON containing recipients, subject and text; see the example below.
        #[arg(long)]
        file: PathBuf,
    },
    /// Show draft content, attachments and the version needed for editing.
    Show {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local draft ID from mail draft list or mail draft show.
        #[arg(long)]
        draft: String,
    },
    /// List local drafts and their IDs for one account.
    List {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Number of items per page (1-100).
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        /// Opaque next_page_token from the previous result; keep the same query and page size.
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Delete a local draft using its current version; does not delete remote mail.
    Delete {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local draft ID from mail draft list or mail draft show.
        #[arg(long)]
        draft: String,
        /// Current object version from show; protects against overwriting newer changes.
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: u64,
    },
}
#[derive(Subcommand)]
pub enum OperationsCommand {
    /// Inspect provider evidence; optionally resume only proven safe work.
    Reconcile {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local operation ID from operation list or the command that queued it.
        #[arg(long)]
        operation: String,
        /// A UUID for this action (create once with uuidgen); reuse only for the identical retry.
        #[arg(long)]
        #[arg(value_parser = parse_request_id)]
        request_id: String,
        /// Current object version from show; protects against overwriting newer changes.
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: u64,
        /// Permit safe desired-state retries or finishing an already-proven copy.
        /// This never authorizes an ambiguous send or COPY replay.
        #[arg(long)]
        resume_safe: bool,
        /// Wait for completion and report the final state; queued work remains inspectable after a timeout.
        #[arg(long)]
        wait: bool,
    },
    /// Wait up to 30 seconds for provider acceptance or an inspectable outcome.
    Wait {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local operation ID from operation list or the command that queued it.
        #[arg(long)]
        operation: String,
    },
    /// Audit a decision about uncertain work. Resend can duplicate delivery.
    #[command(after_help = crate::help_text::RESOLVE)]
    Resolve {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local operation ID from operation list or the command that queued it.
        #[arg(long)]
        operation: String,
        /// Current object version from show; protects against overwriting newer changes.
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        version: u64,
        /// Decision JSON: abandon, confirm_applied or resend; see the example below.
        #[arg(long)]
        file: std::path::PathBuf,
        /// Required for resend: a prior submission may already have been delivered.
        #[arg(long)]
        accept_duplicate_risk: bool,
    },
    /// List durable operations and their current outcomes for one account.
    List {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Number of items per page (1-100).
        #[arg(long,default_value_t=100,value_parser=clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        /// Opaque next_page_token from the previous result; keep the same query and page size.
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Durable attempts and their evidence, newest first.
    Attempts {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local operation ID from operation list or the command that queued it.
        #[arg(long)]
        operation: String,
        /// Number of items per page (1-100).
        #[arg(long,default_value_t=25,value_parser=clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
        /// Opaque next_page_token from the previous result; keep the same query and page size.
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Show one operation, its evidence and any required recovery decision.
    Show {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local operation ID from operation list or the command that queued it.
        #[arg(long)]
        operation: String,
    },
    /// Cancel work before provider dispatch; completed or uncertain effects cannot be undone.
    Cancel {
        /// Local account ID from account list; not an email address or display name.
        #[arg(long)]
        #[arg(value_name = "ACCOUNT_ID")]
        account: String,
        /// Local operation ID from operation list or the command that queued it.
        #[arg(long)]
        operation: String,
    },
}

fn parse_request_id(value: &str) -> Result<String, &'static str> {
    uuid::Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| "Request IDs must be UUIDs")
}
