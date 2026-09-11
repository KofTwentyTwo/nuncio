use super::{Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct SyncRun {
    pub id: String,
    pub account_id: String,
    pub scope: String,
    pub mode: String,
    pub state: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub processed: u64,
    pub error_code: Option<String>,
}
impl Store {
    pub async fn reset_mail_run(&self, account: String, id: String) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;
            let changed=tx.execute("UPDATE sync_runs SET mode='full',state='queued',start_cursor=NULL,page_token=NULL,processed=0 WHERE account_id=?1 AND id=?2 AND state='running' AND scope='gmail'",params![account,id])?;
            if changed!=1 {return Err(StoreError::InvalidInput)}
            tx.execute("DELETE FROM staged_messages WHERE account_id=?1 AND run_id=?2",params![account,id])?;
            tx.execute("DELETE FROM staged_collections WHERE account_id=?1 AND run_id=?2",params![account,id])?;
            super::changes::record(&tx,Some(&account),"sync_run",Some(&id))?;
            tx.commit()?;Ok(())
        }).await
    }
    pub async fn recover_sync_runs(&self, now: i64) -> Result<(), StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;
            let interrupted={let mut stmt=tx.prepare("SELECT account_id,id FROM sync_runs WHERE state IN ('queued','running')")?;let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;rows};
            tx.execute("UPDATE sync_runs SET state='failed',error_code='interrupted',finished_at_ms=?1,page_token=NULL WHERE state IN ('queued','running')",[now])?;
            for (account,id) in interrupted {
                super::schedules::failed(&tx,&account,&id,now)?;
                super::changes::record(&tx,Some(&account),"sync_run",Some(&id))?;
            }
            tx.execute("DELETE FROM staged_calendar_catalog WHERE EXISTS(SELECT 1 FROM sync_runs r WHERE r.account_id=staged_calendar_catalog.account_id AND r.id=staged_calendar_catalog.run_id AND r.state IN ('failed','cancelled'))",[])?;
            tx.execute("DELETE FROM staged_calendar_events WHERE EXISTS(SELECT 1 FROM sync_runs r WHERE r.account_id=staged_calendar_events.account_id AND r.id=staged_calendar_events.run_id AND r.state IN ('failed','cancelled'))",[])?;
            tx.execute("DELETE FROM staged_messages WHERE EXISTS(SELECT 1 FROM sync_runs r WHERE r.account_id=staged_messages.account_id AND r.id=staged_messages.run_id AND r.state IN ('failed','cancelled'))",[])?;
            tx.execute("DELETE FROM staged_collections WHERE EXISTS(SELECT 1 FROM sync_runs r WHERE r.account_id=staged_collections.account_id AND r.id=staged_collections.run_id AND r.state IN ('failed','cancelled'))",[])?;
            tx.execute("DELETE FROM staged_imap_mailboxes WHERE EXISTS(SELECT 1 FROM sync_runs r WHERE r.account_id=staged_imap_mailboxes.account_id AND r.id=staged_imap_mailboxes.run_id AND r.state IN ('failed','cancelled'))",[])?;
            tx.commit()?;Ok(())
        }).await
    }
    pub async fn start_mail_run(
        &self,
        account: String,
        mode: String,
        now: i64,
    ) -> Result<SyncRun, StoreError> {
        let query = account.clone();
        let provider = self
            .execute(move |c| {
                c.query_row("SELECT provider FROM accounts WHERE id=?1", [query], |r| {
                    r.get::<_, String>(0)
                })
                .map_err(StoreError::from)
            })
            .await?;
        self.start_run(
            account,
            if provider == "imap" { "imap" } else { "gmail" }.into(),
            mode,
            now,
        )
        .await
    }
    pub async fn start_calendar_run(
        &self,
        account: String,
        now: i64,
    ) -> Result<SyncRun, StoreError> {
        self.start_run(account, "calendar".into(), "full".into(), now)
            .await
    }
    async fn start_run(
        &self,
        account: String,
        scope: String,
        mode: String,
        now: i64,
    ) -> Result<SyncRun, StoreError> {
        if !matches!(mode.as_str(), "full" | "delta" | "fetch") {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|connection|{
            let transaction=connection.transaction()?;
            let active:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM sync_runs WHERE account_id=?1 AND scope=?2 AND state IN ('queued','running'))",
                params![account,scope],|row|row.get(0))?;
            if active {return Err(StoreError::InvalidInput);}
            let id=uuid::Uuid::new_v4().to_string();
            transaction.execute("INSERT INTO sync_runs(account_id,id,scope,mode,state,started_at_ms) VALUES (?1,?2,?5,?3,'queued',?4)",
                params![account,id,mode,now,scope])?;
            let run=get(&transaction,&account,&id)?;
            super::schedules::started(&transaction,&account,&id)?;
            super::changes::record(&transaction,Some(&account),"sync_run",Some(&id))?;
            transaction.commit()?;
            Ok(run)
        }).await
    }
    pub async fn sync_run(&self, account: String, id: String) -> Result<SyncRun, StoreError> {
        self.execute(move |connection| get(connection, &account, &id))
            .await
    }
    pub async fn begin_sync_run(
        &self,
        account: String,
        id: String,
        cursor: Option<String>,
    ) -> Result<(), StoreError> {
        if cursor
            .as_ref()
            .is_some_and(|c| c.is_empty() || c.len() > 8192)
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|connection|{
            let transaction=connection.transaction()?;
            let run = get(&transaction, &account, &id)?;
            if run.scope=="gmail" && run.mode != "fetch" && cursor.is_none() {
                return Err(StoreError::InvalidInput);
            }
            let changed=transaction.execute("UPDATE sync_runs SET state='running',start_cursor=?1 WHERE account_id=?2 AND id=?3 AND state='queued'",
                params![cursor,account,id])?;
            if changed!=1 {return Err(StoreError::InvalidInput);}
            super::changes::record(&transaction,Some(&account),"sync_run",Some(&id))?;
            transaction.commit()?;
            Ok(())
        }).await
    }
    pub async fn sync_run_progress(
        &self,
        account: String,
        id: String,
        processed: u64,
        page_token: Option<String>,
    ) -> Result<(), StoreError> {
        let processed = i64::try_from(processed).map_err(|_| StoreError::InvalidInput)?;
        if page_token
            .as_ref()
            .is_some_and(|t| t.is_empty() || t.len() > 8192)
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|connection|{
            let transaction=connection.transaction()?;
            let changed=transaction.execute("UPDATE sync_runs SET processed=?1,page_token=?2 WHERE account_id=?3 AND id=?4 AND state='running' AND processed<=?1",
                params![processed,page_token,account,id])?;
            if changed!=1 {return Err(StoreError::InvalidInput);}
            super::changes::record(&transaction,Some(&account),"sync_run",Some(&id))?;
            transaction.commit()?;
            Ok(())
        }).await
    }
    pub async fn finish_sync_run_error(
        &self,
        account: String,
        id: String,
        state: String,
        code: String,
        now: i64,
    ) -> Result<(), StoreError> {
        if !matches!(state.as_str(), "failed" | "cancelled")
            || code.is_empty()
            || code.len() > 64
            || !code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|connection|{
            let transaction=connection.transaction()?;
            let changed=transaction.execute("UPDATE sync_runs SET state=?1,error_code=?2,finished_at_ms=?3,page_token=NULL WHERE account_id=?4 AND id=?5 AND state IN ('queued','running')",
                params![state,code,now,account,id])?;
            if changed!=1 {return Err(StoreError::InvalidInput);}
            super::schedules::failed(&transaction,&account,&id,now)?;
            super::changes::record(&transaction,Some(&account),"sync_run",Some(&id))?;
            transaction.commit()?;
            Ok(())
        }).await
    }
}
pub(super) fn get(connection: &Connection, account: &str, id: &str) -> Result<SyncRun, StoreError> {
    connection.query_row("SELECT id,account_id,scope,mode,state,started_at_ms,finished_at_ms,processed,error_code FROM sync_runs WHERE account_id=?1 AND id=?2",
        params![account,id],|row|{
            let processed:i64=row.get(7)?;
            let processed=u64::try_from(processed).map_err(|_|rusqlite::Error::IntegralValueOutOfRange(7,processed))?;
            Ok(SyncRun{id:row.get(0)?,account_id:row.get(1)?,scope:row.get(2)?,mode:row.get(3)?,state:row.get(4)?,
                started_at_ms:row.get(5)?,finished_at_ms:row.get(6)?,processed,error_code:row.get(8)?})
        }).optional()?.ok_or(StoreError::NotFound)
}
