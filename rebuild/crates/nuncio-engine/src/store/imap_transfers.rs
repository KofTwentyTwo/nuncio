use super::{EnqueueMailChange, ImapFlagPayload, StoreError};
use crate::domain::{
    identity::ImapMailboxId,
    imap::{ImapMailboxState, ImapPlacement, MailboxName},
    imap_account::ImapAccountConfig,
    mail_change::MailAction,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
mod apply;
mod origins;
mod progress;
pub(super) use apply::apply;
pub use apply::ImapTransferResult;
pub use progress::{ImapTransferProgress, ImapTransferStep};

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ImapChangePayload {
    Flags(ImapFlagPayload),
    Transfer(ImapTransferPayload),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImapTransferMode {
    Copy,
    Move,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapTransferPayload {
    pub source: ImapPlacement,
    pub source_mailbox: MailboxName,
    pub destination: ImapMailboxId,
    pub destination_mailbox: MailboxName,
    pub destination_uid_validity: NonZeroU32,
    pub mode: ImapTransferMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_origin: Option<ImapRestoreOrigin>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapRestoreOrigin {
    pub placement: ImapPlacement,
    pub mailbox: MailboxName,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapCopyProof {
    pub source: ImapPlacement,
    pub destination: ImapPlacement,
}
impl ImapTransferPayload {
    pub fn accepts(&self, proof: &ImapCopyProof) -> bool {
        proof.source == self.source
            && proof.destination.account_id == self.source.account_id
            && proof.destination.mailbox_id == self.destination
            && proof.destination.uid_validity == self.destination_uid_validity
    }
}
pub(super) fn capture(
    c: &Connection,
    input: &EnqueueMailChange,
    provider: &str,
) -> Result<ImapTransferPayload, StoreError> {
    let account = input
        .account_id
        .parse()
        .map_err(|_| StoreError::InvalidInput)?;
    let source =
        ImapPlacement::from_provider_id(account, provider).map_err(|_| StoreError::KeyOrCorrupt)?;
    let source_state = mailbox(c, &input.account_id, &source.mailbox_id.to_string())?;
    if source_state.uid_validity != Some(source.uid_validity.get()) {
        return Err(StoreError::VersionConflict);
    }
    let config = configuration(c, &input.account_id)?;
    let trash = config
        .trash_folder
        .as_deref()
        .map(MailboxName::from_unicode)
        .transpose()
        .map_err(|_| StoreError::KeyOrCorrupt)?;
    let is_trash = trash.as_ref() == Some(&source_state.name);
    let prior_origin = origins::load(c, &input.account_id, source)?;
    let (destination, mode) = match &input.action {
        MailAction::Archive {} => (
            configured_folder(c, &input.account_id, config.archive_folder.as_deref())?,
            ImapTransferMode::Move,
        ),
        MailAction::Trash { trashed: true } => (
            configured_folder(c, &input.account_id, config.trash_folder.as_deref())?,
            ImapTransferMode::Move,
        ),
        MailAction::Trash { trashed: false } => {
            let destination = if let Some(origin) = &prior_origin {
                origins::destination(c, &input.account_id, origin)?
            } else if is_trash {
                return Err(StoreError::VersionConflict);
            } else {
                source.mailbox_id.to_string()
            };
            (destination, ImapTransferMode::Move)
        }
        MailAction::Move {
            destination_collection_id,
        } => (destination_collection_id.clone(), ImapTransferMode::Move),
        MailAction::Copy {
            destination_collection_id,
        } => (destination_collection_id.clone(), ImapTransferMode::Copy),
        _ => return Err(StoreError::InvalidInput),
    };
    let destination_id: ImapMailboxId =
        destination.parse().map_err(|_| StoreError::InvalidInput)?;
    if destination_id.to_string() != destination {
        return Err(StoreError::InvalidInput);
    }
    let target = mailbox(c, &input.account_id, &destination)?;
    let restore_origin = if trash.as_ref() == Some(&target.name) {
        if is_trash {
            prior_origin
        } else {
            Some(ImapRestoreOrigin {
                placement: source,
                mailbox: source_state.name.clone(),
            })
        }
    } else {
        None
    };
    Ok(ImapTransferPayload {
        restore_origin,
        source,
        source_mailbox: source_state.name,
        destination: destination_id,
        destination_mailbox: target.name,
        destination_uid_validity: NonZeroU32::new(
            target.uid_validity.ok_or(StoreError::InvalidInput)?,
        )
        .ok_or(StoreError::InvalidInput)?,
        mode,
    })
}
pub(super) fn configuration(
    c: &Connection,
    account: &str,
) -> Result<ImapAccountConfig, StoreError> {
    let config: String = c
        .query_row(
            "SELECT config FROM imap_accounts WHERE account_id=?1",
            [account],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    let config: ImapAccountConfig =
        serde_json::from_str(&config).map_err(|_| StoreError::KeyOrCorrupt)?;
    Ok(config)
}
pub(super) fn configured_folder(
    c: &Connection,
    account: &str,
    name: Option<&str>,
) -> Result<String, StoreError> {
    let name = MailboxName::from_unicode(name.ok_or(StoreError::NotFound)?)
        .map_err(|_| StoreError::KeyOrCorrupt)?;
    let destination: String = c
        .query_row(
            "SELECT id FROM imap_mailboxes WHERE account_id=?1 AND name=?2 AND retired=0",
            params![account, name.as_str()],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    Ok(destination)
}
pub(super) fn mailbox(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<ImapMailboxState, StoreError> {
    let encoded: String = c
        .query_row(
            "SELECT state_json FROM imap_mailboxes WHERE account_id=?1 AND id=?2 AND retired=0",
            params![account, id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    let state: ImapMailboxState =
        serde_json::from_str(&encoded).map_err(|_| StoreError::KeyOrCorrupt)?;
    state.validated().map_err(|_| StoreError::KeyOrCorrupt)
}
impl super::Store {
    pub async fn imap_change_payload(
        &self,
        account: String,
        id: String,
    ) -> Result<ImapChangePayload, StoreError> {
        self.execute(move |c| payload(c, &account, &id)).await
    }
}
pub(super) fn payload(
    c: &Connection,
    account: &str,
    id: &str,
) -> Result<ImapChangePayload, StoreError> {
    let value:String=c.query_row("SELECT p.payload_json FROM mail_change_payloads p JOIN accounts a ON a.id=p.account_id WHERE p.account_id=?1 AND p.operation_id=?2 AND a.provider='imap'",params![account,id],|r|r.get(0)).optional()?.ok_or(StoreError::NotFound)?;
    let result: ImapChangePayload =
        serde_json::from_str(&value).map_err(|_| StoreError::KeyOrCorrupt)?;
    let owner = match &result {
        ImapChangePayload::Flags(p) => p.placement.account_id,
        ImapChangePayload::Transfer(p) => p.source.account_id,
    };
    if owner.to_string() != account {
        return Err(StoreError::KeyOrCorrupt);
    }
    Ok(result)
}
fn transfer(c: &Connection, account: &str, id: &str) -> Result<ImapTransferPayload, StoreError> {
    match payload(c, account, id)? {
        ImapChangePayload::Transfer(p) => Ok(p),
        _ => Err(StoreError::InvalidInput),
    }
}
