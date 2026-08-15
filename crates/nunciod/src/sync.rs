//! Real inbound mail synchronization routine.
//!
//! Prior to this module, "sync" was a status-flag flip: `CoreCommand::SyncAll`
//! / `CoreCommand::SyncAccount` only moved `EngineStatus` to `Syncing` and
//! published `SyncStarted`, and nothing ever fetched a real message from a
//! real server into the store. This module closes that gap: for a configured
//! account it connects a real [`MailBackend`] (a real [`ImapEngine`] for an
//! IMAP/SMTP transport, built from [`AccountConfig`] plus the
//! account's keyring-stored password), fetches folders and messages, and
//! persists every message, together with the mailbox occupancy it was found
//! in, through [`DatabaseEngine::save_email_at`].
//!
//! # Testability
//!
//! [`sync_with_backend`] takes `&dyn MailBackend` rather than a concrete
//! engine, so it is exercised directly in tests with [`MockMailBackend`] --
//! no real network or OS keyring access ever happens inside it. Production
//! entry point [`run_account_sync`] is the only place that resolves a real
//! account's credentials and constructs a real backend; it then delegates the
//! actual fetch/persist/event-emission work to [`sync_with_backend`]. An
//! all-accounts sync fans these per-account runs out through
//! [`crate::sync_dispatcher::sync_all_configured`], so concurrency and the
//! per-account timeout are bounded in one place rather than serialized here.
use nuncio_core::model::PlacementKey;
use nuncio_core::{AccountConfig, CoreCommand, CoreEvent, EventBus, Transport};
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
    /// The account's protocol is not a mail protocol (e.g. a CalDAV account),
    /// so it has no inbound mail backend to sync.
    #[error("account '{0}' is not a mail account and cannot be mail-synced")]
    NotAMailAccount(String),
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

/// Evaluate `email` **as it sits in `placement`** against `filter_engine` and
/// route every matched rule's actions to their real, persisted effect.
///
/// Evaluation is per placement because the rule language can ask where a
/// message is: `WHERE FOLDER = 'INBOX'` has no answer for a message that
/// occupies three mailboxes, so the caller fans out over the occupancies and
/// each call answers for exactly one.
///
/// Acting, in contrast, is once per (rule, message). Every matched rule must
/// win [`DatabaseEngine::claim_filter_fire`] before any of its actions run, and
/// the claim is keyed on message identity, not on the placement. `MOVE` and
/// `FLAG` converge under repetition -- ten moves leave one net effect -- but
/// `FORWARD` and `CALL WEBHOOK` land on third parties and have no shared state
/// to converge against: one message in ten folders would page an on-call
/// engineer ten times. The claim fails closed, so an unverifiable claim skips
/// the actions rather than licensing them.
///
/// The claim is spent the moment it is won, before the actions are known to
/// have succeeded. A rule whose enqueue then fails is therefore not retried on
/// a later pass. That is the deliberate trade: re-firing FORWARD and CALL
/// WEBHOOK on a partial failure would re-deliver to a third party, and a
/// duplicate mail cannot be un-received, while a dropped one is visible in the
/// warning below and recoverable by hand.
///
/// The claim also subsumes the older "only call me for a new message" contract
/// for re-arrivals of the same message. Callers should still gate on a genuinely
/// new occupancy (see [`fetch_and_persist`]) -- the claim stops repeat *actions*,
/// not the wasted evaluation of a mailbox nothing changed in.
///
/// `MarkRead`/`MarkUnread` apply immediately to the occupancy that was
/// evaluated, via [`DatabaseEngine::set_placement_read`]: IMAP `\Seen` is
/// per-mailbox, so "mark this read" is only well-defined once it names a
/// mailbox. Every other action enqueues a real
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
/// store) need, so it takes only a `DatabaseEngine`/`FilterEngine`/`Email`/
/// `Placement` and has no dependency on the live inbound-sync call chain.
pub async fn apply_filter_actions(
    db: &nuncio_store::db::DatabaseEngine,
    filter_engine: &FilterEngine,
    email: &nuncio_core::model::Email,
    placement: &nuncio_core::model::Placement,
) -> usize {
    let mut applied = 0usize;
    let placed = nuncio_filter::PlacedEmail { email, placement };

    for (rule, actions) in filter_engine.evaluate(placed) {
        // Claim before acting, never after. The claim is keyed on message
        // identity, so the same mail reaching a second mailbox -- an ordinary
        // MOVE, or a Gmail label -- evaluates again but acts only once.
        match db.claim_filter_fire(&rule.id, &email.id).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(
                    rule_id = %rule.id,
                    message_key = %email.id,
                    folder_id = %placement.folder_id,
                    "rule already fired for this message; skipping actions for this placement"
                );
                continue;
            }
            Err(e) => {
                // Fail closed. An unverifiable claim must not become a licence
                // to act: a lost claim check on a FORWARD is a duplicate mail
                // the recipient cannot un-receive.
                tracing::error!(
                    rule_id = %rule.id,
                    message_key = %email.id,
                    error = %e,
                    "could not claim the filter fire; skipping actions"
                );
                continue;
            }
        }

        let placement_key = placement.key();
        for action in actions {
            let immediate_ok = match &action {
                // One write with a boolean rather than two near-identical
                // arms. Both name the occupancy that was evaluated, because
                // `\Seen` is per-mailbox: a message read in INBOX is not
                // thereby read in Archive.
                RuleAction::MarkRead | RuleAction::MarkUnread => {
                    let read = matches!(action, RuleAction::MarkRead);
                    let label = if read { "MARK READ" } else { "MARK UNREAD" };
                    match db.set_placement_read(&placement_key, read).await {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::warn!(
                                "filter rule '{}' {label} failed for message '{}' in folder '{}': {e}",
                                rule.id,
                                email.id,
                                placement.folder_id
                            );
                            false
                        }
                    }
                }
                RuleAction::MoveTo(_)
                | RuleAction::CopyTo(_)
                | RuleAction::Flag
                | RuleAction::Unflag
                | RuleAction::Delete
                | RuleAction::ForwardTo(_)
                | RuleAction::CallWebhook(_) => {
                    // Remote actions are handled below via the outbox; this
                    // arm only tracks whether the immediate-action write
                    // itself succeeded (there is none for a remote action,
                    // so it is unconditionally "not applicable" here). Listed
                    // explicitly rather than a wildcard so a future
                    // immediate-style action can't silently fall through to
                    // remote routing by accident.
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

/// Fetch every folder from `backend`, persist each reported occupancy via
/// [`DatabaseEngine::save_email_at`], evaluate the genuinely-new occupancies
/// against `filter_engine` through [`apply_filter_actions`], and delete the
/// occupancies the server no longer reports.
///
/// # Why classification is per placement, not per message
///
/// Message identity is folder-independent: a message already stored from
/// `INBOX` has the same key when it turns up in `Archive`. Asking "have I seen
/// this *message*?" therefore answers `yes` for a mailbox this daemon has never
/// filtered, and the arrival silently fires nothing -- no error, no log. The
/// question that matches the intent is "have I seen this *occupancy*?", so this
/// classifies on [`PlacementKey`] via
/// [`DatabaseEngine::existing_placements`].
///
/// Classification happens BEFORE the write, because `save_email_at` upserts and
/// would otherwise erase the distinction between a first arrival and a
/// re-report. One batched lookup covers the whole chunk rather than an N+1 of
/// per-row existence checks, and a per-pass "already seen" set folds in on top
/// so a backend that surfaces the same occupancy twice in one fetch still
/// classifies it as new exactly once.
///
/// Firing once per *arrival* is not the same as acting once per *message*; the
/// second guarantee lives in the claim inside [`apply_filter_actions`].
///
/// # Absence
///
/// [`nuncio_mail::FolderChanges::present`] is honoured as written: `None` means
/// the pass was incremental and cannot speak to absence, and treating it as
/// "nothing is present" would delete the whole folder. Only `Some(list)` --
/// which `nuncio-mail` withholds when a SELECT omitted UIDVALIDITY, since the
/// keys it could build would name no stored row -- licenses deleting the stored
/// placements missing from it.
///
/// Returns the total number of occupancies processed: one message reported from
/// two folders counts twice, because two mailboxes really did report it.
///
/// Contains no event-bus or credential logic -- pure fetch-and-persist, so
/// it is the smallest unit tests can exercise directly with a
/// [`MockMailBackend`].
async fn fetch_and_persist(
    db: &nuncio_store::db::DatabaseEngine,
    event_bus: &EventBus,
    backend: &dyn MailBackend,
    filter_engine: &FilterEngine,
    account_id: Option<&str>,
    filters_enabled: bool,
) -> Result<usize, SyncError> {
    let folders = backend.sync_folders().await?;
    let mut synced = 0usize;
    for folder in folders {
        // Resume from the folder's stored checkpoint so the backend can fetch
        // only what changed since the last sync. A checkpoint is keyed by
        // (account, folder), so a run with no account context (a mock-driven
        // `SyncAll` with `account_id == None`) has nowhere to key it and always
        // performs a full fetch.
        let last_state = match account_id {
            Some(acct) => db.get_folder_sync_state(acct, &folder.id).await?,
            None => None,
        };
        let changes = backend
            .sync_changes(&folder.id, last_state.as_deref())
            .await?;
        let placed = changes.upserts;
        let fetched = placed.len();
        let new_state = changes.next_state;

        // One round trip classifies the whole chunk instead of one lookup per
        // occupancy. This asks about *placements*, not messages: identity is
        // folder-independent, so a message already stored from another folder
        // is still a first arrival here, and testing the message key instead
        // would classify it as seen and skip filtering it entirely. A lookup
        // failure that is not "not found" surfaces as `Err` rather than
        // silently reading as "not new", so nothing is persisted and
        // skip-filtered on an unverified assumption.
        let chunk_keys: Vec<PlacementKey> = placed.iter().map(|p| p.placement.key()).collect();
        let already_present = db.existing_placements(&chunk_keys).await?;
        let mut seen_this_pass = std::collections::HashSet::new();

        for message in placed {
            let key = message.placement.key();
            let is_new = !already_present.contains(&key) && seen_this_pass.insert(key);
            db.save_email_at(&message.email, message.source, &message.placement)
                .await?;
            // Filter execution is single-owner per account (see
            // `AccountConfig::filters_enabled`). A daemon that is not the owner
            // still syncs and stores the message; it just does not act on it,
            // because the side-effecting actions -- FORWARD, CALL WEBHOOK --
            // are not idempotent across daemons. The gate sits outside the
            // fire-once claim on purpose: a non-owner must not consume the
            // claim it is declining to act on.
            if is_new && filters_enabled {
                apply_filter_actions(db, filter_engine, &message.email, &message.placement).await;
            }
            synced += 1;
        }
        // Reconcile what the server no longer has. Until this existed, a sync
        // could only ever add: a message moved or deleted by any other client
        // -- another daemon, a phone, webmail -- stayed in the local store
        // forever, and every account accumulated ghosts.
        //
        // What a folder stops mentioning is an occupancy, never a message: the
        // same mail may still sit in another mailbox, and it goes only when its
        // last placement does -- which `delete_placements` decides, not this.
        //
        // A `PlacementKey` names its own account, so an explicitly-reported
        // removal is actionable with no ambient account context at all. Only
        // the `present` diff needs one, because its other half is a lookup
        // keyed by (account, folder).
        let mut gone: Vec<PlacementKey> = changes.removals;

        // `present` is the folder's complete contents when the pass was
        // able to enumerate them. `None` and `Some(vec![])` are NOT the
        // same: `None` means the pass was incremental and cannot speak to
        // absence, and treating it as "nothing is present" would delete
        // the folder.
        if let (Some(acct), Some(present)) = (account_id, changes.present) {
            let present: std::collections::HashSet<PlacementKey> = present.into_iter().collect();
            let stored = db.placements_in_folder(acct, &folder.id).await?;
            gone.extend(stored.into_iter().filter(|key| !present.contains(key)));
        }

        gone.sort_unstable();
        gone.dedup();
        if !gone.is_empty() {
            let outcome = db.delete_placements(&gone).await?;
            tracing::info!(
                account_id = ?account_id,
                folder_id = %folder.id,
                placements_removed = outcome.placements_removed,
                messages_reaped = outcome.messages_reaped,
                "removed placements that are no longer on the server"
            );
        }

        // Persist the returned checkpoint only AFTER this folder's messages
        // have landed, so the stored high-water mark can never advance past
        // work that actually reached the store.
        if let Some(acct) = account_id {
            db.save_folder_sync_state(acct, &folder.id, &new_state)
                .await?;
        }
        event_bus.publish_event(CoreEvent::SyncProgress {
            account_id: account_id.map(str::to_string),
            folder_id: folder.id.clone(),
            fetched,
            total: synced,
        });
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
    filters_enabled: bool,
) -> Result<usize, SyncError> {
    match &account_id {
        Some(id) => event_bus.process_command(CoreCommand::SyncAccount {
            account_id: id.clone(),
        }),
        None => event_bus.process_command(CoreCommand::SyncAll),
    }

    let result = fetch_and_persist(
        db,
        event_bus,
        backend,
        filter_engine,
        account_id.as_deref(),
        filters_enabled,
    )
    .await;

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
pub(crate) fn build_mail_backend(
    config: &AccountConfig,
    password: &str,
) -> Result<Box<dyn MailBackend>, SyncError> {
    match &config.transport {
        Transport::ImapSmtp(t) => Ok(Box::new(ImapEngine::with_credentials(
            &config.id,
            &t.imap_host,
            t.imap_port,
            t.imap_tls_mode,
            &config.email_address,
            password,
        ))),
        Transport::Jmap(t) => Ok(Box::new(JmapEngine::with_credentials(
            &config.id,
            &t.endpoint_host,
            &config.email_address,
            password,
        ))),
        // A DAV-protocol account (e.g. CalDAV) has no inbound mail backend;
        // it is synced through its own domain service, not the mail path.
        Transport::Dav(_) | Transport::CardDav(_) => {
            Err(SyncError::NotAMailAccount(config.id.clone()))
        }
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
        let filters_enabled = config.filters_enabled;
        build_mail_backend(&config, &password).map(|backend| (backend, filters_enabled))
    }
    .await;

    match setup {
        Ok((backend, filters_enabled)) => {
            sync_with_backend(
                db,
                event_bus,
                backend.as_ref(),
                filter_engine,
                Some(account_id.to_string()),
                filters_enabled,
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

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::model::{Email, Folder, IdentitySource, Placement};
    use nuncio_core::{CoreEvent, EngineStatus, TlsMode};
    use nuncio_mail::{MockMailBackend, PlacedMessage};
    use nuncio_store::db::DatabaseEngine;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A `FilterEngine` with no rules, for tests that exercise sync
    /// mechanics unrelated to filtering.
    fn empty_filter_engine() -> FilterEngine {
        FilterEngine::new(Vec::new()).expect("empty rule set")
    }

    fn sample_jmap_account(id: &str) -> AccountConfig {
        sample_jmap_account_at(id, "jmap.nuncio.mx")
    }

    fn sample_jmap_account_at(id: &str, host: &str) -> AccountConfig {
        AccountConfig {
            id: id.to_string(),
            name: "JMAP Test Account".to_string(),
            email_address: format!("{id}@nuncio.mx"),
            keyring_secret_key: format!("nuncio/{id}"),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: Transport::Jmap(nuncio_core::JmapTransport {
                endpoint_host: host.to_string(),
            }),
        }
    }

    /// Mount a full JMAP session-discovery + `Mailbox/get` + `Email/query` + `Email/get`
    /// stub set on `mock_server`, serving exactly one folder ("jmap-inbox") containing one
    /// message ("jmap-msg-1"). Used by tests that need `JmapEngine::with_credentials` to
    /// complete a real (wiremock-backed) fetch through `MailBackend`.
    async fn mount_single_message_jmap_stubs(mock_server: &MockServer, account_id: &str) {
        let session_body = json!({
            "username": format!("{account_id}@nuncio.mx"),
            "primaryAccounts": { "urn:ietf:params:jmap:mail": account_id },
            "apiUrl": format!("{}/jmap/api", mock_server.uri()),
            "state": "session-state-1"
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/jmap"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&session_body))
            .mount(mock_server)
            .await;

        let mailbox_get_body = json!({
            "methodResponses": [[
                "Mailbox/get",
                { "list": [ { "id": "jmap-inbox", "name": "Inbox", "totalEmails": 1, "unreadEmails": 1 } ] },
                "c1"
            ]]
        });
        Mock::given(method("POST"))
            .and(path("/jmap/api"))
            .and(body_partial_json(
                json!({"methodCalls": [["Mailbox/get", {}, "c1"]]}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(&mailbox_get_body))
            .mount(mock_server)
            .await;

        let email_query_body = json!({
            "methodResponses": [["Email/query", {"ids": ["jmap-msg-1"]}, "c1"]]
        });
        Mock::given(method("POST"))
            .and(path("/jmap/api"))
            .and(body_partial_json(
                json!({"methodCalls": [["Email/query", {}, "c1"]]}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(&email_query_body))
            .mount(mock_server)
            .await;

        let email_get_body = json!({
            "methodResponses": [[
                "Email/get",
                {
                    "state": "sync-state-1",
                    "list": [{
                        "id": "jmap-msg-1",
                        "subject": "Welcome to JMAP Sync",
                        "from": [{ "email": "support@nuncio.mx" }],
                        "to": [{ "email": format!("{account_id}@nuncio.mx") }],
                        "receivedAt": 1700000000i64,
                        "isUnread": true,
                        "bodySnippet": "Real wiremock-backed JMAP fetch."
                    }]
                },
                "c1"
            ]]
        });
        Mock::given(method("POST"))
            .and(path("/jmap/api"))
            .and(body_partial_json(
                json!({"methodCalls": [["Email/get", {}, "c1"]]}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(&email_get_body))
            .mount(mock_server)
            .await;
    }

    fn sample_imap_account(id: &str) -> AccountConfig {
        AccountConfig {
            id: id.to_string(),
            name: "IMAP Test Account".to_string(),
            email_address: format!("{id}@nuncio.mx"),
            keyring_secret_key: format!("nuncio/{id}"),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                imap_host: "imap.nuncio.mx".to_string(),
                imap_port: 993,
                imap_tls_mode: TlsMode::ImplicitTls,
                smtp_host: "smtp.nuncio.mx".to_string(),
                smtp_port: 465,
                smtp_tls_mode: TlsMode::ImplicitTls,
            }),
        }
    }

    /// One mock message as a backend would surface it: identity plus the
    /// occupancy it was found in. `id` doubles as the folder-scoped UID, which
    /// keeps distinct ids in one folder distinct occupancies.
    fn mock_placed(id: &str, folder_id: &str, subject: &str) -> PlacedMessage {
        PlacedMessage {
            email: Email {
                id: id.to_string(),
                account_id: "acct-mock-1".to_string(),
                subject: subject.to_string(),
                sender: "alice@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: 1_700_000_000,
                body_plain: Some(format!("body for {id}")),
                body_html: None,
                attachments: Vec::new(),
                message_id: None,
                content_hash: None,
            },
            source: IdentitySource::Surrogate,
            placement: Placement {
                account_id: "acct-mock-1".to_string(),
                folder_id: folder_id.to_string(),
                uid_validity: "1".to_string(),
                remote_id: id.to_string(),
                read: false,
            },
        }
    }

    /// The read flag of a message's one occupancy, for the single-placement
    /// mock fixtures. Fails loudly rather than defaulting if the message turns
    /// out to sit in no mailbox at all -- "unread" and "not there" are
    /// different answers and must not be conflated by a test helper.
    async fn read_flag_of(db: &DatabaseEngine, message_key: &str) -> bool {
        let placements = db
            .placements_of(message_key)
            .await
            .expect("read the message's placements");
        assert_eq!(
            placements.len(),
            1,
            "fixture expects exactly one occupancy of '{message_key}'"
        );
        placements[0].read
    }

    /// The message key every [`MockMailBackend::with_same_message_in`]
    /// occupancy carries.
    fn shared_key() -> &'static str {
        MockMailBackend::SHARED_MESSAGE_KEY
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
        mock.add_message(mock_placed("m1", "inbox", "Hello"));
        mock.add_message(mock_placed("m2", "inbox", "World"));

        let filter_engine = empty_filter_engine();
        let synced = sync_with_backend(
            &db,
            &event_bus,
            &mock,
            &filter_engine,
            Some("acct-mock-1".to_string()),
            true,
        )
        .await
        .expect("sync succeeds");
        assert_eq!(synced, 2);

        let persisted = db
            .list_messages("acct-mock-1", "inbox", 10)
            .await
            .expect("list persisted messages");
        assert_eq!(persisted.len(), 2);
        let subjects: Vec<String> = persisted
            .iter()
            .map(|(read, _placement)| {
                read.as_ref()
                    .expect("fixture body is readable")
                    .subject
                    .clone()
            })
            .collect();
        assert!(subjects.contains(&"Hello".to_string()));
        assert!(subjects.contains(&"World".to_string()));

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted {
                account_id: Some("acct-mock-1".to_string())
            }
        );
        // A per-folder progress event is published between start and complete.
        assert_eq!(
            events.recv().await.expect("progress event"),
            CoreEvent::SyncProgress {
                account_id: Some("acct-mock-1".to_string()),
                folder_id: "inbox".to_string(),
                fetched: 2,
                total: 2,
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
        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None, true)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 0);

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted { account_id: None }
        );
        // The folder is empty, but a progress event still marks it done.
        assert_eq!(
            events.recv().await.expect("progress event"),
            CoreEvent::SyncProgress {
                account_id: None,
                folder_id: "inbox".to_string(),
                fetched: 0,
                total: 0,
            }
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
            true,
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
        // Uses a real (non-mock) `JmapEngine::with_credentials`, wired exactly like
        // `build_mail_backend` wires it in production, pointed at a wiremock server standing
        // in for the real JMAP host. This proves `run_account_sync`'s full production wiring
        // (account lookup -> keyring password -> real HTTP backend -> persist -> events)
        // end-to-end without violating the "no live network calls in tests" rule.
        let mock_server = MockServer::start().await;
        mount_single_message_jmap_stubs(&mock_server, "acct-jmap-real-1").await;

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();
        let secrets = SecretManager::mock();

        let config = sample_jmap_account_at("acct-jmap-real-1", &mock_server.uri());
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
        // The wiremock stubs advertise 1 folder containing 1 message.
        assert_eq!(synced, 1);

        // The message persists under an opaque surrogate id; it is recovered by
        // the mailbox it was actually synced from, and its protocol-native JMAP
        // object id round-trips in `remote_id`.
        let persisted = db
            .list_messages("acct-jmap-real-1", "jmap-inbox", 10)
            .await
            .expect("jmap message persisted");
        assert_eq!(persisted.len(), 1);
        let (email, placement) = &persisted[0];
        assert_eq!(
            email.as_ref().expect("fixture body is readable").subject,
            "Welcome to JMAP Sync"
        );
        assert_eq!(placement.remote_id, "jmap-msg-1");

        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted {
                account_id: Some("acct-jmap-real-1".to_string())
            }
        );
        assert_eq!(
            events.recv().await.expect("progress event"),
            CoreEvent::SyncProgress {
                account_id: Some("acct-jmap-real-1".to_string()),
                folder_id: "jmap-inbox".to_string(),
                fetched: 1,
                total: 1,
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
        mock.add_message(mock_placed("m-urgent", "inbox", "Urgent: server down"));

        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None, true)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 1);

        assert!(
            read_flag_of(&db, "m-urgent").await,
            "MARK READ action must genuinely flip the occupancy's read flag"
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
    async fn a_daemon_that_does_not_own_filters_stores_the_mail_but_fires_nothing() {
        // The single-owner rule. A non-owning daemon must still sync and store
        // -- it is a full mail client -- while producing none of the side
        // effects that would be duplicated across the fleet.
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
        mock.add_message(mock_placed("m-urgent", "inbox", "Urgent: server down"));

        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None, false)
            .await
            .expect("sync succeeds");

        assert_eq!(synced, 1, "the message must still be synced and stored");
        assert!(
            !read_flag_of(&db, "m-urgent").await,
            "a non-owning daemon must not apply MARK READ"
        );
        assert!(
            db.list_filter_execution_logs(10)
                .await
                .expect("list logs")
                .is_empty(),
            "no rule ran, so nothing may be logged as executed"
        );
        assert!(
            db.list_pending_mutations(10)
                .await
                .expect("list mutations")
                .is_empty(),
            "a non-owning daemon must enqueue no remote mutation"
        );
    }

    #[tokio::test]
    async fn run_account_sync_takes_filter_ownership_from_the_stored_account() {
        // The flag has to come from the account row, not a caller's guess --
        // otherwise the gate is only ever exercised by tests that pass it.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");

        let mut account = sample_imap_account("acct-owner");
        assert!(
            !account.filters_enabled,
            "a freshly constructed account must default to not owning filters"
        );
        db.save_account(&account).await.expect("save account");
        let reloaded = db
            .list_accounts()
            .await
            .expect("list accounts")
            .into_iter()
            .find(|a| a.id == "acct-owner")
            .expect("account round-trips");
        assert!(!reloaded.filters_enabled);

        account.filters_enabled = true;
        db.save_account(&account).await.expect("update account");
        let reloaded = db
            .list_accounts()
            .await
            .expect("list accounts")
            .into_iter()
            .find(|a| a.id == "acct-owner")
            .expect("account round-trips");
        assert!(
            reloaded.filters_enabled,
            "opting one daemon in must persist"
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
        mock.add_message(mock_placed("m-archive", "inbox", "Please Archive Me"));

        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None, true)
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
        assert!(!read_flag_of(&db, "m-archive").await);
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
        mock.add_message(mock_placed("m-plain", "inbox", "Just a normal update"));

        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None, true)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 1);

        assert!(
            !read_flag_of(&db, "m-plain").await,
            "non-matching message must stay untouched"
        );

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

    #[tokio::test]
    async fn sync_with_backend_does_not_refire_filters_on_a_repeat_sync_of_the_same_message() {
        // Sync is not yet incremental: a real backend re-reports messages it
        // already handed over on every sync pass. This proves a message's
        // filter actions fire exactly once across repeated syncs rather than
        // once per sync cycle -- the regression this guard exists for.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();

        let mark_read_rule = nuncio_filter::NsqlParser::parse_rule(
            "Urgent Auto-Read",
            1,
            "WHERE subject CONTAINS 'Urgent' ACTION MARK READ",
        )
        .expect("parse rule");
        let move_rule = nuncio_filter::NsqlParser::parse_rule(
            "Archive Rule",
            2,
            "WHERE subject CONTAINS 'Urgent' ACTION MOVE TO 'Archive'",
        )
        .expect("parse rule");
        let filter_engine =
            FilterEngine::new(vec![mark_read_rule, move_rule]).expect("compile rules");

        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 1,
            unread_messages: 1,
        });
        let repeat = mock_placed("m-repeat", "inbox", "Urgent: server down");
        let repeat_key = repeat.placement.key();
        mock.add_message(repeat);

        // First sync: the message is genuinely new, so both rules must fire.
        let synced_first = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None, true)
            .await
            .expect("first sync succeeds");
        assert_eq!(synced_first, 1);

        assert!(read_flag_of(&db, "m-repeat").await);

        let logs_after_first = db
            .list_filter_execution_logs(10)
            .await
            .expect("list execution logs after first sync");
        assert_eq!(logs_after_first.len(), 2, "both rules should fire once");

        let pending_after_first = db
            .list_pending_mutations(10)
            .await
            .expect("list pending mutations after first sync");
        assert_eq!(pending_after_first.len(), 1);

        // Manually flip the message back to unread, exactly as a user
        // reading and then un-reading it would -- proves the second sync
        // doesn't re-flip it back to read via a repeat MARK READ fire.
        db.set_placement_read(&repeat_key, false)
            .await
            .expect("manually mark unread");

        // Second sync: the mock backend reports the SAME message again
        // (unchanged id), simulating a non-incremental re-sync.
        let synced_second = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None, true)
            .await
            .expect("second sync succeeds");
        assert_eq!(synced_second, 1);

        assert!(
            !read_flag_of(&db, "m-repeat").await,
            "a re-sync of an already-seen message must not re-fire MARK READ"
        );

        let logs_after_second = db
            .list_filter_execution_logs(10)
            .await
            .expect("list execution logs after second sync");
        assert_eq!(
            logs_after_second.len(),
            2,
            "no new execution log entries should be written on a repeat sync"
        );

        let pending_after_second = db
            .list_pending_mutations(10)
            .await
            .expect("list pending mutations after second sync");
        assert_eq!(
            pending_after_second.len(),
            1,
            "no new outbox mutation should be enqueued on a repeat sync"
        );
    }

    #[tokio::test]
    async fn sync_with_backend_fires_filters_only_for_new_messages_in_a_mixed_batch() {
        // A single fetched chunk containing a mix of an already-stored
        // occupancy (persisted by a prior sync) and genuinely new ones must
        // fire filter actions ONLY for the new ones, via the batched
        // `existing_placements` classification -- not a per-row lookup.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();

        let mark_read_rule = nuncio_filter::NsqlParser::parse_rule(
            "Urgent Auto-Read",
            1,
            "WHERE subject CONTAINS 'Urgent' ACTION MARK READ",
        )
        .expect("parse rule");
        let filter_engine = FilterEngine::new(vec![mark_read_rule]).expect("compile rule");

        // Pre-seed one occupancy directly, simulating it having landed in a
        // prior sync pass. It must NOT re-fire even though this pass's
        // backend still reports it (sync is not yet incremental).
        let seeded = mock_placed("m-already-seen", "inbox", "Urgent: known issue");
        db.save_email_at(&seeded.email, seeded.source, &seeded.placement)
            .await
            .expect("pre-seed existing occupancy");

        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 3,
            unread_messages: 3,
        });
        // Mixed chunk: one already-present id, two genuinely new ids.
        mock.add_message(mock_placed(
            "m-already-seen",
            "inbox",
            "Urgent: known issue",
        ));
        mock.add_message(mock_placed("m-new-1", "inbox", "Urgent: brand new"));
        mock.add_message(mock_placed("m-new-2", "inbox", "Urgent: also new"));

        let synced = sync_with_backend(&db, &event_bus, &mock, &filter_engine, None, true)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 3, "every fetched message is still persisted");

        let logs = db
            .list_filter_execution_logs(10)
            .await
            .expect("list execution logs");
        assert_eq!(
            logs.len(),
            2,
            "filters fire only for the two genuinely new messages in the mixed batch"
        );
        let fired_ids: std::collections::HashSet<String> =
            logs.iter().map(|l| l.message_id.clone()).collect();
        assert!(fired_ids.contains("m-new-1"));
        assert!(fired_ids.contains("m-new-2"));
        assert!(!fired_ids.contains("m-already-seen"));

        // The pre-seeded occupancy must still be persisted (the upsert is
        // unaffected by the batched new/seen classification) but its read flag
        // must be untouched by this pass's MARK READ rule.
        assert!(
            !read_flag_of(&db, "m-already-seen").await,
            "an occupancy that already existed before this sync must not re-fire MARK READ"
        );
    }

    #[tokio::test]
    async fn sync_with_backend_persists_and_threads_the_folder_checkpoint_across_syncs() {
        // The regression this story exists to kill: the returned checkpoint
        // must be persisted and fed back into the NEXT sync, so the second
        // sync is genuinely incremental rather than a full re-fetch. Proven by
        // capturing the exact `since_state` the backend received on each call.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();

        let mock = MockMailBackend::new();
        mock.set_returned_state("uidnext-4242");
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 1,
            unread_messages: 1,
        });
        mock.add_message(mock_placed("m-inc-1", "inbox", "First"));

        let filter_engine = empty_filter_engine();

        // First sync: no checkpoint exists yet, so the backend is called with
        // `None` and does a full fetch (one message).
        let first = sync_with_backend(
            &db,
            &event_bus,
            &mock,
            &filter_engine,
            Some("acct-inc-1".to_string()),
            true,
        )
        .await
        .expect("first sync succeeds");
        assert_eq!(first, 1);

        // The returned checkpoint must now be persisted for (account, folder).
        assert_eq!(
            db.get_folder_sync_state("acct-inc-1", "inbox")
                .await
                .expect("read checkpoint"),
            Some("uidnext-4242".to_string())
        );

        // Second sync: the daemon must feed the stored checkpoint back in. The
        // mock returns nothing new for a non-None checkpoint -> narrower fetch.
        let second = sync_with_backend(
            &db,
            &event_bus,
            &mock,
            &filter_engine,
            Some("acct-inc-1".to_string()),
            true,
        )
        .await
        .expect("second sync succeeds");
        assert_eq!(second, 0, "an incremental second sync fetches nothing new");

        // The proof of genuine incrementality: the backend saw `None` first,
        // then the exact checkpoint the first sync returned.
        assert_eq!(
            mock.since_state_calls(),
            vec![None, Some("uidnext-4242".to_string())],
        );
    }

    /// A rule that matches the one message the shared-identity mock stages.
    fn rule_matching_the_shared_message() -> nuncio_filter::FilterRule {
        nuncio_filter::NsqlParser::parse_rule(
            "Archive Shared",
            1,
            "WHERE subject CONTAINS 'Shared' ACTION MOVE TO 'Archive'",
        )
        .expect("parse rule")
    }

    #[tokio::test]
    async fn a_message_arriving_in_a_second_folder_still_gets_evaluated() {
        // The regression this guards: with identity now folder-independent, a
        // message already stored from INBOX has a known message_key, so a
        // message-scoped "already present" test would classify its arrival in
        // Archive as old and skip filtering it entirely -- silently, with no
        // error and nothing in the log.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();

        let rule = rule_matching_the_shared_message();
        let filter_engine = FilterEngine::new(vec![rule.clone()]).expect("compile rule");

        // Already stored, from INBOX. The message key is therefore known
        // before this sync begins -- but nothing has ever filtered Archive.
        let in_inbox = MockMailBackend::shared_message_in("INBOX", "1");
        db.save_email_at(&in_inbox.email, in_inbox.source, &in_inbox.placement)
            .await
            .expect("pre-seed the INBOX occupancy");

        let backend = MockMailBackend::with_same_message_in(&["Archive"]);
        let processed = fetch_and_persist(
            &db,
            &event_bus,
            &backend,
            &filter_engine,
            Some(MockMailBackend::SHARED_ACCOUNT_ID),
            true,
        )
        .await
        .expect("sync succeeds");

        assert_eq!(processed, 1, "the Archive occupancy is processed");
        let placements = db
            .placements_of(shared_key())
            .await
            .expect("read the message's placements");
        assert_eq!(
            placements.len(),
            2,
            "one message, now occupying both mailboxes"
        );

        assert!(
            db.has_filter_fired(&rule.id, shared_key())
                .await
                .expect("read the claim ledger"),
            "an arrival in a second folder must be evaluated, not skipped as already-seen"
        );
        let pending = db
            .list_pending_mutations(10)
            .await
            .expect("list pending mutations");
        assert_eq!(
            pending.len(),
            1,
            "with a real outbox mutation behind the fire, not just a ledger row"
        );
        assert_eq!(pending[0].message_id, shared_key());
    }

    #[tokio::test]
    async fn a_rule_fires_once_for_a_message_that_lands_in_two_folders() {
        // MOVE and FLAG converge under repetition; FORWARD and CALL WEBHOOK do
        // not. Two occupancies of one message must not page an on-call
        // engineer twice.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();

        let rule = rule_matching_the_shared_message();
        let filter_engine = FilterEngine::new(vec![rule.clone()]).expect("compile rule");

        let backend = MockMailBackend::with_same_message_in(&["INBOX", "Archive"]);
        let processed = fetch_and_persist(
            &db,
            &event_bus,
            &backend,
            &filter_engine,
            Some(MockMailBackend::SHARED_ACCOUNT_ID),
            true,
        )
        .await
        .expect("sync succeeds");

        assert_eq!(processed, 2, "both occupancies are processed");
        let placements = db
            .placements_of(shared_key())
            .await
            .expect("read the message's placements");
        assert_eq!(placements.len(), 2);

        assert_eq!(
            db.filter_fire_count().await.expect("read the claim ledger"),
            1,
            "one claim, therefore one set of actions"
        );
        assert!(db
            .has_filter_fired(&rule.id, shared_key())
            .await
            .expect("read the claim ledger"));

        let pending = db
            .list_pending_mutations(10)
            .await
            .expect("list pending mutations");
        assert_eq!(
            pending.len(),
            1,
            "one outbox mutation for the message, not one per folder"
        );
        let logs = db
            .list_filter_execution_logs(10)
            .await
            .expect("list execution logs");
        assert_eq!(logs.len(), 1, "and one audit entry, not one per folder");
    }

    #[tokio::test]
    async fn an_unverifiable_claim_skips_the_actions_rather_than_licensing_them() {
        // Fail closed. If the claim itself errors, the engine cannot know
        // whether this rule has already acted on this message -- and a lost
        // claim check on a FORWARD is a duplicate mail the recipient cannot
        // un-receive. Dropping the ledger table makes the claim error for real
        // while every other write still works, so a mutation appearing here
        // would be a genuine "acted without a claim", not a dead store.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");

        let rule = rule_matching_the_shared_message();
        let filter_engine = FilterEngine::new(vec![rule.clone()]).expect("compile rule");

        let placed = MockMailBackend::shared_message_in("INBOX", "1");
        db.save_email_at(&placed.email, placed.source, &placed.placement)
            .await
            .expect("store the occupancy");

        sqlx::query("DROP TABLE filter_fired")
            .execute(db.pool())
            .await
            .expect("drop the claim ledger");

        let applied =
            apply_filter_actions(&db, &filter_engine, &placed.email, &placed.placement).await;
        assert_eq!(applied, 0, "an unclaimed rule must apply nothing");

        let pending = db
            .list_pending_mutations(10)
            .await
            .expect("list pending mutations");
        assert!(
            pending.is_empty(),
            "no outbox mutation may be enqueued off an unverifiable claim"
        );
        let logs = db
            .list_filter_execution_logs(10)
            .await
            .expect("list execution logs");
        assert!(
            logs.is_empty(),
            "and nothing may be recorded as having fired"
        );
    }

    #[tokio::test]
    async fn an_account_less_pass_still_applies_server_confirmed_removals() {
        // A `PlacementKey` names its own account, so an explicitly-reported
        // removal needs no ambient account context. Gating the whole removals
        // block on `account_id.is_some()` would silently discard a VANISHED
        // report on a mock-driven `SyncAll`, leaving a ghost the server has
        // already told us is gone.
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let event_bus = EventBus::new();
        let filter_engine = empty_filter_engine();

        let staged = mock_placed("m-vanished", "inbox", "Deleted elsewhere");
        db.save_email_at(&staged.email, staged.source, &staged.placement)
            .await
            .expect("store the occupancy a prior pass left behind");

        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 0,
            unread_messages: 0,
        });
        mock.set_removals(vec![staged.placement.key()]);

        // `None` account: the pass has nowhere to key a checkpoint and cannot
        // run the `present` diff, but the removal is still actionable.
        let synced = fetch_and_persist(&db, &event_bus, &mock, &filter_engine, None, true)
            .await
            .expect("sync succeeds");
        assert_eq!(synced, 0, "nothing new arrived");

        let placements = db
            .placements_of("m-vanished")
            .await
            .expect("read the message's placements");
        assert!(
            placements.is_empty(),
            "the server-confirmed removal must be applied without an account id"
        );
    }
}
