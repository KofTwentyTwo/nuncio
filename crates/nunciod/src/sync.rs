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
use nuncio_filter::{FilterEngine, OutboxManager, RuleAction};
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

/// Map a [`RuleAction`] to the short mutation tag persisted on
/// [`nuncio_filter::PendingRemoteMutation::mutation_type`] and the outbox
/// target parameter, for every action that is not applied immediately.
///
/// Nothing downstream currently parses `mutation_type` back into a
/// `RuleAction`, so these tags are a fixed vocabulary owned by this module;
/// keep them stable once the outbox worker starts consuming them.
fn remote_action_tag(action: &RuleAction) -> Option<(&'static str, Option<String>)> {
    match action {
        RuleAction::MoveTo(folder) => Some(("MOVE", Some(folder.clone()))),
        RuleAction::CopyTo(folder) => Some(("COPY", Some(folder.clone()))),
        RuleAction::Flag => Some(("FLAG", None)),
        RuleAction::Unflag => Some(("UNFLAG", None)),
        RuleAction::Delete => Some(("DELETE", None)),
        RuleAction::ForwardTo(address) => Some(("FORWARD", Some(address.clone()))),
        RuleAction::CallWebhook(url) => Some(("WEBHOOK", Some(url.clone()))),
        RuleAction::MarkRead | RuleAction::MarkUnread => None,
    }
}

/// Evaluate `email` against `filter_engine` and route every matched rule's
/// actions to their real, persisted effect.
///
/// `MarkRead`/`MarkUnread` apply immediately to the stored message via
/// [`DatabaseEngine::set_message_read`]. Every other action enqueues a real
/// [`nuncio_filter::PendingRemoteMutation`] through the existing outbox
/// (`OutboxManager::create_mutation` + `DatabaseEngine::save_pending_mutation`)
/// for the background outbox worker to pick up -- this function never
/// performs the real remote IMAP/JMAP mutation or webhook call itself.
/// Every match also appends a [`nuncio_filter::FilterExecutionLog`] entry via
/// `DatabaseEngine::save_filter_execution_log`, so there is an audit trail of
/// what fired independent of whether the outbox item is ever drained.
///
/// A bookkeeping failure on one action (a DB write error) is logged via
/// `tracing::warn!` and does not stop the remaining actions/rules from being
/// applied -- one broken write must never silently swallow the rest of a
/// sync's filtering. The return value counts only actions that were fully
/// applied AND logged; a partially-failed match (e.g. the mutation persists
/// but the execution log write fails) is not counted as success, keeping the
/// count honest for callers that log or assert on it.
///
/// Standalone and reusable by design: this is the exact evaluate-and-route
/// logic other sync paths (e.g. a bulk retroactive triage over the whole
/// store) need, so it takes only a `DatabaseEngine`/`FilterEngine`/`Email`
/// and has no dependency on the live inbound-sync call chain.
pub async fn apply_filter_actions(
    db: &nuncio_store::db::DatabaseEngine,
    filter_engine: &FilterEngine,
    email: &nuncio_core::model::Email,
) -> usize {
    let mut applied = 0usize;

    for (rule, actions) in filter_engine.evaluate(email) {
        for action in actions {
            let immediate_ok = match &action {
                RuleAction::MarkRead => match db.set_message_read(&email.id, true).await {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!(
                            "filter rule '{}' MARK READ failed for message '{}': {e}",
                            rule.id,
                            email.id
                        );
                        false
                    }
                },
                RuleAction::MarkUnread => match db.set_message_read(&email.id, false).await {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!(
                            "filter rule '{}' MARK UNREAD failed for message '{}': {e}",
                            rule.id,
                            email.id
                        );
                        false
                    }
                },
                _ => {
                    // Remote actions are handled below via the outbox; this
                    // branch only tracks whether the immediate-action write
                    // itself succeeded (there is none for a remote action,
                    // so it is unconditionally "not applicable" here).
                    true
                }
            };

            if !immediate_ok {
                continue;
            }

            let mut action_ok = true;
            if let Some((tag, target)) = remote_action_tag(&action) {
                let mutation = OutboxManager::create_mutation(&rule.id, &email.id, tag, target);
                if let Err(e) = db.save_pending_mutation(&mutation).await {
                    tracing::warn!(
                        "filter rule '{}' failed to enqueue outbox mutation for message '{}': {e}",
                        rule.id,
                        email.id
                    );
                    action_ok = false;
                }
            }

            if !action_ok {
                continue;
            }

            let action_desc = action.to_nsql();
            if let Err(e) = db
                .save_filter_execution_log(&rule.id, &email.id, &action_desc)
                .await
            {
                tracing::warn!(
                    "filter rule '{}' matched message '{}' but failed to write execution log: {e}",
                    rule.id,
                    email.id
                );
                continue;
            }

            applied += 1;
        }
    }

    applied
}

/// Fetch every folder and every message in every folder from `backend`,
/// persisting each message via [`DatabaseEngine::save_email`] and then
/// evaluating it against `filter_engine` via [`apply_filter_actions`].
/// Returns the total number of messages processed (a message id "processed"
/// more than once, e.g. by a backend that returns the same id from multiple
/// folders, is counted once per occurrence -- `save_email` itself is
/// `INSERT OR REPLACE`, so storage stays deduplicated by message id
/// regardless).
///
/// Contains no event-bus or credential logic -- pure fetch-and-persist, so
/// it is the smallest unit tests can exercise directly with a
/// [`MockMailBackend`].
async fn fetch_and_persist(
    db: &nuncio_store::db::DatabaseEngine,
    backend: &dyn MailBackend,
    filter_engine: &FilterEngine,
) -> Result<usize, SyncError> {
    let folders = backend.sync_folders().await?;
    let mut synced = 0usize;
    for folder in folders {
        let (emails, _state) = backend.sync_messages(&folder.id, None).await?;
        for email in emails {
            db.save_email(&email).await?;
            apply_filter_actions(db, filter_engine, &email).await;
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
    filter_engine: &FilterEngine,
    account_id: Option<String>,
) -> Result<usize, SyncError> {
    match &account_id {
        Some(id) => event_bus.process_command(CoreCommand::SyncAccount {
            account_id: id.clone(),
        }),
        None => event_bus.process_command(CoreCommand::SyncAll),
    }

    let result = fetch_and_persist(db, backend, filter_engine).await;

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
    filter_engine: &FilterEngine,
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
                filter_engine,
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
    filter_engine: &FilterEngine,
) -> usize {
    event_bus.process_command(CoreCommand::SyncAll);

    let mut total_synced = 0usize;
    match db.list_accounts().await {
        Ok(accounts) => {
            for config in accounts {
                match secrets.get_secret(&config.keyring_secret_key) {
                    Ok(password) => {
                        let backend = build_mail_backend(&config, &password);
                        match fetch_and_persist(db, backend.as_ref(), filter_engine).await {
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

    /// A `FilterEngine` with no rules, for tests that exercise sync
    /// mechanics unrelated to filtering.
    fn empty_filter_engine() -> FilterEngine {
        FilterEngine::new(Vec::new()).expect("empty rule set")
    }

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

        let filter_engine = empty_filter_engine();
        let synced = sync_with_backend(
            &db,
            &event_bus,
            &mock,
            &filter_engine,
            Some("acct-mock-1".to_string()),
        )
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

        let filter_engine = empty_filter_engine();
        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None)
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

        let filter_engine = empty_filter_engine();
        let err = sync_with_backend(
            &db,
            &event_bus,
            &mock,
            &filter_engine,
            Some("acct-fail".to_string()),
        )
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

        let filter_engine = empty_filter_engine();
        let synced = run_account_sync(
            &db,
            &secrets,
            &event_bus,
            &filter_engine,
            "acct-jmap-real-1",
        )
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

        let filter_engine = empty_filter_engine();
        let err = run_account_sync(
            &db,
            &secrets,
            &event_bus,
            &filter_engine,
            "acct-does-not-exist",
        )
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

        let filter_engine = empty_filter_engine();
        let err = run_account_sync(&db, &secrets, &event_bus, &filter_engine, "acct-no-cred-1")
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

        let filter_engine = empty_filter_engine();
        let total = run_all_accounts_sync(&db, &secrets, &event_bus, &filter_engine).await;
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

        let filter_engine = empty_filter_engine();
        let total = run_all_accounts_sync(&db, &secrets, &event_bus, &filter_engine).await;
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

        let filter_engine = empty_filter_engine();
        let total = run_all_accounts_sync(&db, &secrets, &event_bus, &filter_engine).await;
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

    #[tokio::test]
    async fn sync_with_backend_fires_immediate_mark_read_action_on_match() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();

        let rule = nuncio_filter::NsqlParser::parse_rule(
            "Urgent Auto-Read",
            1,
            "WHERE subject CONTAINS 'Urgent' ACTION MARK READ",
        )
        .expect("parse rule");
        let filter_engine = FilterEngine::new(vec![rule]).expect("compile rule");

        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 1,
            unread_messages: 1,
        });
        mock.add_message(mock_email("m-urgent", "inbox", "Urgent: server down"));

        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 1);

        let stored = db.get_message("m-urgent").await.expect("message persisted");
        assert!(
            stored.read,
            "MARK READ action must genuinely flip the stored read flag"
        );

        let logs = db
            .list_filter_execution_logs(10)
            .await
            .expect("list execution logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message_id, "m-urgent");

        let pending = db
            .list_pending_mutations(10)
            .await
            .expect("list pending mutations");
        assert!(
            pending.is_empty(),
            "an immediate action must not enqueue an outbox mutation"
        );
    }

    #[tokio::test]
    async fn sync_with_backend_enqueues_outbox_mutation_for_remote_action_on_match() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();

        let rule = nuncio_filter::NsqlParser::parse_rule(
            "Archive Rule",
            1,
            "WHERE subject CONTAINS 'Archive Me' ACTION MOVE TO 'Archive'",
        )
        .expect("parse rule");
        let filter_engine = FilterEngine::new(vec![rule]).expect("compile rule");

        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 1,
            unread_messages: 1,
        });
        mock.add_message(mock_email("m-archive", "inbox", "Please Archive Me"));

        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 1);

        let pending = db
            .list_pending_mutations(10)
            .await
            .expect("list pending mutations");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message_id, "m-archive");
        assert_eq!(pending[0].mutation_type, "MOVE");
        assert_eq!(pending[0].status, "pending");
        assert!(pending[0].payload.contains("Archive"));

        let logs = db
            .list_filter_execution_logs(10)
            .await
            .expect("list execution logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message_id, "m-archive");

        // The remote action must not have applied a local read-flag change.
        let stored = db
            .get_message("m-archive")
            .await
            .expect("message persisted");
        assert!(!stored.read);
    }

    #[tokio::test]
    async fn sync_with_backend_non_matching_message_produces_no_filter_side_effects() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();

        let rule = nuncio_filter::NsqlParser::parse_rule(
            "Urgent Auto-Read",
            1,
            "WHERE subject CONTAINS 'Urgent' ACTION MARK READ",
        )
        .expect("parse rule");
        let filter_engine = FilterEngine::new(vec![rule]).expect("compile rule");

        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 1,
            unread_messages: 1,
        });
        mock.add_message(mock_email("m-plain", "inbox", "Just a normal update"));

        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 1);

        let stored = db.get_message("m-plain").await.expect("message persisted");
        assert!(!stored.read, "non-matching message must stay untouched");

        let logs = db
            .list_filter_execution_logs(10)
            .await
            .expect("list execution logs");
        assert!(logs.is_empty(), "no rule matched, so no log should exist");

        let pending = db
            .list_pending_mutations(10)
            .await
            .expect("list pending mutations");
        assert!(
            pending.is_empty(),
            "no rule matched, so no mutation should be enqueued"
        );
    }
}
