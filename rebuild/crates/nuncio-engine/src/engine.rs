use crate::{
    domain::identity::ProfileId,
    profile,
    secrets::{SecretError, SecretStore},
    store::{Store, StoreError, StoreStatus},
};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc};
use zeroize::Zeroizing;
mod recovery;
mod repair;
pub use crate::maintenance::{BackupInput, BackupUpload};
pub use recovery::ProfileRestoreReport;
pub use repair::RepairResult;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Account(#[from] crate::accounts::AccountError),
    #[cfg(feature = "test-harness")]
    #[error(
        "Test configuration requires loopback endpoints, valid limits, and synthetic credentials"
    )]
    InvalidTestConfiguration,
    #[error("Engine is unavailable")]
    Unavailable,
    #[error("Profile metadata is missing or invalid; original files were preserved")]
    InvalidProfile,
    #[error("Required profile key is missing or invalid; original files were preserved")]
    MissingKey,
    #[error("Restore was not activated, but new key cleanup failed for profile {profile_id}; remove only that profile's database/API key entries")]
    RestoreCleanupIncomplete { profile_id: String },
    #[error("Restore recovery requires inspection for profile {profile_id}; files and keys were preserved where ownership or activation could not be proven")]
    RestoreRecoveryPending { profile_id: String },
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error(transparent)]
    Secret(#[from] SecretError),
}

pub struct EngineConfig {
    pub directory: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
}

pub struct Engine {
    maintenance: crate::maintenance::Maintenance,
    operations: crate::operations::OperationWorker,
    send_admission: tokio::sync::Semaphore,
    compose_slots: Arc<tokio::sync::Semaphore>,
    scheduler: crate::scheduler::Scheduler,
    accounts: Arc<crate::accounts::Accounts>,
    mail: Arc<crate::mail::MailSync>,
    calendar: Arc<crate::calendar::CalendarSync>,
    #[cfg(feature = "test-harness")]
    test_config: Option<crate::test_controls::TestConfig>,
    store: Store,
    profile_id: ProfileId,
    directory: PathBuf,
    authorization: Zeroizing<String>,
    _profile_lock: Arc<std::fs::File>,
}

#[derive(Serialize)]
pub struct EngineStatus {
    pub operation_worker_error: Option<String>,
    pub sync: Vec<crate::sync_status::SyncScopeStatus>,
    pub background_sync: bool,
    pub scheduler_error: Option<String>,
    pub version: String,
    pub api_version: String,
    pub profile_id: String,
    pub storage: StoreStatus,
    pub protected_paths: Vec<PathBuf>,
}

impl Engine {
    pub async fn change_message(
        &self,
        input: crate::store::EnqueueMailChange,
    ) -> Result<crate::store::Operation, StoreError> {
        self.store
            .enqueue_mail_change(input, self.accounts.http.clock.now_ms())
            .await
    }
    pub async fn mail_capabilities(
        &self,
        account: String,
    ) -> Result<crate::domain::mail_change::MailCapabilities, StoreError> {
        let row = self
            .store
            .account(account.clone())
            .await?
            .ok_or(StoreError::NotFound)?;
        let google = row.account.provider == "google";
        Ok(crate::domain::mail_change::MailCapabilities {
            account_id: account,
            provider: row.account.provider.clone(),
            placement_model: if google { "labels" } else { "folders" }.into(),
            read_unread: google || row.account.provider == "imap",
            star_unstar: google || row.account.provider == "imap",
            archive: google || row.account.provider == "imap",
            trash_restore: google || row.account.provider == "imap",
            existing_labels: google,
            move_copy: row.account.provider == "imap",
        })
    }
    pub async fn send_draft(
        &self,
        account: String,
        draft_id: String,
        request_id: String,
        expected_version: Option<u64>,
    ) -> Result<crate::store::Operation, StoreError> {
        if let Some(original) = self
            .store
            .find_send_request(
                account.clone(),
                request_id.clone(),
                draft_id.clone(),
                expected_version,
            )
            .await?
        {
            return Ok(original);
        }
        let _admission = self
            .send_admission
            .try_acquire()
            .map_err(|_| StoreError::Busy)?;
        let permit = self
            .compose_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        if let Some(original) = self
            .store
            .find_send_request(
                account.clone(),
                request_id.clone(),
                draft_id.clone(),
                expected_version,
            )
            .await?
        {
            return Ok(original);
        }
        let owner = self
            .store
            .account(account.clone())
            .await?
            .ok_or(StoreError::NotFound)?;
        let draft = self
            .store
            .get_draft(account.clone(), draft_id.clone())
            .await?;
        if expected_version.is_some_and(|v| v != draft.version) {
            return Err(StoreError::VersionConflict);
        }
        let mut attachments = Vec::with_capacity(draft.attachments.len());
        for a in &draft.attachments {
            let bytes = self
                .store
                .read_blob(
                    account.clone(),
                    crate::store::StoredBlob {
                        id: a.blob_id.clone(),
                        byte_length: a.byte_length,
                        sha256: a.sha256.clone(),
                    },
                )
                .await?;
            attachments.push(crate::domain::prepare::PreparedAttachment {
                filename: a.filename.clone(),
                mime_type: a.mime_type.clone(),
                parameters: a.parameters.clone(),
                content_id: a.content_id.clone(),
                disposition: a.disposition.clone(),
                bytes,
            });
        }
        let now = self.accounts.http.clock.now_ms();
        let version = draft.version;
        let sender = owner.account.address;
        let frozen_sender = sender.clone();
        let transport = match owner.account.provider.as_str() {
            "google" => crate::domain::submission::SubmissionTransport::Gmail,
            "imap" => crate::domain::submission::SubmissionTransport::Smtp,
            _ => return Err(StoreError::InvalidInput),
        };
        let (frozen, permit) = tokio::task::spawn_blocking(move || {
            let message_id = format!("{}@nuncio.invalid", uuid::Uuid::new_v4());
            let result = crate::domain::submission::freeze(
                &frozen_sender,
                &draft.content,
                draft.context.as_ref(),
                &attachments,
                &message_id,
                now,
                transport,
            );
            (result, permit)
        })
        .await
        .map_err(|_| StoreError::Unavailable)?;
        self.store
            .enqueue_send_guarded(
                crate::store::EnqueueSend {
                    account_id: account,
                    request_id,
                    draft_id,
                    expected_version,
                    snapshot_version: version,
                    sender,
                    frozen: frozen.map_err(|_| StoreError::InvalidInput)?,
                },
                now,
                Some(permit),
            )
            .await
    }
    pub async fn operation(
        &self,
        account: String,
        id: String,
    ) -> Result<crate::store::Operation, StoreError> {
        self.store.get_operation(account, id).await
    }
    pub async fn list_operations(
        &self,
        account: String,
        size: u32,
        token: Option<String>,
    ) -> Result<crate::store::OperationPage, StoreError> {
        self.store.query_operations(account, size, token).await
    }
    pub async fn operation_attempts(
        &self,
        account: String,
        id: String,
        size: u32,
        token: Option<String>,
    ) -> Result<crate::store::OperationAttemptPage, StoreError> {
        self.store
            .query_operation_attempts(account, id, size, token)
            .await
    }
    pub async fn cancel_operation(
        &self,
        account: String,
        id: String,
    ) -> Result<crate::store::Operation, StoreError> {
        self.store
            .cancel_operation(account, id, self.accounts.http.clock.now_ms())
            .await
    }
    pub async fn reconcile_operation(
        &self,
        input: crate::store::ReconcileOperation,
    ) -> Result<crate::store::Operation, StoreError> {
        let operation = self
            .store
            .request_reconciliation(input, self.accounts.http.clock.now_ms())
            .await?;
        #[cfg(feature = "test-harness")]
        if let Some(config) = &self.accounts.http.test_config {
            let (_sender, stop) = tokio::sync::watch::channel(false);
            config
                .checkpoint("reconciliation_after_admission", stop)
                .await
                .map_err(|_| StoreError::Unavailable)?;
        }
        Ok(operation)
    }
    pub async fn resolve_operation(
        &self,
        input: crate::store::ResolveOperation,
    ) -> Result<crate::store::Operation, StoreError> {
        use crate::{
            domain::submission::{reidentify, FrozenMessage},
            store::ResolutionDecision,
        };
        if let Some(original) = self.store.find_resolution(input.clone()).await? {
            return Ok(original);
        }
        if !matches!(input.decision, ResolutionDecision::Resend { .. }) {
            return self
                .store
                .resolve_operation(input, None, self.accounts.http.clock.now_ms())
                .await;
        }
        let _admission = self
            .send_admission
            .try_acquire()
            .map_err(|_| StoreError::Busy)?;
        let permit = self
            .compose_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        if let Some(original) = self.store.find_resolution(input.clone()).await? {
            return Ok(original);
        }
        let payload = self
            .store
            .send_payload(input.account_id.clone(), input.operation_id.clone())
            .await?;
        let wire = self
            .store
            .read_blob(input.account_id.clone(), payload.wire)
            .await?;
        let sent_copy = match payload.sent_copy {
            Some(blob) => Some(self.store.read_blob(input.account_id.clone(), blob).await?),
            None => None,
        };
        let now = self.accounts.http.clock.now_ms();
        let (frozen, permit) = tokio::task::spawn_blocking(move || {
            let result = (|| {
                let message_id = format!("{}@nuncio.invalid", uuid::Uuid::new_v4());
                Ok::<_, StoreError>(FrozenMessage {
                    wire: reidentify(&wire, &message_id, now)
                        .map_err(|_| StoreError::InvalidInput)?,
                    sent_copy: sent_copy
                        .as_ref()
                        .map(|b| reidentify(b, &message_id, now))
                        .transpose()
                        .map_err(|_| StoreError::InvalidInput)?,
                    recipients: payload.recipients,
                    thread_id: payload.thread_id,
                    message_id,
                })
            })();
            (result, permit)
        })
        .await
        .map_err(|_| StoreError::Unavailable)?;
        self.store
            .resolve_operation_guarded(input, Some(frozen?), now, Some(permit))
            .await
    }
    pub async fn prepare_draft(
        &self,
        account: String,
        message: String,
        kind: crate::domain::prepare::PrepareKind,
        body: Option<String>,
        to: Vec<crate::domain::drafts::Recipient>,
    ) -> Result<crate::store::Draft, StoreError> {
        let permit = self
            .compose_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| StoreError::Busy)?;
        let source = self
            .store
            .draft_source(account.clone(), message.clone())
            .await?;
        let raw = self.store.read_blob(account.clone(), source.raw).await?;
        let (prepared, permit) = tokio::task::spawn_blocking(move || {
            let prepared = crate::domain::prepare::prepare(
                &raw,
                &source.address,
                &message,
                source.thread_id.as_deref(),
                kind,
                body,
                to,
            );
            (prepared, permit)
        })
        .await
        .map_err(|_| StoreError::Unavailable)?;
        self.store
            .create_prepared_draft(
                account,
                prepared.map_err(|_| StoreError::InvalidInput)?,
                self.accounts.http.clock.now_ms(),
                permit,
            )
            .await
    }
    pub async fn begin_draft_upload(
        &self,
        input: crate::store::DraftUploadInput,
    ) -> Result<crate::store::DraftUpload, StoreError> {
        self.store
            .begin_draft_upload(input, self.accounts.http.clock.now_ms())
            .await
    }
    pub async fn append_draft_upload(
        &self,
        upload: &crate::store::DraftUpload,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<(), StoreError> {
        self.store.append_draft_upload(upload, offset, data).await
    }
    pub async fn finish_draft_upload(
        &self,
        upload: crate::store::DraftUpload,
    ) -> Result<crate::store::Draft, StoreError> {
        self.store
            .finish_draft_upload(upload, self.accounts.http.clock.now_ms())
            .await
    }
    pub async fn save_draft(
        &self,
        input: crate::store::SaveDraft,
    ) -> Result<crate::store::Draft, StoreError> {
        self.store
            .save_draft(input, self.accounts.http.clock.now_ms())
            .await
    }
    pub async fn get_draft(
        &self,
        account: String,
        id: String,
    ) -> Result<crate::store::Draft, StoreError> {
        self.store.get_draft(account, id).await
    }
    pub async fn list_drafts(
        &self,
        account: String,
        size: u32,
        token: Option<String>,
    ) -> Result<crate::store::DraftPage, StoreError> {
        self.store.query_drafts(account, size, token).await
    }
    pub async fn delete_draft(
        &self,
        account: String,
        id: String,
        version: u64,
    ) -> Result<(), StoreError> {
        self.store.delete_draft(account, id, version).await
    }
    pub async fn watch_changes(
        &self,
        after: u64,
    ) -> Result<crate::changes::ChangeReader, StoreError> {
        self.store.watch_changes(after).await
    }
    #[cfg(feature = "test-harness")]
    pub async fn open_for_test(
        config: EngineConfig,
        settings: crate::test_controls::TestConfig,
    ) -> Result<Self, EngineError> {
        settings
            .validate()
            .map_err(|_| EngineError::InvalidTestConfiguration)?;
        if !config.secrets.is_synthetic() {
            return Err(EngineError::InvalidTestConfiguration);
        }
        let http = crate::providers::google::http::GoogleHttp::for_test(&settings)?;
        let mut engine = Self::open_inner(config, http).await?;
        engine.test_config = Some(settings);
        Ok(engine)
    }
    #[cfg(feature = "test-harness")]
    pub fn test_config(&self) -> Option<&crate::test_controls::TestConfig> {
        self.test_config.as_ref()
    }
    pub async fn open(config: EngineConfig) -> Result<Self, EngineError> {
        Self::open_inner(
            config,
            crate::providers::google::http::GoogleHttp::production()?,
        )
        .await
    }
    async fn open_inner(
        config: EngineConfig,
        http: crate::providers::google::http::GoogleHttp,
    ) -> Result<Self, EngineError> {
        #[cfg(feature = "test-harness")]
        http.refresh_test_clock().await?;
        let directory = config.directory.clone();
        let secrets = config.secrets.clone();
        let profile = tokio::task::spawn_blocking(move || {
            profile::prepare(&directory, config.secrets.as_ref())
        })
        .await
        .map_err(|_| EngineError::Unavailable)??;
        let profile_lock = Arc::new(profile.lock);
        let options = crate::store::StoreOpenOptions {
            #[cfg(feature = "test-harness")]
            test_config: http.test_config.clone(),
        };
        let store =
            Store::open_with_options(&config.directory, profile.database_key, options).await?;
        if let Err(error) = recovery::recover_restores(
            &store,
            config.directory.clone(),
            profile.id,
            secrets.clone(),
            profile_lock.clone(),
        )
        .await
        {
            let _ = store.close().await;
            return Err(error);
        }
        let maintenance = crate::maintenance::Maintenance::new(
            config.directory.clone(),
            secrets.clone(),
            store.clone(),
            profile_lock.clone(),
        );
        store.recover_draft_uploads().await?;
        store.recover_operations(http.clock.now_ms()).await?;
        let accounts =
            crate::accounts::Accounts::new(store.clone(), profile.id.to_string(), secrets, http)
                .await?;
        let mail = crate::mail::MailSync::new(accounts.clone(), store.clone()).await?;
        let calendar = crate::calendar::CalendarSync::new(
            accounts.clone(),
            store.clone(),
            mail.coordinator.clone(),
        );
        let scheduler =
            crate::scheduler::Scheduler::start(store.clone(), mail.clone(), calendar.clone())
                .await?;
        let compose_slots = Arc::new(tokio::sync::Semaphore::new(1));
        let operations = crate::operations::OperationWorker::start(
            store.clone(),
            accounts.clone(),
            mail.coordinator.clone(),
            compose_slots.clone(),
        );
        Ok(Self {
            maintenance,
            operations,
            send_admission: tokio::sync::Semaphore::new(64),
            compose_slots,
            scheduler,
            accounts,
            mail,
            calendar,
            #[cfg(feature = "test-harness")]
            test_config: None,
            store,
            profile_id: profile.id,
            directory: config.directory,
            authorization: profile.authorization,
            _profile_lock: profile_lock,
        })
    }
    pub async fn status(&self) -> Result<EngineStatus, EngineError> {
        let protected_directory =
            std::fs::canonicalize(&self.directory).map_err(StoreError::from)?;
        let (storage, schedules) = self.store.status_with_schedules().await?;
        let now = self.accounts.http.clock.now_ms();
        let (poll, enabled) = self.accounts.http.sync_policy();
        Ok(EngineStatus {
            operation_worker_error: self.operations.error(),
            sync: schedules
                .into_iter()
                .map(|s| crate::sync_status::SyncScopeStatus::from_schedule(s, now, poll, enabled))
                .collect(),
            background_sync: self.scheduler.enabled,
            scheduler_error: self.scheduler.error(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: "nuncio.v2".into(),
            profile_id: self.profile_id.to_string(),
            storage,
            protected_paths: [
                "store.db",
                "store.db-wal",
                "store.db-shm",
                "store.db-journal",
                "store.lock",
                "profile.json",
                "profile.lock",
            ]
            .iter()
            .map(|name| protected_directory.join(name))
            .collect(),
        })
    }
    pub async fn shutdown(self) -> Result<(), EngineError> {
        self.scheduler.shutdown().await;
        self.operations.shutdown().await;
        self.calendar.shutdown().await;
        self.mail.shutdown().await;
        self.accounts.shutdown().await;
        self.store.close().await.map_err(EngineError::from)
    }
    pub async fn connect_imap(
        &self,
        config: crate::domain::imap_account::ImapAccountConfig,
        credentials: crate::domain::imap_account::ImapCredentials,
        account: Option<String>,
    ) -> Result<crate::accounts::ImapConnection, crate::accounts::AccountError> {
        let config = config
            .canonicalized()
            .map_err(|_| crate::accounts::AccountError::Invalid)?;
        let lane = account.clone().unwrap_or(
            config
                .identity()
                .map_err(|_| crate::accounts::AccountError::Invalid)?,
        );
        let _permit = self.mail.coordinator.acquire(&lane).await?;
        self.accounts
            .connect_imap(config, credentials, account)
            .await
    }
    pub async fn imap_config(
        &self,
        id: String,
    ) -> Result<
        (
            crate::domain::imap_account::ImapAccountConfig,
            crate::domain::imap_account::ImapCapabilities,
        ),
        crate::accounts::AccountError,
    > {
        self.accounts.imap_config(id).await
    }
    pub async fn begin_google_auth(
        &self,
        request: crate::accounts::GoogleAuthRequest,
    ) -> Result<crate::accounts::AuthSession, crate::accounts::AccountError> {
        self.accounts.begin(request).await
    }
    pub async fn auth_status(
        &self,
        id: &str,
    ) -> Result<crate::accounts::AuthSession, crate::accounts::AccountError> {
        self.accounts.auth_status(id).await
    }
    pub async fn list_accounts(
        &self,
    ) -> Result<Vec<crate::accounts::Account>, crate::accounts::AccountError> {
        self.accounts.list().await
    }
    pub async fn disconnect_account(&self, id: &str) -> Result<(), crate::accounts::AccountError> {
        self.accounts.disconnect(id).await
    }
    pub async fn check_account(
        &self,
        id: &str,
    ) -> Result<crate::accounts::Account, crate::accounts::AccountError> {
        let _permit = self.mail.coordinator.acquire(id).await?;
        self.accounts.check(id).await
    }
    pub fn authorization(&self) -> Zeroizing<String> {
        self.authorization.clone()
    }
    pub async fn sync_account(
        &self,
        account: String,
        full: bool,
        fetch: Option<String>,
    ) -> Result<crate::store::SyncRun, crate::mail::MailError> {
        self.mail.start(account, full, fetch).await
    }
    pub async fn sync_run(
        &self,
        account: String,
        run: String,
    ) -> Result<crate::store::SyncRun, StoreError> {
        self.store.sync_run(account, run).await
    }
    pub async fn cancel_sync(
        &self,
        account: &str,
        run: &str,
    ) -> Result<crate::store::SyncRun, crate::mail::MailError> {
        if self.store.sync_run(account.into(), run.into()).await?.scope == "calendar" {
            self.calendar.cancel(account, run).await
        } else {
            self.mail.cancel(account, run).await
        }
    }
    pub async fn list_mail(
        &self,
        query: crate::store::MailQuery,
    ) -> Result<crate::store::MailPage, StoreError> {
        self.store.query_mail(query).await
    }
    pub async fn get_mail(
        &self,
        account: String,
        id: String,
    ) -> Result<crate::store::MailDetail, StoreError> {
        self.store.get_mail(account, id).await
    }
    pub async fn mail_collections(
        &self,
        account: String,
    ) -> Result<(u64, Vec<crate::store::MailCollection>), StoreError> {
        self.store.mail_collections(account).await
    }
    pub async fn mail_blob(
        &self,
        account: String,
        id: String,
        kind: String,
        attachment: Option<String>,
    ) -> Result<crate::store::StoredBlob, StoreError> {
        self.store.mail_blob(account, id, kind, attachment).await
    }
    pub async fn blob_chunk(
        &self,
        account: String,
        id: String,
        ordinal: u64,
    ) -> Result<Vec<u8>, StoreError> {
        self.store.blob_chunk(account, id, ordinal).await
    }
    pub async fn query_free_busy(
        &self,
        input: crate::domain::free_busy::FreeBusyRequest,
    ) -> Result<crate::domain::free_busy::FreeBusyResult, crate::sync_error::SyncError> {
        self.calendar.free_busy(input).await
    }
    pub async fn change_event(
        &self,
        input: crate::store::EnqueueCalendarChange,
    ) -> Result<crate::store::Operation, crate::store::StoreError> {
        self.store
            .enqueue_calendar_change(input, self.accounts.http.clock.now_ms())
            .await
    }
    pub async fn refresh_calendar(
        &self,
        account: String,
        window: Option<crate::domain::calendar::AgendaWindow>,
    ) -> Result<crate::store::SyncRun, crate::sync_error::SyncError> {
        self.calendar.start(account, window).await
    }
    pub async fn calendar_list(
        &self,
        account: String,
        size: u32,
        token: Option<String>,
    ) -> Result<crate::store::CalendarCatalogPage, StoreError> {
        self.store.calendar_catalog_page(account, size, token).await
    }
    pub async fn calendar_event(
        &self,
        account: String,
        calendar: String,
        event: String,
    ) -> Result<crate::store::CalendarEventResult, StoreError> {
        self.store.calendar_event(account, calendar, event).await
    }
    pub async fn calendar_agenda(
        &self,
        account: String,
        window: crate::domain::calendar::AgendaWindow,
        size: u32,
        token: Option<String>,
    ) -> Result<crate::store::CalendarAgendaPage, StoreError> {
        self.store
            .calendar_agenda(account, window, size, token)
            .await
    }
}
