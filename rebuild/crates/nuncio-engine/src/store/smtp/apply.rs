use super::*;
use crate::{
    domain::imap::{ImapFlags, ImapMailboxState, ImapMessageState},
    store::StagedMail,
};
use sha2::{Digest, Sha256};

pub struct SentCopyResult {
    pub placement: ImapPlacement,
    pub mailbox: ImapMailboxState,
    pub flags: ImapFlags,
    pub mail: StagedMail,
}
pub(in crate::store) fn apply_client_observation(
    c: &Connection,
    account: &str,
    id: &str,
    result: SentCopyResult,
) -> Result<String, StoreError> {
    let intent = intent(c, account, id)?;
    let prior = progress(c, account, id)?;
    if intent.sent_policy != SentPolicy::ClientAppend
        || !matches!(prior.step, SmtpStep::Accepted | SmtpStep::Appending)
        || !intent.accepts(result.placement)
        || prior
            .sent_floor
            .is_none_or(|floor| result.placement.uid < floor)
    {
        return Err(StoreError::InvalidInput);
    }
    // The caller commits proof, progress, projection, and completion together.
    c.execute(
        "UPDATE smtp_submissions SET step='copied',placement_json=?3 WHERE account_id=?1 AND operation_id=?2",
        params![account,id,serde_json::to_string(&result.placement).map_err(|_|StoreError::InvalidInput)?],
    )?;
    apply(c, account, id, result)
}
pub(in crate::store) fn apply(
    c: &Connection,
    account: &str,
    id: &str,
    mut result: SentCopyResult,
) -> Result<String, StoreError> {
    let intent = intent(c, account, id)?;
    let progress = progress(c, account, id)?;
    if progress.step != SmtpStep::Copied
        || progress.placement != Some(result.placement)
        || !intent.accepts(result.placement)
    {
        return Err(StoreError::InvalidInput);
    }
    let state = result
        .mailbox
        .validated()
        .map_err(|_| StoreError::InvalidInput)?;
    let current =
        super::super::imap_transfers::mailbox(c, account, &intent.mailbox_id.to_string())?;
    if current.name != intent.mailbox || current.uid_validity != Some(intent.uid_validity.get()) {
        return Err(StoreError::VersionConflict);
    }
    if state.name != intent.mailbox
        || state.uid_validity != Some(intent.uid_validity.get())
        || state
            .uid_next
            .is_none_or(|n| n <= result.placement.uid.get())
    {
        return Err(StoreError::InvalidInput);
    }
    let provider = result
        .placement
        .provider_id()
        .map_err(|_| StoreError::InvalidInput)?;
    if provider != result.mail.provider_id {
        return Err(StoreError::InvalidInput);
    }
    // Client APPEND retains the exact frozen Sent MIME, including its Bcc copy.
    // A UID proof alone must not publish different bytes after a server defect.
    if intent.sent_policy == SentPolicy::ClientAppend {
        let expected:String=c.query_row("SELECT b.sha256 FROM send_payloads p JOIN blobs b ON b.account_id=p.account_id AND b.id=COALESCE(p.sent_blob_id,p.wire_blob_id) WHERE p.account_id=?1 AND p.operation_id=?2",params![account,id],|r|r.get(0))?;
        let raw = result.mail.raw.as_ref().ok_or(StoreError::InvalidInput)?;
        if hex::encode(Sha256::digest(raw)) != expected {
            return Err(StoreError::InvalidInput);
        }
    } else {
        let observed = crate::domain::submission::SentFingerprint::from_mime(
            result.mail.raw.as_deref().ok_or(StoreError::InvalidInput)?,
        )
        .map_err(|_| StoreError::InvalidInput)?;
        if progress
            .sent_floor
            .is_none_or(|floor| result.placement.uid < floor)
            || intent
                .sent_fingerprint
                .as_ref()
                .is_none_or(|expected| !expected.matches(&observed))
        {
            return Err(StoreError::InvalidInput);
        }
    }
    let flags = serde_json::to_string(&result.flags).map_err(|_| StoreError::InvalidInput)?;
    result.mail.provider_json = serde_json::to_string(&ImapMessageState {
        placement: result.placement,
        flags: result.flags,
    })
    .map_err(|_| StoreError::InvalidInput)?;
    result.mail.labels = vec![intent.mailbox_id.to_string()];
    let local = super::super::mail_projection::upsert(c, account, result.mail)?;
    c.execute("INSERT INTO imap_placements(account_id,message_id,mailbox_id,uid_validity,uid,flags_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(account_id,message_id) DO UPDATE SET flags_json=excluded.flags_json",params![account,local,intent.mailbox_id.to_string(),intent.uid_validity.get(),result.placement.uid.get(),flags])?;
    c.execute(
        "UPDATE imap_mailboxes SET state_json=?3 WHERE account_id=?1 AND id=?2",
        params![
            account,
            intent.mailbox_id.to_string(),
            serde_json::to_string(&state).map_err(|_| StoreError::InvalidInput)?
        ],
    )?;
    super::super::changes::record(c, Some(account), "mail", Some(&local))?;
    Ok(provider)
}
