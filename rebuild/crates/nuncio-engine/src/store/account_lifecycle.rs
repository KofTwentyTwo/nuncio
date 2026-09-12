use super::{accounts, changes, Store, StoreError, StoredAccount};
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Clone, Copy)]
pub enum AccountLifecycle {
    Pause,
    Resume,
    Archive,
    Restore,
}

#[derive(Serialize)]
pub struct AccountPurgePreview {
    pub account_id: String,
    pub version: u64,
    pub revision: u64,
    pub archived: bool,
    pub messages: u64,
    pub calendars: u64,
    pub events: u64,
    pub drafts: u64,
    pub blobs: u64,
    pub blob_bytes: u64,
    pub operations: u64,
    pub queued_operations: u64,
    pub unresolved_operations: u64,
}

// Explicitly account-scoped tables; change_log is redacted without breaking revision replay.
const PURGE_TABLES: &[&str] = &[
    "message_search",
    "draft_upload_chunks",
    "draft_uploads",
    "draft_attachments",
    "drafts",
    "operation_receipts",
    "operation_resolutions",
    "operation_attempts",
    "operation_reconciliation_requests",
    "restored_operations",
    "smtp_submissions",
    "imap_transfer_progress",
    "calendar_change_payloads",
    "mail_change_payloads",
    "send_payloads",
    "operations",
    "staged_calendar_repair_events",
    "staged_calendar_repair_ids",
    "staged_calendar_events",
    "staged_calendar_catalog",
    "agenda_coverage",
    "calendar_occurrences",
    "calendar_objects",
    "calendar_event_ids",
    "calendars",
    "staged_imap_mailboxes",
    "staged_attachments",
    "staged_memberships",
    "staged_headers",
    "staged_messages",
    "staged_collections",
    "imap_trash_origins",
    "imap_placements",
    "imap_mailboxes",
    "attachments",
    "message_headers",
    "memberships",
    "messages",
    "collections",
    "sync_schedules",
    "sync_scopes",
    "sync_runs",
    "imap_accounts",
    "blob_chunks",
    "blobs",
];

impl Store {
    pub async fn edit_account_name(
        &self,
        id: String,
        version: u64,
        name: String,
    ) -> Result<StoredAccount, StoreError> {
        if name.len() > 256 || name.chars().any(char::is_control) || name.trim() != name {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move |c| {
            let tx = c.transaction()?;
            let old = accounts::get(&tx, &id)?.ok_or(StoreError::NotFound)?;
            if old.version != version {
                return Err(StoreError::VersionConflict);
            }
            if old.display_name != name {
                tx.execute(
                    "UPDATE accounts SET display_name=?2,version=version+1 WHERE id=?1",
                    params![id, name],
                )?;
                changes::record(&tx, Some(&id), "account_edited", None)?;
            }
            let updated = accounts::get(&tx, &id)?.ok_or(StoreError::NotFound)?;
            tx.commit()?;
            Ok(updated)
        })
        .await
    }

    pub async fn change_account_lifecycle(
        &self,
        id: String,
        action: AccountLifecycle,
    ) -> Result<StoredAccount, StoreError> {
        self.execute(move |c| {
            let tx = c.transaction()?;
            let old = accounts::get(&tx, &id)?.ok_or(StoreError::NotFound)?;
            let (sql, kind, changed) = match action {
                AccountLifecycle::Pause | AccountLifecycle::Resume if old.archived => return Err(StoreError::AccountLifecycle),
                AccountLifecycle::Pause => ("UPDATE accounts SET paused=1,version=version+1 WHERE id=?1", "account_paused", !old.paused),
                AccountLifecycle::Resume => ("UPDATE accounts SET paused=0,version=version+1 WHERE id=?1", "account_resumed", old.paused),
                AccountLifecycle::Archive => ("UPDATE accounts SET archived=1,paused=0,state='disconnected',credential_ref=NULL,version=version+1 WHERE id=?1", "account_archived", !old.archived),
                AccountLifecycle::Restore => ("UPDATE accounts SET archived=0,version=version+1 WHERE id=?1", "account_restored", old.archived),
            };
            if changed {
                if matches!(action, AccountLifecycle::Archive) {
                    if let Some(reference) = old.credential_ref {
                        tx.execute("INSERT OR IGNORE INTO credential_cleanup(reference) VALUES (?1)", [reference])?;
                    }
                }
                tx.execute(sql, [&id])?;
                changes::record(&tx, Some(&id), kind, None)?;
            }
            let updated = accounts::get(&tx, &id)?.ok_or(StoreError::NotFound)?;
            tx.commit()?;
            Ok(updated)
        }).await
    }

    pub async fn preview_account_purge(
        &self,
        id: String,
    ) -> Result<AccountPurgePreview, StoreError> {
        self.execute(move |c| preview(c, &id)).await
    }

    pub async fn purge_account(
        &self,
        id: String,
        version: u64,
        revision: u64,
    ) -> Result<AccountPurgePreview, StoreError> {
        self.execute(move |c| {
            let tx = c.transaction()?;
            let current = preview(&tx, &id)?;
            if current.version != version || current.revision != revision { return Err(StoreError::VersionConflict); }
            if !current.archived || current.unresolved_operations != 0 { return Err(StoreError::AccountLifecycle); }
            tx.execute_batch("PRAGMA defer_foreign_keys=ON")?;
            for table in PURGE_TABLES {
                tx.execute(&format!("DELETE FROM {table} WHERE account_id=?1"), [&id])?;
            }
            tx.execute("UPDATE change_log SET account_id=NULL,resource_id=NULL,kind='account_history_removed' WHERE account_id=?1", [&id])?;
            tx.execute("DELETE FROM accounts WHERE id=?1", [&id])?;
            changes::record(&tx, None, "account_purged", Some(&id))?;
            let broken = tx.prepare("PRAGMA foreign_key_check")?.exists([])?;
            if broken { return Err(StoreError::KeyOrCorrupt); }
            tx.commit()?;
            Ok(current)
        }).await
    }
}

fn preview(c: &Connection, id: &str) -> Result<AccountPurgePreview, StoreError> {
    let row = accounts::get(c, id)?.ok_or(StoreError::NotFound)?;
    let count = |table: &str| -> Result<u64, StoreError> {
        Ok(c.query_row(
            &format!("SELECT count(*) FROM {table} WHERE account_id=?1"),
            [id],
            count_value,
        )?)
    };
    Ok(AccountPurgePreview {
        account_id: id.into(), version: row.version, revision: super::worker::status(c)?.revision, archived: row.archived,
        messages: count("messages")?, calendars: count("calendars")?, events: count("calendar_objects")?,
        drafts: count("drafts")?, blobs: count("blobs")?, operations: count("operations")?,
        blob_bytes: c.query_row("SELECT coalesce(sum(byte_length),0) FROM blobs WHERE account_id=?1", [id], count_value)?,
        queued_operations: c.query_row("SELECT count(*) FROM operations WHERE account_id=?1 AND state IN ('queued','retry_wait')", [id], count_value)?,
        unresolved_operations: c.query_row("SELECT count(*) FROM operations WHERE account_id=?1 AND (state='running' OR (state IN ('uncertain','conflict') AND disposition IS NULL) OR (needs_reconciliation=1 AND disposition IS NULL))", [id], count_value)?,
    })
}

fn count_value(row: &rusqlite::Row<'_>) -> rusqlite::Result<u64> {
    u64::try_from(row.get::<_, i64>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn purge_covers_every_account_scoped_table() {
        let mut c = rusqlite::Connection::open_in_memory().unwrap();
        super::super::migrations::migrate(&mut c, &super::super::StoreOpenOptions::default())
            .unwrap();
        let actual: std::collections::BTreeSet<String> = c.prepare("SELECT s.name FROM sqlite_schema s WHERE s.type='table' AND s.name<>'change_log' AND EXISTS(SELECT 1 FROM pragma_table_info(s.name) p WHERE p.name='account_id')").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
        let expected = super::PURGE_TABLES.iter().map(|s| s.to_string()).collect();
        assert_eq!(actual, expected);
    }
}
