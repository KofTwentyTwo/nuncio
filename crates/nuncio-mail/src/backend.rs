//! Protocol-agnostic async mail backend trait definitions.

use async_trait::async_trait;
use nuncio_core::model::{Attachment, Email, Folder};

use crate::parser::MailError;

/// Protocol-agnostic mail backend engine trait implemented by JMAP and IMAP engines.
#[async_trait]
pub trait MailBackend: Send + Sync {
    /// Synchronize and list available mailbox folders.
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError>;

    /// Synchronize message envelopes for a specific folder since a checkpoint state.
    /// Returns the updated email list and the new server state checkpoint.
    async fn sync_messages(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<(Vec<Email>, String), MailError>;

    /// Send an email message over the configured transport.
    async fn send_email(&self, email: &Email) -> Result<(), MailError>;

    /// Validate that this backend's configured account settings genuinely
    /// work end to end: real connect + auth against every transport the
    /// backend uses, with no sync/send side effects. Implementations MUST
    /// return the real underlying error on failure -- never a fabricated
    /// success -- and MUST return an honest "not supported" error rather
    /// than a fabricated success for a protocol whose transport isn't
    /// genuinely implemented yet.
    async fn test_connection(&self) -> Result<(), MailError>;
}

/// A composed outbound email message ready to send over SMTP. Deliberately
/// independent of the persisted [`Email`] model: an outbound compose has no
/// `id`/`folder_id`/
/// `received_at`/`read` -- it is never itself a synced inbox message, only
/// something being handed to a transport for delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMessage {
    /// Sender address (the configured account's own email address).
    pub from: String,
    /// Recipient email address.
    pub to: String,
    /// Optional CC recipient email address.
    pub cc: Option<String>,
    /// Message subject line.
    pub subject: String,
    /// Plaintext message body.
    pub body_plain: Option<String>,
    /// HTML message body.
    pub body_html: Option<String>,
    /// File attachments.
    pub attachments: Vec<Attachment>,
}

/// Narrow, send-only transport seam used by the outbound send RPC, distinct
/// from the broader [`MailBackend`] trait: an SMTP-only transport cannot
/// meaningfully implement
/// `sync_folders`/`sync_messages`, so forcing it to implement all of
/// `MailBackend` would mean fabricating those methods. Production code
/// (`nunciod`) builds a real [`crate::smtp::SmtpTransportEngine`] from the
/// account's SMTP endpoint + keyring password; tests inject a mock that
/// records the exact [`OutboundMessage`] it received instead of touching
/// the network.
#[async_trait]
pub trait MessageSender: Send + Sync {
    /// Send `message` over the configured transport. MUST return `Ok(())`
    /// only when the transport genuinely accepted the message for
    /// delivery -- implementations must never fabricate success.
    async fn send(&self, message: &OutboundMessage) -> Result<(), MailError>;
}
