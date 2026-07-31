//! Real outbound mail send routine, including per-account SMTP endpoint
//! resolution.
//!
//! Prior to this module, `nuncio-cli`'s `mail send` fabricated success --
//! it never dialed an SMTP server at all (see `HeadlessRunner::handle_send_email`
//! previously). This module closes that gap: the daemon resolves the
//! account to send from, reads its keyring-stored password, builds a real
//! [`SmtpTransportEngine`] from the account's SMTP endpoint
//! (`AccountConfig::smtp_host`/`smtp_port`), and hands the composed message
//! to it. A genuine SMTP transport failure surfaces as [`SendError`]; success
//! is only ever reported when the transport actually accepted the message.
//!
//! # Account selection
//!
//! `SendMessage` accepts an optional `account_id`. When the caller supplies
//! one, that specific account is resolved and used as the sender -- a
//! request naming an account that does not exist fails honestly rather than
//! silently falling back to a different identity. When `account_id` is
//! omitted, the account resolves to the first one returned by
//! [`DatabaseEngine::list_accounts`], preserving behavior for callers that
//! predate explicit selection.
//!
//! # Testability
//!
//! [`send_with_sender`] takes `&dyn MessageSender` rather than a concrete
//! transport, so it is exercised directly in tests with [`MockMessageSender`]
//! -- no real network or OS keyring access ever happens inside it, and tests
//! can assert on the EXACT [`OutboundMessage`] the transport received.
//! Production entry points ([`send_message_for_account`]) are the only place
//! that resolves a real account's credentials and constructs a real
//! [`SmtpTransportEngine`]; they then delegate the actual send to
//! [`send_with_sender`]. This mirrors `nunciod::sync`'s `&dyn MailBackend`
//! seam for inbound sync.
//!
//! # Outbox (reliability) design decision
//!
//! The existing transactional outbox (`nuncio_filter::OutboxManager` /
//! `pending_remote_mutations`) is purpose-built for retrying remote
//! mutations against an EXISTING synced message (`rule_id` + `message_id` +
//! an action type like `MOVE`/`FLAG`/`DELETE`) -- it has no notion of a
//! freshly-composed outbound message (recipients, subject, body,
//! attachments) that was never itself a persisted/synced message. Reshaping
//! that schema to carry a full compose payload would be a much larger
//! change than the scope of this send path. As a deliberate escape hatch,
//! this send is therefore a DIRECT send inside the `SendMessage` RPC handler:
//! synchronous, and honest about success/failure -- never fabricated --
//! but a transient SMTP failure is NOT automatically retried in the
//! background the way a filter mutation is. Outbound sends could route
//! through a dedicated outbox table in the future if automatic retry
//! becomes a requirement.
use nuncio_core::model::Attachment;
use nuncio_core::AccountConfig;
use nuncio_mail::{MailError, MessageSender, OutboundMessage, SmtpTransportEngine};
use nuncio_store::db::{DatabaseEngine, DatabaseError};
use nuncio_store::vault::{SecretManager, VaultError};
use thiserror::Error;

/// Errors that can occur while sending an outbound message.
#[derive(Debug, Error)]
pub enum SendError {
    /// No account is configured to send mail from.
    #[error("no account is configured to send mail from")]
    NoAccountConfigured,
    /// The caller specified an `account_id` that does not match any
    /// configured account. Distinct from [`SendError::NoAccountConfigured`]
    /// (which means zero accounts exist at all): this means at least one
    /// account exists, just not the one requested, so the request must fail
    /// rather than silently sending from a different account.
    #[error("no account configured with id {0:?}")]
    AccountNotFound(String),
    /// The resolved account does not have an SMTP outbound endpoint (only an
    /// IMAP/SMTP account can send outbound mail over SMTP; a JMAP or DAV
    /// account cannot).
    #[error("account {0:?} has no SMTP transport and cannot send outbound mail")]
    NotAnSmtpAccount(String),
    /// Failed to read the account's credential from the secret vault.
    #[error("failed to read account credential from the secret vault: {0}")]
    Vault(#[from] VaultError),
    /// The SMTP transport reported an error. Never fabricated -- this is
    /// always a genuine failure surfaced from the transport.
    #[error("mail transport error: {0}")]
    Mail(#[from] MailError),
    /// A database error occurred while resolving the sending account.
    #[error("database error: {0}")]
    Store(#[from] DatabaseError),
}

/// A caller-supplied compose request: everything the `SendMessage` RPC
/// receives from the client, MINUS the `From:` address (always resolved
/// server-side from the configured account, never supplied by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeRequest {
    /// Recipient email address.
    pub to: String,
    /// Optional CC recipient email address.
    pub cc: Option<String>,
    /// Message subject line.
    pub subject: String,
    /// Plaintext message body.
    pub body_text: Option<String>,
    /// Optional HTML message body.
    pub body_html: Option<String>,
    /// File attachments.
    pub attachments: Vec<Attachment>,
    /// Identifier of the account to send from. `None` preserves the prior
    /// "first configured account" behavior.
    pub account_id: Option<String>,
}

/// Send `message` through `sender`. Pure seam shared by production and
/// tests -- see the module-level "Testability" doc above. Never fabricates
/// success: `sender.send` reporting an error propagates as [`SendError::Mail`].
pub async fn send_with_sender(
    sender: &dyn MessageSender,
    message: &OutboundMessage,
) -> Result<(), SendError> {
    sender.send(message).await.map_err(SendError::from)
}

/// Construct the real [`SmtpTransportEngine`] for `config`, authenticated
/// with `password` (read from the OS keyring by the caller). This is the
/// ONLY place a concrete transport is chosen in production; every other
/// function in this module only ever sees `&dyn MessageSender`.
fn build_smtp_sender(
    config: &AccountConfig,
    password: &str,
) -> Result<SmtpTransportEngine, SendError> {
    let transport = config
        .imap_smtp()
        .ok_or_else(|| SendError::NotAnSmtpAccount(config.id.clone()))?;
    let engine = SmtpTransportEngine::new(
        &transport.smtp_host,
        transport.smtp_port,
        transport.smtp_tls_mode,
        &config.email_address,
        password,
    )?;
    Ok(engine)
}

/// Resolves the account to send mail from. See the module-level "Account
/// selection" doc above: an explicit `account_id` must match a configured
/// account or the request fails; `None` preserves the "first configured
/// account" behavior.
async fn resolve_sending_account(
    db: &DatabaseEngine,
    account_id: Option<&str>,
) -> Result<AccountConfig, SendError> {
    let accounts = db.list_accounts().await?;
    match account_id {
        Some(id) => accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| SendError::AccountNotFound(id.to_string())),
        None => accounts
            .into_iter()
            .next()
            .ok_or(SendError::NoAccountConfigured),
    }
}

/// Generates a daemon-local identifier for a sent message, for reference in
/// logs/output only -- NOT a persisted message id (outbound sends are not
/// written back into the synced message store by this RPC).
fn generate_message_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("sent-{nanos:x}")
}

/// Production entry point for the `SendMessage` RPC: resolves the sending
/// account and its keyring password, builds a real [`SmtpTransportEngine`]
/// from the account's SMTP endpoint, and sends `request` through it.
/// Returns the generated
/// message id ONLY on genuine transport acceptance -- a resolution failure
/// (no account, missing credential) or transport failure both surface as
/// `Err`, never a fabricated success.
pub async fn send_message_for_account(
    db: &DatabaseEngine,
    secrets: &SecretManager,
    request: ComposeRequest,
) -> Result<String, SendError> {
    let config = resolve_sending_account(db, request.account_id.as_deref()).await?;
    let password = secrets.get_secret(&config.keyring_secret_key)?;
    let sender = build_smtp_sender(&config, &password)?;

    let outbound = OutboundMessage {
        from: config.email_address,
        to: request.to,
        cc: request.cc,
        subject: request.subject,
        body_plain: request.body_text,
        body_html: request.body_html,
        attachments: request.attachments,
    };

    send_with_sender(&sender, &outbound).await?;
    Ok(generate_message_id())
}

/// Test/E2E entry point for the `SendMessage` RPC when a
/// [`nunciod::grpc::MailEngineOverrides::message_sender`] is injected:
/// resolves ONLY the sending account's `From:` address from the store --
/// no keyring lookup, no real SMTP
/// transport construction -- and sends `request` through the given
/// `sender` (e.g. a [`nuncio_mail::MockMessageSender`] in a full-daemon
/// offline E2E test). Mirrors [`send_message_for_account`] in every other
/// respect: returns the generated message id ONLY on genuine acceptance by
/// `sender`, never a fabricated success.
pub async fn send_message_with_injected_sender(
    db: &DatabaseEngine,
    sender: &dyn MessageSender,
    request: ComposeRequest,
) -> Result<String, SendError> {
    let config = resolve_sending_account(db, request.account_id.as_deref()).await?;

    let outbound = OutboundMessage {
        from: config.email_address,
        to: request.to,
        cc: request.cc,
        subject: request.subject,
        body_plain: request.body_text,
        body_html: request.body_html,
        attachments: request.attachments,
    };

    send_with_sender(sender, &outbound).await?;
    Ok(generate_message_id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::TlsMode;
    use nuncio_mail::MockMessageSender;

    fn sample_account(id: &str) -> AccountConfig {
        AccountConfig {
            id: id.to_string(),
            name: "Send Test Account".to_string(),
            email_address: format!("{id}@nuncio.mx"),
            keyring_secret_key: format!("nuncio/{id}"),
            sync_interval_secs: 60,
            transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
                imap_host: "imap.nuncio.mx".to_string(),
                imap_port: 993,
                imap_tls_mode: TlsMode::ImplicitTls,
                smtp_host: "127.0.0.1".to_string(),
                smtp_port: 1,
                smtp_tls_mode: TlsMode::ImplicitTls,
            }),
        }
    }

    fn sample_outbound_message() -> OutboundMessage {
        OutboundMessage {
            from: "alice@nuncio.mx".to_string(),
            to: "bob@nuncio.mx".to_string(),
            cc: Some("carol@nuncio.mx".to_string()),
            subject: "Quarterly Roadmap".to_string(),
            body_plain: Some("Let's discuss the roadmap.".to_string()),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn send_with_sender_records_the_exact_message_and_reports_genuine_success() {
        let mock = MockMessageSender::new();
        let message = sample_outbound_message();

        send_with_sender(&mock, &message)
            .await
            .expect("send succeeds");

        let sent = mock.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], message);
        assert_eq!(sent[0].to, "bob@nuncio.mx");
        assert_eq!(sent[0].cc.as_deref(), Some("carol@nuncio.mx"));
        assert_eq!(sent[0].subject, "Quarterly Roadmap");
    }

    #[tokio::test]
    async fn send_with_sender_surfaces_transport_failure_as_error_never_fabricating_success() {
        let mock = MockMessageSender::new();
        mock.set_should_fail(true);
        let message = sample_outbound_message();

        let err = send_with_sender(&mock, &message)
            .await
            .expect_err("simulated transport failure must surface as an error");
        assert!(matches!(err, SendError::Mail(_)));
        // The failed attempt must never be recorded as "sent".
        assert!(mock.sent_messages().is_empty());
    }

    #[tokio::test]
    async fn send_message_for_account_reports_honest_error_when_no_account_configured() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let secrets = SecretManager::mock();

        let err = send_message_for_account(
            &db,
            &secrets,
            ComposeRequest {
                to: "bob@nuncio.mx".to_string(),
                cc: None,
                subject: "Hi".to_string(),
                body_text: Some("Body".to_string()),
                body_html: None,
                attachments: Vec::new(),
                account_id: None,
            },
        )
        .await
        .expect_err("no configured account must fail");
        assert!(matches!(err, SendError::NoAccountConfigured));
    }

    #[tokio::test]
    async fn send_message_for_account_reports_honest_error_when_credential_missing() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let secrets = SecretManager::mock();

        let config = sample_account("acct-no-cred");
        db.save_account(&config).await.expect("save account");
        // Deliberately never calls `secrets.set_secret(...)`.

        let err = send_message_for_account(
            &db,
            &secrets,
            ComposeRequest {
                to: "bob@nuncio.mx".to_string(),
                cc: None,
                subject: "Hi".to_string(),
                body_text: Some("Body".to_string()),
                body_html: None,
                attachments: Vec::new(),
                account_id: None,
            },
        )
        .await
        .expect_err("missing credential must fail");
        assert!(matches!(err, SendError::Vault(_)));
    }

    /// Proves the full production wiring (account resolution -> keyring
    /// password -> real `SmtpTransportEngine` construction -> send) reports
    /// a genuine transport error rather than fabricating success, WITHOUT
    /// any live network dependency: `smtp_host`/`smtp_port` point at a
    /// reserved loopback port nothing is listening on (mirroring
    /// `nuncio_mail::smtp`'s own `send_email_to_unreachable_server_fails_with_transport_error`
    /// test), so the connection attempt fails fast and deterministically.
    #[tokio::test]
    async fn send_message_for_account_builds_real_smtp_transport_and_fails_honestly_when_unreachable(
    ) {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let secrets = SecretManager::mock();

        let config = sample_account("acct-unreachable");
        db.save_account(&config).await.expect("save account");
        secrets
            .set_secret(&config.keyring_secret_key, "irrelevant-password")
            .expect("store credential in mock vault");

        let err = send_message_for_account(
            &db,
            &secrets,
            ComposeRequest {
                to: "bob@nuncio.mx".to_string(),
                cc: None,
                subject: "Hi".to_string(),
                body_text: Some("Body".to_string()),
                body_html: None,
                attachments: Vec::new(),
                account_id: None,
            },
        )
        .await
        .expect_err("delivery to an unreachable SMTP host must fail");
        assert!(matches!(err, SendError::Mail(_)));
    }

    #[test]
    fn generate_message_id_produces_a_non_empty_prefixed_identifier() {
        let id = generate_message_id();
        assert!(id.starts_with("sent-"));
        assert!(id.len() > "sent-".len());
    }

    /// Proves `send_message_with_injected_sender` resolves the `From:`
    /// address from the store and records the exact
    /// outbound message on the injected sender, WITHOUT ever touching the
    /// keyring (no credential is even stored for this account).
    #[tokio::test]
    async fn send_message_with_injected_sender_records_exact_message_without_touching_vault() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");

        let config = sample_account("acct-injected-sender");
        db.save_account(&config).await.expect("save account");
        // Deliberately never stores a keyring credential -- this path must
        // never need one.

        let mock = MockMessageSender::new();
        let message_id = send_message_with_injected_sender(
            &db,
            &mock,
            ComposeRequest {
                to: "bob@nuncio.mx".to_string(),
                cc: Some("carol@nuncio.mx".to_string()),
                subject: "Injected Sender Test".to_string(),
                body_text: Some("Body via injected sender.".to_string()),
                body_html: None,
                attachments: Vec::new(),
                account_id: None,
            },
        )
        .await
        .expect("send succeeds via injected sender");
        assert!(message_id.starts_with("sent-"));

        let sent = mock.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].from, config.email_address);
        assert_eq!(sent[0].to, "bob@nuncio.mx");
        assert_eq!(sent[0].cc.as_deref(), Some("carol@nuncio.mx"));
        assert_eq!(sent[0].subject, "Injected Sender Test");
    }

    #[tokio::test]
    async fn send_message_with_injected_sender_reports_honest_error_when_no_account_configured() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        let mock = MockMessageSender::new();

        let err = send_message_with_injected_sender(
            &db,
            &mock,
            ComposeRequest {
                to: "bob@nuncio.mx".to_string(),
                cc: None,
                subject: "Hi".to_string(),
                body_text: Some("Body".to_string()),
                body_html: None,
                attachments: Vec::new(),
                account_id: None,
            },
        )
        .await
        .expect_err("no configured account must fail");
        assert!(matches!(err, SendError::NoAccountConfigured));
        assert!(mock.sent_messages().is_empty());
    }

    /// Proves that with multiple configured accounts, an explicit
    /// `account_id` selects that SPECIFIC account as the sender -- not
    /// whichever account happens to be first -- by asserting the captured
    /// outbound message's `From:` matches the requested account.
    #[tokio::test]
    async fn send_message_with_injected_sender_uses_the_explicitly_selected_account() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");

        let first = sample_account("acct-first");
        let second = sample_account("acct-second");
        db.save_account(&first).await.expect("save first account");
        db.save_account(&second).await.expect("save second account");

        let mock = MockMessageSender::new();
        send_message_with_injected_sender(
            &db,
            &mock,
            ComposeRequest {
                to: "bob@nuncio.mx".to_string(),
                cc: None,
                subject: "Hi".to_string(),
                body_text: Some("Body".to_string()),
                body_html: None,
                attachments: Vec::new(),
                account_id: Some(second.id.clone()),
            },
        )
        .await
        .expect("send succeeds via explicitly selected account");

        let sent = mock.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].from, second.email_address);
        assert_ne!(sent[0].from, first.email_address);
    }

    /// An `account_id` naming an account that does not exist must fail with
    /// the specific [`SendError::AccountNotFound`] variant -- never a silent
    /// fallback to the first configured account, and never confusable with
    /// [`SendError::NoAccountConfigured`] (which means zero accounts exist).
    #[tokio::test]
    async fn send_message_with_injected_sender_rejects_unknown_account_id() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");

        let config = sample_account("acct-only");
        db.save_account(&config).await.expect("save account");

        let mock = MockMessageSender::new();
        let err = send_message_with_injected_sender(
            &db,
            &mock,
            ComposeRequest {
                to: "bob@nuncio.mx".to_string(),
                cc: None,
                subject: "Hi".to_string(),
                body_text: Some("Body".to_string()),
                body_html: None,
                attachments: Vec::new(),
                account_id: Some("acct-does-not-exist".to_string()),
            },
        )
        .await
        .expect_err("unknown account id must fail");
        assert!(matches!(err, SendError::AccountNotFound(id) if id == "acct-does-not-exist"));
        assert!(mock.sent_messages().is_empty());
    }
}
