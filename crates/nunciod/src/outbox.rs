//! Outbox remote-mutation executor.
//!
//! The filter engine enqueues a [`nuncio_filter::PendingRemoteMutation`] for
//! every genuinely-remote action a rule matches (`MOVE`/`COPY`/`FLAG`/
//! `UNFLAG`/`DELETE`/`FORWARD`/`CALL WEBHOOK`); `MARK READ`/`MARK UNREAD` are
//! applied to the local store inline and are never enqueued. This module
//! drains that outbox against the real world -- the resolved account's
//! [`MailBackend`] for the mailbox mutations, a [`MessageSender`] for
//! `FORWARD`, and the SSRF-hardened webhook dispatcher for `CALL WEBHOOK`.
//!
//! # The one inviolable rule
//!
//! A mutation is marked `completed` ONLY when the real remote operation
//! genuinely succeeded. A transient failure keeps the item `pending` (bumping
//! its retry count) until [`nuncio_filter::OutboxManager::MAX_RETRIES`] is
//! exceeded, at which point it flips to `failed`. A permanent failure (the
//! message no longer exists, a malformed payload, an unknown action) flips to
//! `failed` immediately -- there is nothing a retry could fix. Success is
//! never fabricated. A mutation whose execution exceeds
//! [`PER_ITEM_EXECUTION_TIMEOUT`] is dispositioned the same way as any other
//! transient failure -- it is RETRYABLE, never `completed`.
//!
//! # Testability
//!
//! [`execute_pending_mutations`] takes a `&dyn RemoteExecutionEnv`, so an
//! offline E2E injects mock backends/senders and a loopback-allowed webhook
//! dispatcher, proving the full sync -> filter -> outbox -> execute vertical
//! without any live network. Production wires [`ProductionExecutionEnv`],
//! which resolves real per-account engines from the store and the OS keyring.

use crate::lifecycle::ShutdownSignal;
use async_trait::async_trait;
use nuncio_core::AccountConfig;
use nuncio_filter::{
    MutationPayload, OutboxManager, PendingRemoteMutation, ValidationOptions, WebhookDispatcher,
    WebhookError,
};
use nuncio_mail::{
    MailBackend, MailError, MessageSender, OutboundMessage, RemoteMutationKind, RemoteMutationSpec,
    SmtpTransportEngine,
};
use nuncio_store::db::{DatabaseEngine, DatabaseError};
use nuncio_store::vault::{SecretManager, VaultError, WEBHOOK_SIGNING_KEY_ACCOUNT};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// Bound on how long a single outbox mutation may run before it is treated
/// as a retryable timeout. A healthy IMAP/JMAP/SMTP round trip or webhook
/// POST comfortably finishes well inside this window; a value this generous
/// still keeps one hung remote op from stalling every OTHER pending
/// mutation -- across every account -- behind it in the same drain pass, and
/// from blocking graceful shutdown for longer than the shutdown grace
/// period tolerates.
const PER_ITEM_EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Errors that can occur while resolving the resources needed to execute a
/// single outbox mutation. These are surfaced from a [`RemoteExecutionEnv`]
/// and treated as transient (retryable) by the executor unless it has already
/// determined the failure is permanent.
#[derive(Debug, Error)]
pub enum OutboxExecuteError {
    /// No persisted account configuration matches the resolved account id.
    #[error("account '{0}' is not configured")]
    AccountNotFound(String),
    /// Failed to read the account's credential from the secret vault.
    #[error("failed to read account credential from the secret vault: {0}")]
    Vault(#[from] VaultError),
    /// A mail backend / transport construction or operation error.
    #[error("mail error: {0}")]
    Mail(#[from] MailError),
    /// A database error occurred while resolving the account/message.
    #[error("database error: {0}")]
    Store(#[from] DatabaseError),
    /// The resolved account is not a mail account (e.g. a CalDAV account), so
    /// it has no mail backend to execute an outbound mutation against.
    #[error("account '{0}' is not a mail account")]
    NotAMailAccount(String),
}

/// Injectable seam for the resources a mutation needs: the per-account mail
/// backend and outbound sender, plus the webhook dispatcher. Production uses
/// [`ProductionExecutionEnv`]; tests inject mocks.
#[async_trait]
pub trait RemoteExecutionEnv: Send + Sync {
    /// Build the [`MailBackend`] for the account that owns the target message.
    async fn mail_backend(
        &self,
        account_id: &str,
    ) -> Result<Box<dyn MailBackend>, OutboxExecuteError>;

    /// Build the [`MessageSender`] used to deliver a `FORWARD`.
    async fn message_sender(
        &self,
        account_id: &str,
    ) -> Result<Box<dyn MessageSender>, OutboxExecuteError>;

    /// Dispatch a signed `CALL WEBHOOK` POST, returning the HTTP status code on
    /// delivery. The implementation owns the SSRF egress policy -- production
    /// keeps the secure default (private targets blocked); a test env may relax
    /// it to reach a loopback mock server.
    async fn dispatch_webhook(
        &self,
        url: &str,
        rule_id: &str,
        message_id: &str,
        subject: &str,
        sender: &str,
    ) -> Result<u16, WebhookError>;
}

/// The disposition of a single mutation attempt.
enum Disposition {
    /// The real operation genuinely succeeded; mark `completed`.
    Completed,
    /// A transient failure; keep retrying until `MAX_RETRIES` is exceeded.
    Retry(String),
    /// A permanent failure nothing could retry away; mark `failed` now.
    Permanent(String),
}

/// Tally of what a drain pass did, for logging and test assertions.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionSummary {
    /// Mutations marked `completed` on genuine success.
    pub completed: usize,
    /// Mutations left `pending` for a later retry.
    pub retried: usize,
    /// Mutations flipped to `failed` (permanent, or retries exhausted).
    pub failed: usize,
}

/// Drain up to `limit` pending mutations, executing each against `env` and
/// persisting the honest outcome. Never marks a mutation `completed` unless the
/// real operation succeeded. Each item's execution is bounded by
/// [`PER_ITEM_EXECUTION_TIMEOUT`] so one hung remote op cannot freeze delivery
/// for every other item behind it; `shutdown` is raced against that same
/// timeout so a hung item cannot delay graceful shutdown either -- on
/// shutdown the drain pass stops immediately, leaving the in-flight item (and
/// anything still queued behind it) untouched and `pending` for the next
/// start. Returns a tally of the pass.
pub async fn execute_pending_mutations(
    db: &DatabaseEngine,
    env: &dyn RemoteExecutionEnv,
    limit: usize,
    shutdown: &mut ShutdownSignal,
) -> ExecutionSummary {
    let pending = match db.list_pending_mutations(limit).await {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!("outbox: failed to list pending mutations: {e}");
            return ExecutionSummary::default();
        }
    };

    let mut summary = ExecutionSummary::default();
    for item in pending {
        // Log the executor's decision-level attempt before issuing the real
        // remote op: the mutation identity, its operation, and the surrogate
        // message id it targets (never the message body/subject/recipients).
        tracing::info!(
            mutation_id = %item.id,
            op = %item.mutation_type,
            message_id = %item.message_id,
            retry_count = item.retry_count,
            "outbox: attempting mutation"
        );
        let outcome = tokio::select! {
            biased;
            _ = shutdown.wait() => {
                tracing::info!(
                    mutation_id = %item.id,
                    "outbox: drain pass interrupted by shutdown signal; item left pending"
                );
                break;
            }
            result = tokio::time::timeout(PER_ITEM_EXECUTION_TIMEOUT, execute_one(db, env, &item)) => result,
        };
        let disposition = match outcome {
            Ok(disposition) => disposition,
            Err(_elapsed) => Disposition::Retry(format!(
                "mutation execution exceeded the {PER_ITEM_EXECUTION_TIMEOUT:?} timeout"
            )),
        };
        let next_retry = item.retry_count + 1;
        let (status, retry, tally): (&str, i32, fn(&mut ExecutionSummary)) = match disposition {
            Disposition::Completed => {
                tracing::info!(
                    mutation_id = %item.id,
                    op = %item.mutation_type,
                    "outbox: mutation completed"
                );
                ("completed", item.retry_count, |s| s.completed += 1)
            }
            Disposition::Permanent(reason) => {
                tracing::warn!(
                    mutation_id = %item.id,
                    "outbox: permanently failing mutation: {reason}"
                );
                ("failed", next_retry, |s| s.failed += 1)
            }
            Disposition::Retry(reason) if next_retry > OutboxManager::MAX_RETRIES => {
                tracing::warn!(
                    mutation_id = %item.id,
                    "outbox: mutation failed after exhausting retries: {reason}"
                );
                ("failed", next_retry, |s| s.failed += 1)
            }
            Disposition::Retry(reason) => {
                tracing::debug!(
                    mutation_id = %item.id,
                    "outbox: mutation will be retried: {reason}"
                );
                ("pending", next_retry, |s| s.retried += 1)
            }
        };

        if let Err(e) = db.update_mutation_status(&item.id, status, retry).await {
            // A status-write failure must not crash the worker; the item stays
            // in its prior state and is reconsidered on the next poll.
            tracing::warn!(mutation_id = %item.id, "outbox: failed to persist status: {e}");
        } else {
            tally(&mut summary);
        }
    }
    summary
}

/// Execute a single mutation and report its honest disposition. Performs no
/// status writes itself -- the caller persists the outcome.
async fn execute_one(
    db: &DatabaseEngine,
    env: &dyn RemoteExecutionEnv,
    mutation: &PendingRemoteMutation,
) -> Disposition {
    // Resolve the message to recover its account and source folder. A message
    // that no longer exists can never be acted on -- fail permanently.
    let email = match db.get_message(&mutation.message_id).await {
        Ok(email) => email,
        Err(e) if e.is_not_found() => {
            return Disposition::Permanent(format!(
                "message '{}' no longer exists in the store",
                mutation.message_id
            ));
        }
        Err(e) => {
            return Disposition::Retry(format!("failed to load message: {e}"));
        }
    };

    let payload = match serde_json::from_str::<MutationPayload>(&mutation.payload) {
        Ok(payload) => payload,
        Err(e) => {
            return Disposition::Permanent(format!("malformed mutation payload: {e}"));
        }
    };

    match mutation.mutation_type.as_str() {
        "MOVE" | "COPY" | "FLAG" | "UNFLAG" | "DELETE" => {
            dispatch_mailbox_mutation(env, mutation, &email, &payload).await
        }
        "FORWARD" => dispatch_forward(db, env, &email, &payload).await,
        "WEBHOOK" => dispatch_webhook(env, mutation, &email, &payload).await,
        other => Disposition::Permanent(format!("unknown mutation type '{other}'")),
    }
}

/// Execute a mailbox-affecting mutation (move/copy/flag/unflag/delete) against
/// the account's mail backend, recovering the remote addressing (protocol id +
/// UIDVALIDITY scope) from the stored message row so the backend can enforce
/// its UIDVALIDITY guard.
async fn dispatch_mailbox_mutation(
    env: &dyn RemoteExecutionEnv,
    mutation: &PendingRemoteMutation,
    email: &nuncio_core::model::Email,
    payload: &MutationPayload,
) -> Disposition {
    let kind = match mutation.mutation_type.as_str() {
        "FLAG" => RemoteMutationKind::SetFlagged { value: true },
        "UNFLAG" => RemoteMutationKind::SetFlagged { value: false },
        "DELETE" => RemoteMutationKind::Delete,
        "MOVE" => match &payload.target {
            Some(folder) => RemoteMutationKind::Move {
                to_folder: folder.clone(),
            },
            None => {
                return Disposition::Permanent("MOVE mutation has no target folder".to_string())
            }
        },
        "COPY" => match &payload.target {
            Some(folder) => RemoteMutationKind::Copy {
                to_folder: folder.clone(),
            },
            None => {
                return Disposition::Permanent("COPY mutation has no target folder".to_string())
            }
        },
        other => return Disposition::Permanent(format!("unexpected mailbox mutation '{other}'")),
    };

    let backend = match env.mail_backend(&email.account_id).await {
        Ok(backend) => backend,
        Err(e) => return Disposition::Retry(format!("failed to build mail backend: {e}")),
    };

    // Recover the remote addressing from the stored message row -- the
    // protocol-native id and the UIDVALIDITY scope it was captured under --
    // rather than parsing it out of the opaque surrogate id. The backend still
    // SELECT-verifies the UIDVALIDITY before mutating.
    let spec = RemoteMutationSpec {
        message_id: email.id.clone(),
        remote_id: email.remote_id.clone(),
        folder_id: email.folder_id.clone(),
        uid_validity: email.uid_validity.clone(),
        kind,
    };

    tracing::debug!(
        op = %mutation.mutation_type,
        remote_id = %spec.remote_id,
        folder_id = %spec.folder_id,
        uid_validity = %spec.uid_validity,
        "outbox: issuing mailbox mutation against remote backend"
    );

    match backend.apply_mutation(&spec).await {
        Ok(()) => Disposition::Completed,
        Err(e) => Disposition::Retry(format!("remote mutation failed: {e}")),
    }
}

/// Compose and send a forward of `email` to the payload's target address via
/// the account's outbound transport.
async fn dispatch_forward(
    db: &DatabaseEngine,
    env: &dyn RemoteExecutionEnv,
    email: &nuncio_core::model::Email,
    payload: &MutationPayload,
) -> Disposition {
    let Some(to) = payload.target.clone() else {
        return Disposition::Permanent("FORWARD mutation has no target address".to_string());
    };

    // The From: is the forwarding account's own address, resolved from its
    // persisted configuration rather than trusting the stored envelope.
    let from = match find_account_config(db, &email.account_id).await {
        Ok(config) => config.email_address,
        Err(OutboxExecuteError::AccountNotFound(id)) => {
            return Disposition::Permanent(format!("forwarding account '{id}' is not configured"));
        }
        Err(e) => return Disposition::Retry(format!("failed to resolve forwarding account: {e}")),
    };

    let sender = match env.message_sender(&email.account_id).await {
        Ok(sender) => sender,
        Err(e) => return Disposition::Retry(format!("failed to build message sender: {e}")),
    };

    let outbound = OutboundMessage {
        from,
        to,
        cc: None,
        subject: format!("Fwd: {}", email.subject),
        body_plain: email.body_plain.clone(),
        body_html: email.body_html.clone(),
        attachments: email.attachments.clone(),
    };

    match sender.send(&outbound).await {
        Ok(()) => Disposition::Completed,
        Err(e) => Disposition::Retry(format!("forward send failed: {e}")),
    }
}

/// Deliver a signed webhook for the matched message. A 2xx/3xx response is a
/// genuine delivery; a 4xx/5xx is treated as a retryable failure so a transient
/// receiver outage does not silently drop the notification.
async fn dispatch_webhook(
    env: &dyn RemoteExecutionEnv,
    mutation: &PendingRemoteMutation,
    email: &nuncio_core::model::Email,
    payload: &MutationPayload,
) -> Disposition {
    let Some(url) = payload.target.as_deref() else {
        return Disposition::Permanent("WEBHOOK mutation has no target URL".to_string());
    };

    match env
        .dispatch_webhook(
            url,
            &mutation.rule_id,
            &mutation.message_id,
            &email.subject,
            &email.sender,
        )
        .await
    {
        Ok(status) if (200..400).contains(&status) => Disposition::Completed,
        Ok(status) => Disposition::Retry(format!("webhook returned HTTP {status}")),
        // A security-policy violation (blocked SSRF target) is permanent: the
        // URL will never become allowable on retry.
        Err(WebhookError::SecurityViolation(reason)) => {
            Disposition::Permanent(format!("webhook blocked by security policy: {reason}"))
        }
        Err(e) => Disposition::Retry(format!("webhook dispatch failed: {e}")),
    }
}

/// Look up an account's persisted configuration by id.
async fn find_account_config(
    db: &DatabaseEngine,
    account_id: &str,
) -> Result<AccountConfig, OutboxExecuteError> {
    db.list_accounts()
        .await?
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| OutboxExecuteError::AccountNotFound(account_id.to_string()))
}

/// Production [`RemoteExecutionEnv`]: resolves real per-account IMAP/JMAP mail
/// backends and SMTP senders from the store plus the OS keyring, and lazily
/// builds a single SSRF-hardened [`WebhookDispatcher`] keyed by a daemon-wide
/// signing secret provisioned from the vault.
///
/// The webhook signing key is provisioned lazily -- only the first time a
/// `CALL WEBHOOK` mutation is actually dispatched -- so a vault issue with that
/// one key can never disable the mailbox-mutation and forward paths, which do
/// not need it.
pub struct ProductionExecutionEnv {
    db: Arc<DatabaseEngine>,
    secrets: Arc<SecretManager>,
    webhook: std::sync::OnceLock<WebhookDispatcher>,
}

impl ProductionExecutionEnv {
    /// Build the production environment. Infallible: no vault access happens
    /// here, so the outbox worker can always start; the webhook signing key is
    /// provisioned on first webhook dispatch instead (see the struct doc).
    pub fn new(db: Arc<DatabaseEngine>, secrets: Arc<SecretManager>) -> Self {
        Self {
            db,
            secrets,
            webhook: std::sync::OnceLock::new(),
        }
    }

    async fn account(&self, account_id: &str) -> Result<AccountConfig, OutboxExecuteError> {
        find_account_config(&self.db, account_id).await
    }

    /// Return the lazily-provisioned webhook dispatcher, minting (or loading)
    /// the daemon-wide signing key from the vault on first use. Fails closed if
    /// the vault is unavailable, so a `CALL WEBHOOK` is never dispatched
    /// unsigned -- and, being lazy, that failure is isolated to webhook
    /// dispatch and never disables the other mutation paths.
    fn webhook_dispatcher(&self) -> Result<&WebhookDispatcher, WebhookError> {
        if let Some(dispatcher) = self.webhook.get() {
            return Ok(dispatcher);
        }
        let key_bytes = self
            .secrets
            .get_or_create_key_bytes(WEBHOOK_SIGNING_KEY_ACCOUNT, 32)
            .map_err(|e| {
                WebhookError::SigningError(format!(
                    "failed to provision webhook signing key from the vault: {e}"
                ))
            })?;
        // A concurrent caller may win the race to set it; either way the stored
        // dispatcher is authoritative.
        let _ = self
            .webhook
            .set(WebhookDispatcher::new(hex::encode(key_bytes)));
        self.webhook.get().ok_or_else(|| {
            WebhookError::SigningError("webhook dispatcher unavailable after provisioning".into())
        })
    }
}

#[async_trait]
impl RemoteExecutionEnv for ProductionExecutionEnv {
    async fn mail_backend(
        &self,
        account_id: &str,
    ) -> Result<Box<dyn MailBackend>, OutboxExecuteError> {
        let config = self.account(account_id).await?;
        let password = self.secrets.get_secret(&config.keyring_secret_key)?;
        crate::sync::build_mail_backend(&config, &password)
            .map_err(|_| OutboxExecuteError::NotAMailAccount(account_id.to_string()))
    }

    async fn message_sender(
        &self,
        account_id: &str,
    ) -> Result<Box<dyn MessageSender>, OutboxExecuteError> {
        let config = self.account(account_id).await?;
        let password = self.secrets.get_secret(&config.keyring_secret_key)?;
        let transport = config
            .imap_smtp()
            .ok_or_else(|| OutboxExecuteError::NotAMailAccount(account_id.to_string()))?;
        let engine = SmtpTransportEngine::new(
            &transport.smtp_host,
            transport.smtp_port,
            transport.smtp_tls_mode,
            &config.email_address,
            &password,
        )?;
        Ok(Box::new(engine))
    }

    async fn dispatch_webhook(
        &self,
        url: &str,
        rule_id: &str,
        message_id: &str,
        subject: &str,
        sender: &str,
    ) -> Result<u16, WebhookError> {
        // Production keeps the secure default egress policy (private/loopback
        // targets are rejected).
        self.webhook_dispatcher()?
            .dispatch_with_options(
                url,
                rule_id,
                message_id,
                subject,
                sender,
                &ValidationOptions::default(),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{ShutdownController, ShutdownSignal};
    use crate::test_tracing::with_recorder;
    use nuncio_core::model::Email;
    use nuncio_core::EventBus;
    use nuncio_filter::MutationPayload;
    use nuncio_mail::MockMailBackend;
    use tracing::Level;

    /// Minimal test [`RemoteExecutionEnv`] whose mailbox mutations always
    /// genuinely succeed against a recording [`MockMailBackend`]; the forward
    /// and webhook paths are unused by these tests.
    struct SucceedingEnv {
        backend: MockMailBackend,
    }

    #[async_trait]
    impl RemoteExecutionEnv for SucceedingEnv {
        async fn mail_backend(
            &self,
            _account_id: &str,
        ) -> Result<Box<dyn MailBackend>, OutboxExecuteError> {
            Ok(Box::new(self.backend.clone()))
        }

        async fn message_sender(
            &self,
            account_id: &str,
        ) -> Result<Box<dyn MessageSender>, OutboxExecuteError> {
            // The FLAG-only tests never take the forward path; surface an error
            // rather than a panic if that assumption ever changes.
            Err(OutboxExecuteError::NotAMailAccount(account_id.to_string()))
        }

        async fn dispatch_webhook(
            &self,
            _url: &str,
            _rule_id: &str,
            _message_id: &str,
            _subject: &str,
            _sender: &str,
        ) -> Result<u16, WebhookError> {
            // The FLAG-only tests never take the webhook path.
            Err(WebhookError::SigningError(
                "webhook path unused in test".into(),
            ))
        }
    }

    fn no_shutdown() -> (ShutdownController, ShutdownSignal) {
        ShutdownController::new(Arc::new(EventBus::new()))
    }

    fn sample_email() -> Email {
        Email {
            id: "msg-outbox-log".to_string(),
            account_id: "acct-outbox-log".to_string(),
            folder_id: "INBOX".to_string(),
            remote_id: "77".to_string(),
            uid_validity: "9".to_string(),
            subject: "Confidential outbox subject".to_string(),
            sender: "alice@example.com".to_string(),
            recipient: "owner@nuncio.mx".to_string(),
            received_at: 1_700_000_000,
            read: false,
            body_plain: Some("secret outbox body".to_string()),
            body_html: None,
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
        }
    }

    fn flag_mutation() -> PendingRemoteMutation {
        PendingRemoteMutation {
            id: "mut-1".to_string(),
            rule_id: "rule-1".to_string(),
            message_id: "msg-outbox-log".to_string(),
            mutation_type: "FLAG".to_string(),
            payload: serde_json::to_string(&MutationPayload {
                action_type: "FLAG".to_string(),
                target: None,
            })
            .expect("serialize payload"),
            status: "pending".to_string(),
            retry_count: 0,
            created_at: 1_700_000_000,
        }
    }

    /// An outbox drain must log the executor's decision-level attempt BEFORE
    /// issuing the remote op and the Completed result AFTER, carrying the
    /// mutation id and operation -- and never the message body or subject.
    #[test]
    fn drain_logs_attempt_and_completion_without_leaking_message_content() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let (recorder, ()) = with_recorder(|| {
            runtime.block_on(async {
                let (db, _dir) = DatabaseEngine::connect_ephemeral()
                    .await
                    .expect("ephemeral db");
                db.save_folder_sync_state("acct-outbox-log", "INBOX", "9:100")
                    .await
                    .expect("save folder checkpoint");
                db.save_email(&sample_email()).await.expect("save email");
                db.save_pending_mutation(&flag_mutation())
                    .await
                    .expect("save pending mutation");

                let env = SucceedingEnv {
                    backend: MockMailBackend::new(),
                };
                let (_ctl, mut shutdown) = no_shutdown();
                let summary = execute_pending_mutations(&db, &env, 10, &mut shutdown).await;
                assert_eq!(summary.completed, 1, "the mutation must complete");
            });
        });

        let events = recorder.events();
        let attempt = events
            .iter()
            .find(|e| e.message() == "outbox: attempting mutation")
            .expect("an attempt event must be logged before issuing the op");
        assert_eq!(attempt.level, Level::INFO);
        assert_eq!(attempt.fields.get("op").map(String::as_str), Some("FLAG"));
        assert_eq!(
            attempt.fields.get("mutation_id").map(String::as_str),
            Some("mut-1")
        );

        let completed = events
            .iter()
            .find(|e| e.message() == "outbox: mutation completed")
            .expect("a completion event must be logged after a genuine success");
        assert_eq!(completed.level, Level::INFO);
        assert_eq!(completed.fields.get("op").map(String::as_str), Some("FLAG"));

        for value in recorder.all_field_values() {
            assert!(
                !value.contains("Confidential outbox subject"),
                "subject leaked into telemetry: {value}"
            );
            assert!(
                !value.contains("secret outbox body"),
                "body leaked into telemetry: {value}"
            );
        }
    }
}
