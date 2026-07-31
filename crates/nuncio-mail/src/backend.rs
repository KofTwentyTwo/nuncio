//! Protocol-agnostic async mail backend trait definitions.

use async_trait::async_trait;
use nuncio_core::model::{Attachment, Email, Folder};

use crate::parser::MailError;

/// The remote change a [`RemoteMutationSpec`] applies to one addressed message.
///
/// This is the closed vocabulary of genuinely-remote filter actions that the
/// outbox drains against a real server. `MARK READ`/`MARK UNREAD` are applied
/// locally to the store when a filter matches, so they are deliberately NOT
/// represented here -- only actions that mutate server-side state are.
/// `FORWARD` and `CALL WEBHOOK` are also excluded: they are not backend
/// mutations (one composes an outbound message, the other calls an HTTP
/// endpoint), so they are executed through the send/webhook seams instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteMutationKind {
    /// Add (`value == true`) or remove (`value == false`) the "flagged"/
    /// starred keyword: IMAP `\Flagged`, JMAP `$flagged`. Covers `FLAG`/`UNFLAG`.
    SetFlagged {
        /// Whether the message should end up flagged.
        value: bool,
    },
    /// Move the message from its source folder to `to_folder`.
    Move {
        /// Destination folder (IMAP mailbox name / JMAP mailbox id).
        to_folder: String,
    },
    /// Copy the message into `to_folder`, leaving the source copy in place.
    Copy {
        /// Destination folder (IMAP mailbox name / JMAP mailbox id).
        to_folder: String,
    },
    /// Delete the message (IMAP `\Deleted` + `EXPUNGE`; JMAP `destroy`).
    Delete,
}

/// A fully-addressed remote mutation for a single message: which message
/// (protocol-native id), the folder it currently lives in, that folder's
/// stored sync checkpoint (carrying IMAP UIDVALIDITY), and the change to
/// apply.
///
/// `folder_checkpoint` is the value persisted by a prior sync
/// (`"{uidvalidity}:{uidnext}"` for IMAP). The IMAP backend uses it to refuse
/// to act when the mailbox's current UIDVALIDITY no longer matches -- a
/// renumbered mailbox reassigns UIDs, so the stored UID would otherwise
/// address the wrong message. Protocols that do not use UIDVALIDITY (JMAP)
/// ignore it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMutationSpec {
    /// Protocol-native message id (IMAP `imap-uid-{uid}`, JMAP object id).
    pub message_id: String,
    /// Folder the message currently lives in.
    pub folder_id: String,
    /// The source folder's stored sync checkpoint, if any.
    pub folder_checkpoint: Option<String>,
    /// The change to apply.
    pub kind: RemoteMutationKind,
}

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

    /// Apply a remote mutation (move/copy/flag/delete) to a single message
    /// against the real server. MUST return `Ok(())` only when the server
    /// genuinely applied the change -- implementations must never fabricate
    /// success, and must refuse to act (returning an error) when the target
    /// message cannot be safely identified (e.g. an IMAP UIDVALIDITY mismatch).
    async fn apply_mutation(&self, spec: &RemoteMutationSpec) -> Result<(), MailError>;
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
