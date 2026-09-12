mod cleanup_directory;
pub(crate) use cleanup_directory::CleanupDirectory;
pub(crate) use restore::stage_restore_with;
mod restore_jobs;
pub(crate) use restore_jobs::{DirectoryIdentity, OwnedDirectory, RestoreJob};
mod backup;
mod backup_io;
pub use backup::{inspect_backup, BackupArtifact, BackupInspection};
pub(crate) use backup_io::{preflight as backup_preflight, MAX_BACKUP_BYTES};
mod restore;
pub use restore::{stage_restore, RestoreReport, StagedRestore};
mod smtp;
pub use smtp::{SentCopyResult, ServerSentEvidence, SmtpIntent, SmtpProgress, SmtpStep};
mod accounts;
mod calendar_changes;
mod imap_accounts;
mod imap_mailboxes;
pub use calendar_changes::{
    CalendarChangePayload, CalendarChangeReceipt, CalendarRetryEvidence, CalendarWriteKind,
    EnqueueCalendarChange,
};
pub use imap_mailboxes::StoredImapMailbox;
mod imap_transfers;
pub use imap_transfers::{
    ImapChangePayload, ImapCopyProof, ImapTransferMode, ImapTransferPayload, ImapTransferProgress,
    ImapTransferResult, ImapTransferStep,
};
mod imap_changes;
pub use imap_changes::{ImapFlag, ImapFlagPayload};
mod mail_changes;
pub use mail_changes::{EnqueueMailChange, MailChangePayload, MailChangeReceipt};
mod draft_uploads;
mod drafts;
mod operation_attempts;
mod operation_queries;
mod operation_reconciliation;
mod operation_resolutions;
pub use operation_queries::{
    OperationAttemptPage, OperationPage, OperationSummary, ReadyOperation,
};
pub(crate) use operation_reconciliation::ExecutionPolicy;
pub use operation_reconciliation::{ReconcileOperation, ReconciliationMode, ReconciliationRequest};
pub use operation_resolutions::{
    ConfirmationEvidence, OperationResolution, ResolutionDecision, ResolveOperation,
};
mod operations;
pub use operation_attempts::{AttemptKind, AttemptOutcome, OperationAttempt, OperationReceipt};
mod prepared_drafts;
pub use draft_uploads::{DraftUpload, DraftUploadInput};
pub use drafts::{Draft, DraftAttachment, DraftPage, DraftSummary, SaveDraft};
pub use operations::{EnqueueSend, Operation, SendPayload};
mod changes;
mod schedules;
pub use changes::{ChangeEvent, ChangePage};
pub use schedules::SyncSchedule;
mod calendar;
mod calendar_repair;
mod repair;
pub use calendar_repair::CalendarRepairCheckpoint;
pub use repair::{PreservedStateCounts, ProjectionCounts, ProjectionScope, RepairPreview};
mod calendar_queries;
pub use calendar::{
    CalendarCatalogEntry, CalendarCheckpoint, CalendarCoverage, StoredCalendar, StoredEvent,
};
pub use calendar_queries::{CalendarAgendaPage, CalendarCatalogPage, CalendarEventResult};
mod blobs;
pub use blobs::StoredBlob;
mod mail;
pub use mail::*;
mod mail_projection;
mod mail_promotion;
mod mail_queries;
mod mail_staging;
mod migrations;
mod sync;
pub use sync::SyncRun;
mod worker;
pub(crate) use worker::private_directory;

use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("The local resource changed; read its latest version before editing")]
    VersionConflict,
    #[error("Different work is already active for this account and scope")]
    Busy,
    #[error("Change history is unavailable for this revision; take a new snapshot")]
    ChangeHistoryExpired,
    #[error("Query result is too large; request a smaller page")]
    ResultTooLarge,
    #[error("Requested local resource was not found")]
    NotFound,
    #[error("Invalid query or payload")]
    InvalidInput,
    #[error("The query snapshot changed; restart pagination")]
    RefreshRequired,
    #[error("Store is unavailable")]
    Unavailable,
    #[error("Store is already in use")]
    Locked,
    #[error("Database key must contain 32 bytes")]
    InvalidKey,
    #[error("Database key is incorrect or the database is damaged")]
    KeyOrCorrupt,
    #[error("Required database encryption is unavailable")]
    CipherUnavailable,
    #[error("Database schema is newer than this application")]
    FutureSchema,
    #[error("Restored profile was activated but directory sync failed; inspect the target before retrying")]
    RestoreActivationUncertain,
    #[error("Invalid account data")]
    InvalidAccount,
    #[error("Storage path must be a regular file or directory, without symlinks")]
    InvalidPath,
    #[error("Storage I/O failed ({0:?})")]
    Io(std::io::ErrorKind),
    #[error("Database operation failed (code {0:?})")]
    Database(Option<i32>),
}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.kind())
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value.sqlite_error().map(|error| error.extended_code))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountRecord {
    pub id: String,
    pub provider: String,
    pub address: String,
}

#[derive(Clone)]
pub struct StoredAccount {
    pub account: AccountRecord,
    pub subject: Option<String>,
    pub state: String,
    pub credential_ref: Option<String>,
}

pub struct ConnectedAccount {
    pub id: String,
    pub subject: String,
    pub address: String,
    pub credential_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoreStatus {
    pub schema_version: u32,
    pub account_count: u64,
    pub revision: u64,
}

type Reply<T> = oneshot::Sender<Result<T, StoreError>>;
type StoreJob = Box<dyn FnOnce(&mut rusqlite::Connection) + Send>;

enum Request {
    Execute(StoreJob),
    Status(Reply<StoreStatus>),
    AddAccount(AccountRecord, Reply<()>),
    Accounts(Reply<Vec<AccountRecord>>),
    Account(String, Reply<Option<StoredAccount>>),
    ConnectGoogle(ConnectedAccount, Reply<()>),
    SetAccountState(String, String, Reply<()>),
    PrepareCredential(String, Reply<()>),
    CredentialCleanup(Reply<Vec<String>>),
    FinishCredentialCleanup(String, Reply<()>),
    Close(Reply<()>),
}

#[derive(Clone)]
pub struct Store {
    // Drop all request senders before the final owner joins the worker.
    sender: mpsc::Sender<Request>,
    upload_slots: std::sync::Arc<tokio::sync::Semaphore>,
    pub(crate) revision: tokio::sync::watch::Receiver<u64>,
    _worker: std::sync::Arc<WorkerOwner>,
}
struct WorkerOwner(Option<std::thread::JoinHandle<()>>);
impl Drop for WorkerOwner {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            let _ = worker.join();
        }
    }
}

impl Store {
    pub fn queue_depth(&self) -> u64 {
        (self.sender.max_capacity() - self.sender.capacity()) as u64
    }
    async fn execute<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        self.request(|reply| {
            Request::Execute(Box::new(move |connection| {
                let _ = reply.send(job(connection));
            }))
        })
        .await
    }
    pub async fn account(&self, id: String) -> Result<Option<StoredAccount>, StoreError> {
        self.request(|reply| Request::Account(id, reply)).await
    }
    pub async fn connect_google(&self, account: ConnectedAccount) -> Result<(), StoreError> {
        self.request(|reply| Request::ConnectGoogle(account, reply))
            .await
    }
    pub async fn set_account_state(&self, id: String, state: String) -> Result<(), StoreError> {
        self.request(|reply| Request::SetAccountState(id, state, reply))
            .await
    }
    pub async fn prepare_credential(&self, reference: String) -> Result<(), StoreError> {
        self.request(|reply| Request::PrepareCredential(reference, reply))
            .await
    }
    pub async fn credential_cleanup(&self) -> Result<Vec<String>, StoreError> {
        self.request(Request::CredentialCleanup).await
    }
    pub async fn finish_credential_cleanup(&self, reference: String) -> Result<(), StoreError> {
        self.request(|reply| Request::FinishCredentialCleanup(reference, reply))
            .await
    }
    pub async fn open(directory: &Path, key: Zeroizing<Vec<u8>>) -> Result<Self, StoreError> {
        Self::open_with_options(directory, key, StoreOpenOptions::default()).await
    }
    pub(crate) async fn open_with_options(
        directory: &Path,
        key: Zeroizing<Vec<u8>>,
        options: StoreOpenOptions,
    ) -> Result<Self, StoreError> {
        if key.len() != 32 {
            return Err(StoreError::InvalidKey);
        }
        let directory = directory.to_path_buf();
        let (sender, receiver) = mpsc::channel(64);
        let (ready, result) = oneshot::channel();
        let (changed, revision) = tokio::sync::watch::channel(0);
        let worker = std::thread::Builder::new()
            .name("nuncio-store".into())
            .spawn(move || worker::run(directory, key, receiver, ready, changed, options))?;
        let store = Self {
            sender,
            upload_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
            revision,
            _worker: std::sync::Arc::new(WorkerOwner(Some(worker))),
        };
        result.await.map_err(|_| StoreError::Unavailable)??;
        Ok(store)
    }

    pub async fn close(self) -> Result<(), StoreError> {
        self.request(Request::Close).await
    }
    pub async fn add_account(&self, account: AccountRecord) -> Result<(), StoreError> {
        self.request(|reply| Request::AddAccount(account, reply))
            .await
    }
    pub async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError> {
        self.request(Request::Accounts).await
    }
    pub async fn status(&self) -> Result<StoreStatus, StoreError> {
        self.request(Request::Status).await
    }
    async fn request<T>(&self, make: impl FnOnce(Reply<T>) -> Request) -> Result<T, StoreError> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .send(make(reply))
            .await
            .map_err(|_| StoreError::Unavailable)?;
        receiver.await.map_err(|_| StoreError::Unavailable)?
    }
}

#[derive(Default)]
pub(crate) struct StoreOpenOptions {
    #[cfg(feature = "test-harness")]
    pub test_config: Option<crate::test_controls::TestConfig>,
}
