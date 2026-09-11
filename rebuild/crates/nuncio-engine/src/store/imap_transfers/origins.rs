use super::{mailbox, ImapRestoreOrigin};
use crate::{domain::imap::ImapPlacement, store::StoreError};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn load(
    c: &Connection,
    account: &str,
    source: ImapPlacement,
) -> Result<Option<ImapRestoreOrigin>, StoreError> {
    let provider = source.provider_id().map_err(|_| StoreError::InvalidInput)?;
    let value: Option<String> = c
        .query_row(
            "SELECT origin_json FROM imap_trash_origins WHERE account_id=?1 AND provider_id=?2",
            params![account, provider],
            |r| r.get(0),
        )
        .optional()?;
    value
        .map(|value| {
            let origin: ImapRestoreOrigin =
                serde_json::from_str(&value).map_err(|_| StoreError::KeyOrCorrupt)?;
            if origin.placement.account_id != source.account_id {
                return Err(StoreError::KeyOrCorrupt);
            }
            Ok(origin)
        })
        .transpose()
}

pub(super) fn destination(
    c: &Connection,
    account: &str,
    origin: &ImapRestoreOrigin,
) -> Result<String, StoreError> {
    let id = origin.placement.mailbox_id.to_string();
    let target = match mailbox(c, account, &id) {
        Err(StoreError::NotFound) => return Err(StoreError::VersionConflict),
        result => result?,
    };
    if target.name != origin.mailbox
        || target.uid_validity != Some(origin.placement.uid_validity.get())
    {
        return Err(StoreError::VersionConflict);
    }
    Ok(id)
}

pub(super) fn record(
    c: &Connection,
    account: &str,
    destination: ImapPlacement,
    origin: &ImapRestoreOrigin,
) -> Result<(), StoreError> {
    if destination.account_id != origin.placement.account_id
        || destination.account_id.to_string() != account
        || destination == origin.placement
    {
        return Err(StoreError::InvalidInput);
    }
    if load(c, account, destination)?.is_some_and(|prior| prior != *origin) {
        return Err(StoreError::VersionConflict);
    }
    c.execute("INSERT INTO imap_trash_origins(account_id,provider_id,origin_json) VALUES(?1,?2,?3) ON CONFLICT(account_id,provider_id) DO NOTHING",params![account,destination.provider_id().map_err(|_| StoreError::InvalidInput)?,serde_json::to_string(origin).map_err(|_|StoreError::InvalidInput)?])?;
    Ok(())
}
