use super::{operations, Store, StoreError};
use crate::domain::{
    identity::{AccountId, ImapMailboxId},
    imap::{ImapPlacement, MailboxName},
    imap_account::{MailEndpoint, SentPolicy},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
mod apply;
mod progress;
pub use apply::SentCopyResult;
pub(super) use apply::{apply, apply_client_observation};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpIntent {
    pub account_id: AccountId,
    pub endpoint: MailEndpoint,
    pub sent_policy: SentPolicy,
    pub mailbox_id: ImapMailboxId,
    pub mailbox: MailboxName,
    pub uid_validity: NonZeroU32,
    pub uid_next: NonZeroU32,
    #[serde(default)]
    pub sent_fingerprint: Option<crate::domain::submission::SentFingerprint>,
}
impl SmtpIntent {
    pub fn accepts(&self, placement: ImapPlacement) -> bool {
        placement.account_id == self.account_id
            && placement.mailbox_id == self.mailbox_id
            && placement.uid_validity == self.uid_validity
            && placement.uid >= self.uid_next
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpStep {
    Prepared,
    Started,
    Accepted,
    Appending,
    Copied,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmtpProgress {
    pub step: SmtpStep,
    pub placement: Option<ImapPlacement>,
    pub sent_floor: Option<NonZeroU32>,
}
pub struct ServerSentEvidence {
    pub placement: ImapPlacement,
    pub fingerprint: crate::domain::submission::SentFingerprint,
}
pub(super) fn capture(
    c: &Connection,
    account: &str,
    id: &str,
    raw: &[u8],
) -> Result<(), StoreError> {
    let config = super::imap_transfers::configuration(c, account)?;
    let folder = super::imap_transfers::configured_folder(c, account, Some(&config.sent_folder))?;
    let state = super::imap_transfers::mailbox(c, account, &folder)?;
    let intent = SmtpIntent {
        sent_fingerprint: (config.sent_policy == SentPolicy::Server)
            .then(|| {
                crate::domain::submission::SentFingerprint::from_mime(raw)
                    .map_err(|_| StoreError::InvalidInput)
            })
            .transpose()?,
        account_id: account.parse().map_err(|_| StoreError::InvalidAccount)?,
        endpoint: config.smtp,
        sent_policy: config.sent_policy,
        mailbox_id: folder.parse().map_err(|_| StoreError::KeyOrCorrupt)?,
        mailbox: state.name,
        uid_validity: state
            .uid_validity
            .and_then(NonZeroU32::new)
            .ok_or(StoreError::VersionConflict)?,
        uid_next: state
            .uid_next
            .and_then(NonZeroU32::new)
            .ok_or(StoreError::VersionConflict)?,
    };
    c.execute(
        "INSERT INTO smtp_submissions(account_id,operation_id,intent_json) VALUES(?1,?2,?3)",
        params![
            account,
            id,
            serde_json::to_string(&intent).map_err(|_| StoreError::InvalidInput)?
        ],
    )?;
    Ok(())
}
pub(super) fn intent(c: &Connection, account: &str, id: &str) -> Result<SmtpIntent, StoreError> {
    let value: String = c
        .query_row(
            "SELECT intent_json FROM smtp_submissions WHERE account_id=?1 AND operation_id=?2",
            params![account, id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    let value: SmtpIntent = serde_json::from_str(&value).map_err(|_| StoreError::KeyOrCorrupt)?;
    if value.account_id.to_string() != account {
        return Err(StoreError::KeyOrCorrupt);
    }
    Ok(value)
}
pub(super) fn progress(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<SmtpProgress, StoreError> {
    let (step, placement, floor):(String,Option<String>,Option<u32>)=c.query_row("SELECT step,placement_json,sent_floor FROM smtp_submissions WHERE account_id=?1 AND operation_id=?2",params![account,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or(StoreError::NotFound)?;
    Ok(SmtpProgress {
        sent_floor: floor
            .map(|v| NonZeroU32::new(v).ok_or(StoreError::KeyOrCorrupt))
            .transpose()?,
        step: serde_json::from_value(step.into()).map_err(|_| StoreError::KeyOrCorrupt)?,
        placement: placement
            .map(|v| serde_json::from_str(&v).map_err(|_| StoreError::KeyOrCorrupt))
            .transpose()?,
    })
}
pub(super) fn active(
    c: &Connection,
    account: &str,
    id: &str,
    ordinal: u32,
    now: i64,
) -> Result<i64, StoreError> {
    let op = operations::get(c, account, id)?;
    let (kind,finished):(String,Option<i64>)=c.query_row("SELECT kind,finished_at_ms FROM operation_attempts WHERE account_id=?1 AND operation_id=?2 AND ordinal=?3",params![account,id,ordinal],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if now < 0
        || op.kind != "send"
        || finished.is_some()
        || op.disposition.is_some()
        || !matches!(
            (kind.as_str(), op.state.as_str()),
            ("dispatch", "running") | ("reconcile", "uncertain")
        )
    {
        return Err(StoreError::VersionConflict);
    }
    intent(c, account, id)?;
    Ok(now.max(op.updated_at_ms))
}
pub(super) fn touch(c: &Connection, account: &str, id: &str, now: i64) -> Result<(), StoreError> {
    c.execute(
        "UPDATE operations SET version=version+1,updated_at_ms=?3 WHERE account_id=?1 AND id=?2",
        params![account, id, now],
    )?;
    super::changes::record(c, Some(account), "operation", Some(id))?;
    Ok(())
}
impl Store {
    pub async fn smtp_intent(&self, account: String, id: String) -> Result<SmtpIntent, StoreError> {
        self.execute(move |c| intent(c, &account, &id)).await
    }
    pub async fn smtp_progress(
        &self,
        account: String,
        id: String,
    ) -> Result<SmtpProgress, StoreError> {
        self.execute(move |c| progress(c, &account, &id)).await
    }
}
