use super::{mail_staging::running, sync, Store, StoreError, GMAIL_QUERY_FINGERPRINT};
use rusqlite::{params, Connection};
impl Store {
    pub async fn promote_mail(
        &self,
        account: String,
        run: String,
        cursor: Option<String>,
        now: i64,
    ) -> Result<(), StoreError> {
        if cursor
            .as_ref()
            .is_some_and(|c| c.is_empty() || c.len() > 8192)
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;running(&tx,&account,&run)?;
            let sync_run=sync::get(&tx,&account,&run)?;
            let mode=sync_run.mode;
            let provider:String=tx.query_row("SELECT provider FROM accounts WHERE id=?1",[&account],|r|r.get(0))?;
            let imap=provider=="imap";
            if sync_run.scope!=if imap {"imap"} else {"gmail"} {return Err(StoreError::InvalidInput);}
            if !imap && mode!="fetch"&&cursor.is_none(){return Err(StoreError::InvalidInput)}
            if imap {super::imap_mailboxes::prepare(&tx,&account,&run)?;}
            // Labels unknown to discovery still retain their provider identity.
            let mut labels=tx.prepare("SELECT provider_id,name,kind FROM staged_collections WHERE account_id=?1 AND run_id=?2 UNION SELECT label_id,NULL,'unknown' FROM staged_memberships WHERE account_id=?1 AND run_id=?2 AND label_id NOT IN (SELECT provider_id FROM staged_collections WHERE account_id=?1 AND run_id=?2)")?;
            let mut rows=labels.query(params![account,run])?;
            while let Some(row)=rows.next()? {
                let provider:String=row.get(0)?;let name:Option<String>=row.get(1)?;let kind:String=row.get(2)?;
                tx.execute("INSERT INTO collections(account_id,id,provider_id,name,kind,retired) VALUES (?1,?2,?3,?4,?5,0) ON CONFLICT(account_id,provider_id) DO UPDATE SET name=coalesce(excluded.name,collections.name),kind=CASE WHEN excluded.kind='unknown' THEN collections.kind ELSE excluded.kind END,retired=0",
                    params![account,uuid::Uuid::new_v4().to_string(),provider,name,kind])?;
            }
            drop(rows);drop(labels);
            let full=mode=="full";
            if full {
                // A full generation also repairs search rows with no surviving message.
                tx.execute("DELETE FROM message_search WHERE account_id=?1",[&account])?;
            } else {
                // FTS account/message columns are unindexed. Clear affected rows once;
                // repeating a full index scan per message starves the store worker.
                tx.execute("DELETE FROM message_search WHERE account_id=?1 AND message_id IN (SELECT m.id FROM messages m JOIN staged_messages s ON s.account_id=m.account_id AND s.provider_id=m.provider_id WHERE s.account_id=?1 AND s.run_id=?2)",params![account,run])?;
            }
            let mut items=tx.prepare("SELECT provider_id FROM staged_messages WHERE account_id=?1 AND run_id=?2 AND deleted=0 ORDER BY provider_id")?;
            let mut rows=items.query(params![account,run])?;
            while let Some(row)=rows.next()? {let provider:String=row.get(0)?;promote_one(&tx,&account,&run,&provider)?;}
            drop(rows);drop(items);
            // Only a completed full generation is authoritative for absence.
            tx.execute("DELETE FROM messages WHERE account_id=?1 AND ((?3 AND NOT EXISTS(SELECT 1 FROM staged_messages s WHERE s.account_id=?1 AND s.run_id=?2 AND s.provider_id=messages.provider_id AND s.deleted=0)) OR EXISTS(SELECT 1 FROM staged_messages s WHERE s.account_id=?1 AND s.run_id=?2 AND s.provider_id=messages.provider_id AND s.deleted=1))",params![account,run,full])?;
            if full {
                tx.execute("UPDATE collections SET retired=1 WHERE account_id=?1 AND provider_id NOT IN (SELECT provider_id FROM staged_collections WHERE account_id=?1 AND run_id=?2 UNION SELECT label_id FROM staged_memberships WHERE account_id=?1 AND run_id=?2)",params![account,run])?;
            }
            let cursor=if imap {Some(super::imap_mailboxes::finish(&tx,&account,&run)?)} else {cursor};
            if mode!="fetch" {
                let (scope,kind,fingerprint)=if imap {("imap","imap_snapshot","imap:all:v1")} else {("gmail","gmail_history",GMAIL_QUERY_FINGERPRINT)};
                tx.execute("INSERT INTO sync_scopes(account_id,scope,cursor_kind,cursor,query_fingerprint,active_generation,synchronized_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(account_id,scope) DO UPDATE SET cursor_kind=excluded.cursor_kind,cursor=excluded.cursor,query_fingerprint=excluded.query_fingerprint,active_generation=excluded.active_generation,synchronized_at_ms=excluded.synchronized_at_ms",params![account,scope,kind,cursor,fingerprint,run,now])?;
            }
            tx.execute("UPDATE sync_runs SET state='succeeded',finished_at_ms=?3,page_token=NULL WHERE account_id=?1 AND id=?2",params![account,run,now])?;
            tx.execute("DELETE FROM staged_messages WHERE account_id=?1 AND run_id=?2",params![account,run])?;
            tx.execute("DELETE FROM staged_collections WHERE account_id=?1 AND run_id=?2",params![account,run])?;
            tx.execute("DELETE FROM staged_imap_mailboxes WHERE account_id=?1 AND run_id=?2",params![account,run])?;
            super::changes::record(&tx,Some(&account),"mail",Some(&run))?;
            super::schedules::success(&tx,&account,&run,now)?;
            tx.commit()?;Ok(())
        }).await
    }
}
fn promote_one(c: &Connection, account: &str, run: &str, provider: &str) -> Result<(), StoreError> {
    c.execute("INSERT INTO messages(account_id,id,provider_id,thread_id,history_id,internal_date_ms,subject,provider_json,raw_blob_id,text_blob_id,html_blob_id,body_availability) SELECT account_id,?4,provider_id,thread_id,history_id,internal_date_ms,subject,provider_json,raw_blob_id,text_blob_id,html_blob_id,body_availability FROM staged_messages WHERE account_id=?1 AND run_id=?2 AND provider_id=?3 ON CONFLICT(account_id,provider_id) DO UPDATE SET thread_id=excluded.thread_id,history_id=excluded.history_id,internal_date_ms=excluded.internal_date_ms,subject=excluded.subject,provider_json=excluded.provider_json,raw_blob_id=excluded.raw_blob_id,text_blob_id=excluded.text_blob_id,html_blob_id=excluded.html_blob_id,body_availability=excluded.body_availability",params![account,run,provider,uuid::Uuid::new_v4().to_string()])?;
    let id: String = c.query_row(
        "SELECT id FROM messages WHERE account_id=?1 AND provider_id=?2",
        params![account, provider],
        |r| r.get(0),
    )?;
    c.execute(
        "DELETE FROM message_headers WHERE account_id=?1 AND message_id=?2",
        params![account, id],
    )?;
    c.execute("INSERT INTO message_headers SELECT account_id,?4,ordinal,name,value FROM staged_headers WHERE account_id=?1 AND run_id=?2 AND provider_id=?3",params![account,run,provider,id])?;
    c.execute(
        "DELETE FROM memberships WHERE account_id=?1 AND message_id=?2",
        params![account, id],
    )?;
    c.execute("INSERT INTO memberships SELECT s.account_id,?4,c.id FROM staged_memberships s JOIN collections c ON c.account_id=s.account_id AND c.provider_id=s.label_id WHERE s.account_id=?1 AND s.run_id=?2 AND s.provider_id=?3",params![account,run,provider,id])?;
    let mut attachments=c.prepare("SELECT part_index,filename,mime_type,content_id,blob_id FROM staged_attachments WHERE account_id=?1 AND run_id=?2 AND provider_id=?3")?;
    let mut rows = attachments.query(params![account, run, provider])?;
    while let Some(row) = rows.next()? {
        c.execute("INSERT INTO attachments(account_id,id,message_id,part_index,filename,mime_type,content_id,blob_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(account_id,message_id,part_index) DO UPDATE SET filename=excluded.filename,mime_type=excluded.mime_type,content_id=excluded.content_id,blob_id=excluded.blob_id",
            params![account,uuid::Uuid::new_v4().to_string(),id,row.get::<_,u32>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,String>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,String>(4)?])?;
    }
    c.execute("DELETE FROM attachments WHERE account_id=?1 AND message_id=?4 AND part_index NOT IN (SELECT part_index FROM staged_attachments WHERE account_id=?1 AND run_id=?2 AND provider_id=?3)",params![account,run,provider,id])?;
    c.execute("INSERT INTO message_search(account_id,message_id,subject,body) SELECT account_id,?4,subject,search_text FROM staged_messages WHERE account_id=?1 AND run_id=?2 AND provider_id=?3",params![account,run,provider,id])?;
    Ok(())
}
