//! Protocol-agnostic async mail backend trait definitions.

use async_trait::async_trait;
use nuncio_core::model::{Attachment, Email, Folder, IdentitySource, Placement, PlacementKey};

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

/// What one folder-sync pass observed on the server.
///
/// The reason this is not just a list of messages: a sync that can only report
/// what *is* there can never report what stopped being there. A message moved
/// or deleted by any other client -- another Nuncio daemon, a phone, webmail --
/// simply stays in the local store forever. Expressing absence is the whole
/// point of the type.
///
/// Absence is reported as [`PlacementKey`]s, not message keys, because what a
/// folder stops mentioning is an *occupancy*. The same message may still sit in
/// another mailbox, and deleting the message on the strength of one folder's
/// silence would destroy a copy the user still has. A message goes only when
/// its last placement does.
#[derive(Debug, Default, Clone)]
pub struct FolderChanges {
    /// Messages to store, whether newly arrived or changed, each with the
    /// occupancy this pass found it in.
    pub upserts: Vec<PlacedMessage>,
    /// Occupancies the server **explicitly reported as gone**, via QRESYNC
    /// `VANISHED`. Always safe to delete.
    pub removals: Vec<PlacementKey>,
    /// Every occupancy the folder currently holds, when this pass enumerated
    /// the complete UID set. The caller may delete any placement it stores for
    /// this folder that is absent from this list.
    ///
    /// `None` and `Some(vec![])` mean different things, and conflating them
    /// deletes a mailbox. `None` is "this pass was incremental and cannot
    /// speak to absence"; `Some(vec![])` is "the folder is genuinely empty".
    pub present: Option<Vec<PlacementKey>>,
    /// The checkpoint to resume from next time.
    pub next_state: String,
}

/// What a mutation attempt actually established.
///
/// Two states are not enough. A protocol that answers `OK` for a command that
/// did nothing -- which IMAP does, by design, for a UID that no longer exists
/// (RFC 3501 section 6.4.8) -- makes "succeeded" and "failed" an incomplete
/// partition. The missing third state is *the server accepted the command and
/// its response does not establish what happened*, and collapsing that into
/// either neighbour is how a lost mutation gets recorded as done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationOutcome {
    /// The server's response proves the change was made.
    ///
    /// Requires positive evidence: a `COPYUID` naming the destination UIDs, a
    /// removal report for a delete, or an explicit acknowledgement for a flag
    /// change. `token` carries whatever the server returned to address the
    /// result afterwards (the destination UID for a move), when it returned
    /// one at all -- a DAV server that rewrites the object MUST NOT return an
    /// ETag (RFC 4791 section 5.3.4), so absence here is ordinary.
    Applied {
        /// Server-assigned handle for the result, when the response carried one.
        token: Option<String>,
    },
    /// Another client changed the message first, and the server said so.
    ///
    /// Requires positive evidence too: `[MODIFIED …]` (RFC 7162 section 3.1.3)
    /// or an HTTP 412. A mutation that merely failed to find its target is
    /// **not** a conflict -- `MODIFIED` reports "changed by someone else",
    /// never "gone".
    Conflict {
        /// What the server reported about the conflicting state.
        observed: String,
    },
    /// The command was accepted and its response settles nothing.
    ///
    /// The honest reading of an absent `COPYUID` (only `SHOULD` for `MOVE` per
    /// RFC 6851 section 4.3, and legitimately omitted for a `UIDNOTSTICKY`
    /// destination), or of a UID set that matched nothing. The caller must
    /// re-enumerate to find out, not guess.
    Unknown {
        /// Why the response was inconclusive.
        reason: String,
    },
}

/// Protocol-agnostic mail backend engine trait implemented by JMAP and IMAP engines.
#[async_trait]
pub trait MailBackend: Send + Sync {
    /// Synchronize and list available mailbox folders.
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError>;

    /// Enumerate what changed in a folder since `since_state`.
    ///
    /// Replaces a fetch-only sync: implementations must report removals when
    /// the protocol can express them, and must say honestly (via
    /// [`FolderChanges::present`]) whether the pass was able to observe
    /// absence at all.
    async fn sync_changes(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<FolderChanges, MailError>;

    /// Fetch-only view of [`MailBackend::sync_changes`], for callers that
    /// genuinely only want the messages.
    ///
    /// Provided rather than required so no implementation can satisfy the
    /// trait by supplying this and quietly never reporting a removal.
    ///
    /// Every message comes back as a [`PlacedMessage`] so the caller can tell
    /// a message it already stores in another folder from one it has never
    /// seen: the derived key is the same in both folders, only the placement
    /// differs.
    async fn sync_messages(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<(Vec<PlacedMessage>, String), MailError> {
        let changes = self.sync_changes(folder_id, since_state).await?;
        Ok((changes.upserts, changes.next_state))
    }

    /// Apply a remote mutation (move/copy/flag/delete) to a single message
    /// against the real server.
    ///
    /// Returns what the server's response actually proves. Implementations
    /// must never report [`MutationOutcome::Applied`] on the strength of a
    /// tagged `OK` alone, and must refuse to act (returning `Err`) when the
    /// target cannot be safely identified -- e.g. an IMAP UIDVALIDITY
    /// mismatch, where the stored UID now names a different message.
    async fn apply_mutation(&self, spec: &RemoteMutationSpec)
        -> Result<MutationOutcome, MailError>;
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
