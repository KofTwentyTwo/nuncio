use super::{transfer, ImapCopyProof, ImapTransferMode, ImapTransferStep};
use crate::{
    domain::imap::{ImapFlags, ImapMailboxState, ImapMessageState},
    store::{StagedMail, StoreError},
};
use rusqlite::{params, Connection};

pub struct ImapTransferResult {
    pub proof: ImapCopyProof,
    pub mailbox: ImapMailboxState,
    pub flags: ImapFlags,
    pub mail: StagedMail,
}
pub(in crate::store) fn apply(
    c: &Connection,
    account: &str,
    id: &str,
    mut result: ImapTransferResult,
) -> Result<String, StoreError> {
    let intent = transfer(c, account, id)?;
    if !intent.accepts(&result.proof) {
        return Err(StoreError::InvalidInput);
    }
    let progress = super::progress::progress(c, account, id)?;
    let noop = intent.mode == ImapTransferMode::Move
        && result.proof.source == result.proof.destination
        && intent.source.mailbox_id == intent.destination;
    if noop {
        if progress.phase != ImapTransferStep::Prepared || progress.copy.is_some() {
            return Err(StoreError::InvalidInput);
        }
    } else if progress.copy.as_ref() != Some(&result.proof)
        || progress.phase
            != if intent.mode == ImapTransferMode::Move {
                ImapTransferStep::SourceRemoved
            } else {
                ImapTransferStep::Copied
            }
    {
        return Err(StoreError::InvalidInput);
    }
    let state = result
        .mailbox
        .validated()
        .map_err(|_| StoreError::InvalidInput)?;
    let current = super::mailbox(c, account, &intent.destination.to_string())?;
    if current.name != intent.destination_mailbox
        || current.uid_validity != Some(intent.destination_uid_validity.get())
    {
        return Err(StoreError::VersionConflict);
    }
    if state.name != intent.destination_mailbox
        || state.uid_validity != Some(intent.destination_uid_validity.get())
        || state
            .uid_next
            .is_none_or(|next| next <= result.proof.destination.uid.get())
    {
        return Err(StoreError::InvalidInput);
    }
    let provider = result
        .proof
        .destination
        .provider_id()
        .map_err(|_| StoreError::InvalidInput)?;
    if result.mail.provider_id != provider {
        return Err(StoreError::InvalidInput);
    }
    let flags = serde_json::to_string(&result.flags).map_err(|_| StoreError::InvalidInput)?;
    result.mail.provider_json = serde_json::to_string(&ImapMessageState {
        placement: result.proof.destination,
        flags: result.flags,
    })
    .map_err(|_| StoreError::InvalidInput)?;
    result.mail.labels = vec![intent.destination.to_string()];
    let local = crate::store::mail_projection::upsert(c, account, result.mail)?;
    c.execute("INSERT INTO imap_placements(account_id,message_id,mailbox_id,uid_validity,uid,flags_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(account_id,message_id) DO UPDATE SET flags_json=excluded.flags_json",params![account,local,intent.destination.to_string(),intent.destination_uid_validity.get(),result.proof.destination.uid.get(),flags])?;
    c.execute(
        "UPDATE imap_mailboxes SET state_json=?3 WHERE account_id=?1 AND id=?2",
        params![
            account,
            intent.destination.to_string(),
            serde_json::to_string(&state).map_err(|_| StoreError::InvalidInput)?
        ],
    )?;
    if !noop {
        if let Some(origin) = &intent.restore_origin {
            super::origins::record(c, account, result.proof.destination, origin)?;
        }
    }
    if intent.mode == ImapTransferMode::Move && !noop {
        let source = intent
            .source
            .provider_id()
            .map_err(|_| StoreError::InvalidInput)?;
        if source == provider {
            return Err(StoreError::InvalidInput);
        }
        c.execute("DELETE FROM message_search WHERE account_id=?1 AND message_id IN (SELECT id FROM messages WHERE account_id=?1 AND provider_id=?2)",params![account,source])?;
        c.execute(
            "DELETE FROM imap_trash_origins WHERE account_id=?1 AND provider_id=?2",
            params![account, source],
        )?;
        c.execute(
            "DELETE FROM messages WHERE account_id=?1 AND provider_id=?2",
            params![account, source],
        )?;
    }
    crate::store::changes::record(c, Some(account), "mail", Some(&local))?;
    Ok(provider)
}
