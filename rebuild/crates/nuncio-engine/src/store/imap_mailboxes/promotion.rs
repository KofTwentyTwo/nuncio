use super::{kind, validate_message, ImapMailboxState};
use crate::store::StoreError;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

pub(crate) fn prepare(c: &Connection, account: &str, run: &str) -> Result<(), StoreError> {
    super::check_run(c, account, run)?;
    let unknown:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM staged_collections s WHERE s.account_id=?1 AND s.run_id=?2 AND NOT EXISTS(SELECT 1 FROM staged_imap_mailboxes b WHERE b.account_id=s.account_id AND b.run_id=s.run_id AND b.id=s.provider_id AND b.name=s.name))",params![account,run],|r|r.get(0))?;
    if unknown {
        return Err(StoreError::InvalidInput);
    }
    if crate::store::sync::get(c, account, run)?.mode == "fetch" {
        validate_fetch(c, account, run)?;
    } else {
        c.execute(
            "UPDATE imap_mailboxes SET retired=1 WHERE account_id=?1",
            [account],
        )?;
        let mut stmt=c.prepare("SELECT id,name,state_json FROM staged_imap_mailboxes WHERE account_id=?1 AND run_id=?2 ORDER BY id")?;
        let mut rows = stmt.query(params![account, run])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let json: String = row.get(2)?;
            let state: ImapMailboxState =
                serde_json::from_str(&json).map_err(|_| StoreError::KeyOrCorrupt)?;
            c.execute("INSERT INTO collections(account_id,id,provider_id,name,kind,retired) VALUES(?1,?2,?2,?3,?4,0) ON CONFLICT(account_id,id) DO UPDATE SET name=excluded.name,kind=excluded.kind,retired=0",params![account,id,name,kind(&state)])?;
            c.execute("INSERT INTO imap_mailboxes(account_id,id,name,state_json,retired) VALUES(?1,?2,?3,?4,0) ON CONFLICT(account_id,id) DO UPDATE SET name=excluded.name,state_json=excluded.state_json,retired=0",params![account,id,name,json])?;
        }
        c.execute("UPDATE collections SET retired=1 WHERE account_id=?1 AND id IN (SELECT id FROM imap_mailboxes WHERE account_id=?1 AND retired=1)",[account])?;
    }
    let mut stmt=c.prepare("SELECT provider_id,provider_json FROM staged_messages WHERE account_id=?1 AND run_id=?2 AND deleted=0")?;
    let mut rows = stmt.query(params![account, run])?;
    while let Some(row) = rows.next()? {
        let provider: String = row.get(0)?;
        let json: String = row.get(1)?;
        let data = validate_message(c, account, run, &provider, &json)?;
        let (count,matching):(i64,i64)=c.query_row("SELECT count(*),coalesce(sum(label_id=?4),0) FROM staged_memberships WHERE account_id=?1 AND run_id=?2 AND provider_id=?3",params![account,run,provider,data.placement.mailbox_id.to_string()],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if count != 1 || matching != 1 {
            return Err(StoreError::InvalidInput);
        }
    }
    Ok(())
}
pub(crate) fn finish(c: &Connection, account: &str, run: &str) -> Result<String, StoreError> {
    let mut stmt=c.prepare("SELECT s.provider_id,s.provider_json,m.id FROM staged_messages s JOIN messages m ON m.account_id=s.account_id AND m.provider_id=s.provider_id WHERE s.account_id=?1 AND s.run_id=?2 AND s.deleted=0")?;
    let mut rows = stmt.query(params![account, run])?;
    while let Some(row) = rows.next()? {
        let provider: String = row.get(0)?;
        let json: String = row.get(1)?;
        let id: String = row.get(2)?;
        let data = validate_message(c, account, run, &provider, &json)?;
        c.execute("INSERT INTO imap_placements(account_id,message_id,mailbox_id,uid_validity,uid,flags_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(account_id,message_id) DO UPDATE SET flags_json=excluded.flags_json",params![account,id,data.placement.mailbox_id.to_string(),data.placement.uid_validity.get(),data.placement.uid.get(),serde_json::to_string(&data.flags).map_err(|_|StoreError::InvalidInput)?])?;
    }
    drop(rows);
    drop(stmt);
    let obsolete="SELECT p.message_id FROM imap_placements p JOIN imap_mailboxes b ON b.account_id=p.account_id AND b.id=p.mailbox_id WHERE p.account_id=?1 AND (b.retired=1 OR json_extract(b.state_json,'$.uid_validity') IS NULL OR p.uid_validity<>json_extract(b.state_json,'$.uid_validity'))";
    c.execute(
        &format!("DELETE FROM message_search WHERE account_id=?1 AND message_id IN ({obsolete})"),
        [account],
    )?;
    c.execute(
        &format!("DELETE FROM messages WHERE account_id=?1 AND id IN ({obsolete})"),
        [account],
    )?;
    let mut hash = Sha256::new();
    hash.update(b"imap-snapshot-v1");
    let mut stmt = c.prepare(
        "SELECT id,state_json FROM imap_mailboxes WHERE account_id=?1 AND retired=0 ORDER BY id",
    )?;
    let mut rows = stmt.query([account])?;
    while let Some(row) = rows.next()? {
        hash.update(
            serde_json::to_vec(&(row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                .map_err(|_| StoreError::InvalidInput)?,
        );
    }
    let mut stmt=c.prepare("SELECT mailbox_id,uid_validity,uid,flags_json FROM imap_placements WHERE account_id=?1 ORDER BY mailbox_id,uid_validity,uid")?;
    let mut rows = stmt.query([account])?;
    while let Some(row) = rows.next()? {
        hash.update(
            serde_json::to_vec(&(
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, String>(3)?,
            ))
            .map_err(|_| StoreError::InvalidInput)?,
        );
    }
    Ok(hex::encode(hash.finalize()))
}

fn validate_fetch(c: &Connection, account: &str, run: &str) -> Result<(), StoreError> {
    let (total,matching):(i64,i64)=c.query_row("SELECT count(*),coalesce(sum(s.deleted=0 AND EXISTS(SELECT 1 FROM messages m WHERE m.account_id=s.account_id AND m.provider_id=s.provider_id)),0) FROM staged_messages s WHERE s.account_id=?1 AND s.run_id=?2",params![account,run],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let (folders,matching_folders):(i64,i64)=c.query_row("SELECT count(*),coalesce(sum(EXISTS(SELECT 1 FROM imap_mailboxes b WHERE b.account_id=s.account_id AND b.id=s.id AND b.name=s.name AND b.retired=0 AND json_extract(b.state_json,'$.uid_validity')=json_extract(s.state_json,'$.uid_validity'))),0) FROM staged_imap_mailboxes s WHERE s.account_id=?1 AND s.run_id=?2",params![account,run],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if (total, matching, folders, matching_folders) != (1, 1, 1, 1) {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}
