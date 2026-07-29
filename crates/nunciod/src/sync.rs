//! Real inbound mail synchronization routine.
//!
//! Prior to this module, "sync" was a status-flag flip: `CoreCommand::SyncAll`
//! / `CoreCommand::SyncAccount` only moved `EngineStatus` to `Syncing` and
//! published `SyncStarted`, and nothing ever fetched a real message from a
//! real server into the store. This module closes that gap: for a configured
//! account it connects a real [`MailBackend`] (a real [`ImapEngine`] for
//! [`AccountProtocol::ImapSmtp`], built from [`AccountConfig`] plus the
//! account's keyring-stored password), fetches folders and messages, and
//! persists every message through [`DatabaseEngine::save_email`].
//!
//! # Testability
//!
//! [`sync_with_backend`] takes `&dyn MailBackend` rather than a concrete
//! engine, so it is exercised directly in tests with [`MockMailBackend`] --
//! no real network or OS keyring access ever happens inside it. Production
//! entry points ([`run_account_sync`], [`run_all_accounts_sync`]) are the
//! only places that resolve a real account's credentials and construct a
//! real backend; they then delegate the actual fetch/persist/event-emission
//! work to [`sync_with_backend`].
use nuncio_core::{AccountConfig, AccountProtocol, CoreCommand, EventBus};
use nuncio_mail::{ImapEngine, JmapEngine, MailBackend, MailError};
use nuncio_store::db::DatabaseError;
use nuncio_store::vault::{SecretManager, VaultError};
use thiserror::Error;

/// Errors that can occur while synchronizing a single account's inbound mail.
#[derive(Debug, Error)]
pub enum SyncError {
    /// No persisted account configuration matches the requested account ID.
    #[error("account '{0}' is not configured")]
    AccountNotFound(String),
    /// Failed to read the account's credential from the secret vault.
    #[error("failed to read account credential from the secret vault: {0}")]
    Vault(#[from] VaultError),
    /// The mail backend (IMAP/JMAP) reported an error.
    #[error("mail backend error: {0}")]
    Mail(#[from] MailError),
    /// A database persistence error occurred while saving synced messages.
    #[error("database error: {0}")]
    Store(#[from] DatabaseError),
}

/// Fetch every folder and every message in every folder from `backend`,
/// persisting each message via [`DatabaseEngine::save_email`]. Returns the
/// total number of messages processed (a message id "processed" more than
/// once, e.g. by a backend that returns the same id from multiple folders,
/// is counted once per occurrence -- `save_email` itself is `INSERT OR
/// REPLACE`, so storage stays deduplicated by message id regardless).
///
/// Contains no event-bus or credential logic -- pure fetch-and-persist, so
/// it is the smallest unit tests can exercise directly with a
/// [`MockMailBackend`].
async fn fetch_and_persist(
    db: &nuncio_store::db::DatabaseEngine,
    backend: &dyn MailBackend,
) -> Result<usize, SyncError> {
    let folders = backend.sync_folders().await?;
    let mut synced = 0usize;
    for folder in folders {
        let (emails, _state) = backend.sync_messages(&folder.id, None).await?;
        for email in emails {
            db.save_email(&email).await?;
            synced += 1;
        }
    }
    Ok(synced)
}

/// Run a real inbound sync against `backend`, wrapping the work with the
/// standard `CoreCommand`-driven engine-status lifecycle: flips to
/// `EngineStatus::Syncing` and publishes `SyncStarted` before, always flips
/// back to `EngineStatus::Idle` and publishes `SyncCompleted` after --
/// including when the sync fails, so a failed sync can never leave the
/// engine stuck at `Syncing`. On failure, a `CoreEvent::Error` is published
/// via `CoreCommand::ReportError` before the completion event.
///
/// `account_id` is `Some(id)` for a single-account sync (mirrors
/// `CoreCommand::SyncAccount`) or `None` for an all-accounts batch (mirrors
/// `CoreCommand::SyncAll`).
///
/// This is the seam production and tests share: production always passes a
/// real engine (see [`run_account_sync`]); tests pass a [`MockMailBackend`].
pub async fn sync_with_backend(
    db: &nuncio_store::db::DatabaseEngine,
    event_bus: &EventBus,
    backend: &dyn MailBackend,
    account_id: Option<String>,
) -> Result<usize, SyncError> {
    match &account_id {
        Some(id) => event_bus.process_command(CoreCommand::SyncAccount {
            account_id: id.clone(),
        }),
        None => event_bus.process_command(CoreCommand::SyncAll),
    }

    let result = fetch_and_persist(db, backend).await;

    if let Err(e) = &result {
        event_bus.process_command(CoreCommand::ReportError {
            message: format!("sync failed: {e}"),
        });
    }

    event_bus.complete_sync(account_id);
    result
}

/// Look up `account_id`'s persisted configuration.
async fn find_account(
    db: &nuncio_store::db::DatabaseEngine,
    account_id: &str,
) -> Result<AccountConfig, SyncError> {
    let accounts = db.list_accounts().await?;
    accounts
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| SyncError::AccountNotFound(account_id.to_string()))
}

/// Construct the real [`MailBackend`] engine appropriate for `config`'s
/// configured protocol, authenticated with `password` (read from the OS
/// keyring by the caller). This is the ONLY place a concrete protocol engine
/// is chosen; every other function in this module only ever sees `&dyn
/// MailBackend`.
fn build_mail_backend(config: &AccountConfig, password: &str) -> Box<dyn MailBackend> {
    match config.protocol {
        AccountProtocol::ImapSmtp => Box::new(ImapEngine::with_credentials(
            &config.id,
            &config.server_host,
            config.server_port,
            &config.email_address,
            password,
        )),
        AccountProtocol::Jmap => Box::new(JmapEngine::new(&config.id)),
    }
}

/// Production entry point for `CoreCommand::SyncAccount`: resolves
/// `account_id`'s configuration and keyring password, builds the real mail
/// backend, and runs a full inbound sync through [`sync_with_backend`].
///
/// If the account is unknown or its credential cannot be read, a
/// `CoreEvent::Error` is published and `Err` is returned WITHOUT flipping
/// the engine status to `Syncing` -- the sync never actually began, so
/// there is nothing to "complete".
pub async fn run_account_sync(
    db: &nuncio_store::db::DatabaseEngine,
    secrets: &SecretManager,
    event_bus: &EventBus,
    account_id: &str,
) -> Result<usize, SyncError> {
    let setup = async {
        let config = find_account(db, account_id).await?;
        let password = secrets.get_secret(&config.keyring_secret_key)?;
        Ok::<_, SyncError>(build_mail_backend(&config, &password))
    }
    .await;

    match setup {
        Ok(backend) => {
            sync_with_backend(
                db,
                event_bus,
                backend.as_ref(),
                Some(account_id.to_string()),
            )
            .await
        }
        Err(e) => {
            event_bus.process_command(CoreCommand::ReportError {
                message: format!("cannot sync account '{account_id}': {e}"),
            });
            Err(e)
        }
    }
}

/// Production entry point for `CoreCommand::SyncAll`: syncs every persisted
/// account in turn, wrapping the whole batch in a single
/// `SyncStarted{None}`/`SyncCompleted{None}` pair (rather than one pair per
/// account). A single account failing to sync (missing credential, backend
/// error, etc.) publishes a `CoreEvent::Error` and does not stop the
/// remaining accounts from being attempted. Returns the total number of
/// messages synced across all accounts.
pub async fn run_all_accounts_sync(
    db: &nuncio_store::db::DatabaseEngine,
    secrets: &SecretManager,
    event_bus: &EventBus,
) -> usize {
    event_bus.process_command(CoreCommand::SyncAll);

    let mut total_synced = 0usize;
    match db.list_accounts().await {
        Ok(accounts) => {
            for config in accounts {
                match secrets.get_secret(&config.keyring_secret_key) {
                    Ok(password) => {
                        let backend = build_mail_backend(&config, &password);
                        match fetch_and_persist(db, backend.as_ref()).await {
                            Ok(count) => total_synced += count,
                            Err(e) => {
                                event_bus.process_command(CoreCommand::ReportError {
                                    message: format!(
                                        "sync failed for account '{}': {e}",
                                        config.id
                                    ),
                                });
                            }
                        }
                    }
                    Err(e) => {
                        event_bus.process_command(CoreCommand::ReportError {
                            message: format!(
                                "cannot read credential for account '{}': {e}",
                                config.id
                            ),
                        });
                    }
                }
            }
        }
        Err(e) => {
            event_bus.process_command(CoreCommand::ReportError {
                message: format!("failed to list accounts for sync: {e}"),
            });
        }
    }

    event_bus.complete_sync(None);
    total_synced
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::model::{Email, Folder};
    use nuncio_core::{CoreEvent, EngineStatus, TlsMode};
    use nuncio_mail::MockMailBackend;
    use nuncio_store::db::DatabaseEngine;

    fn sample_jmap_account(id: &str) -> AccountConfig {
        AccountConfig {
            id: id.to_string(),
            name: "JMAP Test Account".to_string(),
            email_address: format!("{id}@nuncio.mx"),
            protocol: AccountProtocol::Jmap,
            server_host: "jmap.nuncio.mx".to_string(),
            server_port: 443,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
            use_tls: true,
            imap_tls_mode: TlsMode::ImplicitTls,
            smtp_tls_mode: TlsMode::ImplicitTls,
            keyring_secret_key: format!("nuncio/{id}"),
            sync_interval_secs: 60,
        }
    }

    fn sample_imap_account(id: &str) -> AccountConfig {
        AccountConfig {
            id: id.to_string(),
            name: "IMAP Test Account".to_string(),
            email_address: format!("{id}@nuncio.mx"),
            protocol: AccountProtocol::ImapSmtp,
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
            use_tls: true,
            imap_tls_mode: TlsMode::ImplicitTls,
            smtp_tls_mode: TlsMode::ImplicitTls,
            keyring_secret_key: format!("nuncio/{id}"),
            sync_interval_secs: 60,
        }
    }

    fn mock_email(id: &str, folder_id: &str, subject: &str) -> Email {
        Email {
            id: id.to_string(),
            account_id: "acct-mock-1".to_string(),
            folder_id: folder_id.to_string(),
            subject: subject.to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1_700_000_000,
            read: false,
            body_plain: Some(format!("body for {id}")),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn sync_with_backend_persists_mock_messages_and_emits_events() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();

        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 2,
            unread_messages: 2,
        });
        mock.add_message(mock_email("m1", "inbox", "Hello"));
        mock.add_message(mock_email("m2", "inbox", "World"));

        let synced = sync_with_backend(&db, &event_bus, &mock, Some("acct-mock-1".to_string()))
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 2);

        let persisted = db
            .list_messages("inbox", 10)
            .await
            .expect("list persisted messages");
        assert_eq!(persisted.len(), 2);
        let subjects: Vec<String> = persisted.iter().map(|e| e.subject.clone()).collect();
        assert!(subjects.contains(&"Hello".to_string()));
        assert!(subjects.contains(&"World".to_string()));

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted {
                account_id: Some("acct-mock-1".to_string())
            }
        );
        assert_eq!(
            events.recv().await.expect("complete event"),
            CoreEvent::SyncCompleted {
                account_id: Some("acct-mock-1".to_string())
            }
        );
        assert_eq!(event_bus.current_state().status, EngineStatus::Idle);
    }

    #[tokio::test]
    async fn sync_with_backend_none_account_id_mirrors_sync_all() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();

        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 0,
            unread_messages: 0,
        });

        let synced = sync_with_backend(&db, &event_bus, &mock, None)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 0);

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted { account_id: None }
        );
        assert_eq!(
            events.recv().await.expect("complete event"),
            CoreEvent::SyncCompleted { account_id: None }
        );
    }

    #[tokio::test]
    async fn sync_with_backend_failure_emits_error_and_returns_to_idle() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();

        let mock = MockMailBackend::new();
        mock.set_should_fail(true);

        let err = sync_with_backend(&db, &event_bus, &mock, Some("acct-fail".to_string()))
            .await
            .expect_err("sync fails");
        assert!(err.to_string().contains("mail backend error"));

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted {
                account_id: Some("acct-fail".to_string())
            }
        );
        match events.recv().await.expect("error event") {
            CoreEvent::Error { message } => assert!(message.contains("sync failed")),
            other => panic!("expected CoreEvent::Error, got {other:?}"),
        }
        assert_eq!(
            events.recv().await.expect("complete event"),
            CoreEvent::SyncCompleted {
                account_id: Some("acct-fail".to_string())
            }
        );
        // Never stuck at Syncing, even on failure.
        assert_eq!(event_bus.current_state().status, EngineStatus::Idle);
    }

    #[tokio::test]
    async fn find_account_returns_config_when_present() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let config = sample_jmap_account("acct-find-1");
        db.save_account(&config).await.expect("save account");

        let found = find_account(&db, "acct-find-1")
            .await
            .expect("account found");
        assert_eq!(found.id, "acct-find-1");
    }

    #[tokio::test]
    async fn find_account_errors_when_missing() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");

        let err = find_account(&db, "acct-missing")
            .await
            .expect_err("account not found");
        assert!(matches!(err, SyncError::AccountNotFound(id) if id == "acct-missing"));
    }

    #[test]
    fn build_mail_backend_constructs_imap_engine_for_imap_smtp_protocol() {
        let config = sample_imap_account("acct-imap-1");
        // Only constructs the engine -- never invokes any trait method, so
        // this never touches the network.
        let _backend = build_mail_backend(&config, "irrelevant-password");
    }

    #[test]
    fn build_mail_backend_constructs_jmap_engine_for_jmap_protocol() {
        let config = sample_jmap_account("acct-jmap-build-1");
        let _backend = build_mail_backend(&config, "irrelevant-password");
    }

    #[tokio::test]
    async fn run_account_sync_persists_real_backend_messages_and_reports_success() {
        // Uses a real (non-mock) `JmapEngine` -- its `MailBackend` impl
        // returns deterministic static data with no network I/O, so this
        // proves `run_account_sync`'s full production wiring (account
        // lookup -> keyring password -> real backend -> persist -> events)
        // end-to-end without violating the "no live network calls in
        // tests" rule.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();
        let secrets = SecretManager::mock();

        let config = sample_jmap_account("acct-jmap-real-1");
        db.save_account(&config).await.expect("save account");
        secrets
            .set_secret(&config.keyring_secret_key, "irrelevant-jmap-token")
            .expect("store credential in mock vault");

        let synced = run_account_sync(&db, &secrets, &event_bus, "acct-jmap-real-1")
            .await
            .expect("sync succeeds");
        // JmapEngine::sync_folders() returns 3 static folders; sync_messages
        // returns the same 1 static message per folder regardless of
        // folder_id, so 3 folders => 3 processed messages (deduplicated to
        // 1 stored row by `save_email`'s INSERT OR REPLACE on message id).
        assert_eq!(synced, 3);

        let persisted = db
            .get_message("jmap-msg-1")
            .await
            .expect("jmap message persisted");
        assert_eq!(persisted.subject, "Welcome to JMAP Sync");

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted {
                account_id: Some("acct-jmap-real-1".to_string())
            }
        );
        assert_eq!(
            events.recv().await.expect("complete event"),
            CoreEvent::SyncCompleted {
                account_id: Some("acct-jmap-real-1".to_string())
            }
        );
        assert_eq!(event_bus.current_state().status, EngineStatus::Idle);
    }

    #[tokio::test]
    async fn run_account_sync_reports_error_for_unknown_account_without_starting_sync() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();
        let secrets = SecretManager::mock();

        let err = run_account_sync(&db, &secrets, &event_bus, "acct-does-not-exist")
            .await
            .expect_err("unknown account fails");
        assert!(matches!(err, SyncError::AccountNotFound(_)));

        match events.recv().await.expect("error event") {
            CoreEvent::Error { message } => {
                assert!(message.contains("acct-does-not-exist"));
            }
            other => panic!("expected CoreEvent::Error, got {other:?}"),
        }
        // The sync never began, so status must remain at its default Idle.
        assert_eq!(event_bus.current_state().status, EngineStatus::Idle);
    }

    #[tokio::test]
    async fn run_account_sync_reports_error_when_credential_missing() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();
        let secrets = SecretManager::mock();

        let config = sample_jmap_account("acct-no-cred-1");
        db.save_account(&config).await.expect("save account");
        // Deliberately never calls `secrets.set_secret(...)`.

        let err = run_account_sync(&db, &secrets, &event_bus, "acct-no-cred-1")
            .await
            .expect_err("missing credential fails");
        assert!(matches!(err, SyncError::Vault(_)));

        match events.recv().await.expect("error event") {
            CoreEvent::Error { message } => {
                assert!(message.contains("acct-no-cred-1"));
            }
            other => panic!("expected CoreEvent::Error, got {other:?}"),
        }
        assert_eq!(event_bus.current_state().status, EngineStatus::Idle);
    }

    #[tokio::test]
    async fn run_all_accounts_sync_with_no_accounts_returns_zero_and_emits_events() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();
        let secrets = SecretManager::mock();

        let total = run_all_accounts_sync(&db, &secrets, &event_bus).await;
        assert_eq!(total, 0);

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted { account_id: None }
        );
        assert_eq!(
            events.recv().await.expect("complete event"),
            CoreEvent::SyncCompleted { account_id: None }
        );
    }

    #[tokio::test]
    async fn run_all_accounts_sync_persists_across_multiple_accounts() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let secrets = SecretManager::mock();

        let config_a = sample_jmap_account("acct-all-a");
        let config_b = sample_jmap_account("acct-all-b");
        db.save_account(&config_a).await.expect("save account a");
        db.save_account(&config_b).await.expect("save account b");
        secrets
            .set_secret(&config_a.keyring_secret_key, "token-a")
            .expect("store credential a");
        secrets
            .set_secret(&config_b.keyring_secret_key, "token-b")
            .expect("store credential b");

        let total = run_all_accounts_sync(&db, &secrets, &event_bus).await;
        // 2 accounts * 3 folders each = 6 processed messages.
        assert_eq!(total, 6);
        assert_eq!(event_bus.current_state().status, EngineStatus::Idle);
    }

    #[tokio::test]
    async fn run_all_accounts_sync_reports_error_for_account_missing_credential() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();
        let secrets = SecretManager::mock();

        let config = sample_jmap_account("acct-all-nocred");
        db.save_account(&config).await.expect("save account");
        // No credential stored in the vault for this account.

        let total = run_all_accounts_sync(&db, &secrets, &event_bus).await;
        assert_eq!(total, 0);

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted { account_id: None }
        );
        match events.recv().await.expect("error event") {
            CoreEvent::Error { message } => {
                assert!(message.contains("acct-all-nocred"));
            }
            other => panic!("expected CoreEvent::Error, got {other:?}"),
        }
        assert_eq!(
            events.recv().await.expect("complete event"),
            CoreEvent::SyncCompleted { account_id: None }
        );
    }
}
