//! Protocol-agnostic async mail backend trait definitions.

use async_trait::async_trait;
use nuncio_core::model::{Attachment, Email, Folder, IdentitySource, Placement};

use crate::parser::MailError;

/// One message as a backend surfaced it: identity, content, and the mailbox
/// occupancy it was found in.
///
/// Backends emit this rather than a bare [`Email`] because a fetch observes two
/// separate facts at once: *which message this is* -- folder-independent, and
/// the same in every mailbox that holds a copy -- and *where this pass found
/// it*, which is per-mailbox and changes under a move. Collapsing them into one
/// record is what made a moved message read as a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedMessage {
    /// Identity and content. `email.id` is the derived message key.
    pub email: Email,
    /// Which precedence tier produced `email.id`.
    pub source: IdentitySource,
    /// The occupancy this fetch found it in.
    pub placement: Placement,
}

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

/// A fully-addressed remote mutation for a single message: the protocol-native
/// id to address it on the server, the folder it currently lives in, the
/// UIDVALIDITY scope that id was captured under, and the change to apply.
///
/// Addressing is driven by `remote_id` (the IMAP UID / JMAP object id), NOT by
/// the opaque `message_id` -- so remote-mutation targeting never depends on the
/// shape of the surrogate id. `uid_validity` is the mailbox UIDVALIDITY the
/// `remote_id` was captured under; the IMAP backend refuses to act unless the
/// mailbox's current UIDVALIDITY still matches it, because a renumbered mailbox
/// (RFC 3501 s2.3.1.1) reassigns UIDs and the stored UID would otherwise
/// address the wrong message. Protocols without a UIDVALIDITY (JMAP) carry a
/// stable sentinel and ignore this guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMutationSpec {
    /// Opaque surrogate id of the target message. Diagnostic only -- it names
    /// the message in errors/logs and is never used to address it on the wire.
    pub message_id: String,
    /// Protocol-native id used to address the message on its server: the IMAP
    /// UID as a decimal string, or the JMAP Email object id.
    pub remote_id: String,
    /// Folder the message currently lives in.
    pub folder_id: String,
    /// The UIDVALIDITY the `remote_id` was captured under (IMAP; decimal
    /// string), or a protocol sentinel for servers without UIDVALIDITY (JMAP).
    pub uid_validity: String,
    /// The change to apply.
    pub kind: RemoteMutationKind,
}

/// Protocol-agnostic mail backend engine trait implemented by JMAP and IMAP engines.
#[async_trait]
pub trait MailBackend: Send + Sync {
    /// Synchronize and list available mailbox folders.
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError>;

    /// Synchronize message envelopes for a specific folder since a checkpoint state.
    /// Returns the messages with the occupancy each was found in, and the new
    /// server state checkpoint.
    ///
    /// Every message is reported as a [`PlacedMessage`] so the caller can tell
    /// a message it already stores in another folder from one it has never
    /// seen: the derived key is the same in both folders, only the placement
    /// differs.
    async fn sync_messages(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<(Vec<PlacedMessage>, String), MailError>;

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
