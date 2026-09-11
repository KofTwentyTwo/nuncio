use super::{EnqueueMailChange, Store, StoreError};
use crate::domain::{
    imap::{ImapFlags, ImapMailboxState, ImapMessageState, ImapPlacement, MailboxName},
    mail_change::MailAction,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImapFlag {
    Seen,
    Flagged,
}
impl ImapFlag {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Seen => "\\Seen",
            Self::Flagged => "\\Flagged",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImapFlagPayload {
    pub placement: ImapPlacement,
    pub mailbox: MailboxName,
    pub flag: ImapFlag,
    pub present: bool,
}
impl ImapFlagPayload {
    pub fn satisfied_by(&self, message: &ImapMessageState) -> bool {
        self.placement == message.placement
            && message
                .flags
                .values()
                .iter()
                .any(|f| f == self.flag.wire_name())
                == self.present
    }
}
pub(super) fn capture(
    c: &Connection,
    input: &EnqueueMailChange,
    provider: &str,
) -> Result<super::ImapChangePayload, StoreError> {
    if matches!(
        input.action,
        MailAction::Archive {}
            | MailAction::Move { .. }
            | MailAction::Copy { .. }
            | MailAction::Trash { .. }
    ) {
        Ok(super::ImapChangePayload::Transfer(
            super::imap_transfers::capture(c, input, provider)?,
        ))
    } else {
        Ok(super::ImapChangePayload::Flags(capture_flag(
            c, input, provider,
        )?))
    }
}
fn capture_flag(
    c: &Connection,
    input: &EnqueueMailChange,
    provider: &str,
) -> Result<ImapFlagPayload, StoreError> {
    let placement = ImapPlacement::from_provider_id(
        input
            .account_id
            .parse()
            .map_err(|_| StoreError::InvalidInput)?,
        provider,
    )
    .map_err(|_| StoreError::KeyOrCorrupt)?;
    let encoded: String = c
        .query_row(
            "SELECT state_json FROM imap_mailboxes WHERE account_id=?1 AND id=?2 AND retired=0",
            params![input.account_id, placement.mailbox_id.to_string()],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    let state: ImapMailboxState =
        serde_json::from_str(&encoded).map_err(|_| StoreError::KeyOrCorrupt)?;
    let state = state.validated().map_err(|_| StoreError::KeyOrCorrupt)?;
    if state.uid_validity != Some(placement.uid_validity.get()) {
        return Err(StoreError::VersionConflict);
    }
    let (flag, present) = match input.action {
        MailAction::Read { read } => (ImapFlag::Seen, read),
        MailAction::Star { starred } => (ImapFlag::Flagged, starred),
        _ => return Err(StoreError::InvalidInput),
    };
    Ok(ImapFlagPayload {
        placement,
        mailbox: state.name,
        flag,
        present,
    })
}
impl Store {
    pub async fn imap_flag_payload(
        &self,
        account: String,
        id: String,
    ) -> Result<ImapFlagPayload, StoreError> {
        self.execute(move |c| payload(c, &account, &id)).await
    }
}
fn payload(c: &Connection, account: &str, id: &str) -> Result<ImapFlagPayload, StoreError> {
    let encoded: String = c.query_row(
        "SELECT p.payload_json FROM mail_change_payloads p JOIN accounts a ON a.id=p.account_id WHERE p.account_id=?1 AND p.operation_id=?2 AND a.provider='imap'",
        params![account,id], |r| r.get(0),
    ).optional()?.ok_or(StoreError::NotFound)?;
    let payload: ImapFlagPayload =
        serde_json::from_str(&encoded).map_err(|_| StoreError::KeyOrCorrupt)?;
    if payload.placement.account_id.to_string() != account {
        return Err(StoreError::KeyOrCorrupt);
    }
    Ok(payload)
}
pub(super) fn apply(
    c: &Connection,
    account: &str,
    id: &str,
    message: &ImapMessageState,
) -> Result<String, StoreError> {
    if !payload(c, account, id)?.satisfied_by(message) {
        return Err(StoreError::InvalidInput);
    }
    ImapFlags::new(message.flags.values().to_vec()).map_err(|_| StoreError::InvalidInput)?;
    let provider = message
        .placement
        .provider_id()
        .map_err(|_| StoreError::InvalidInput)?;
    let local: Option<String> = c
        .query_row(
            "SELECT id FROM messages WHERE account_id=?1 AND provider_id=?2",
            params![account, provider],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(local) = local {
        let flags = serde_json::to_string(&message.flags).map_err(|_| StoreError::InvalidInput)?;
        let state = serde_json::to_string(message).map_err(|_| StoreError::InvalidInput)?;
        c.execute(
            "UPDATE imap_placements SET flags_json=?3 WHERE account_id=?1 AND message_id=?2",
            params![account, local, flags],
        )?;
        c.execute(
            "UPDATE messages SET provider_json=?3 WHERE account_id=?1 AND id=?2",
            params![account, local, state],
        )?;
        super::changes::record(c, Some(account), "mail", Some(&local))?;
    }
    Ok(provider)
}
