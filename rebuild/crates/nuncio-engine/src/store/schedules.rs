use super::{changes, sync, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct SyncSchedule {
    pub account_id: String,
    pub scope: String,
    pub account_state: String,
    pub last_run_id: Option<String>,
    pub run_state: Option<String>,
    pub processed: u64,
    pub last_success_at_ms: Option<i64>,
    pub next_attempt_at_ms: i64,
    pub provider_retry_after_ms: Option<i64>,
    pub consecutive_failures: u32,
    pub error_code: Option<String>,
}
pub(super) fn ensure(c: &Connection, account: &str) -> Result<(), StoreError> {
    let provider: String = c
        .query_row(
            "SELECT provider FROM accounts WHERE id=?1",
            [account],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    let scopes: &[&str] = if provider == "google" {
        &["gmail", "calendar"]
    } else {
        &["imap"]
    };
    for scope in scopes {
        c.execute(
            "INSERT OR IGNORE INTO sync_schedules(account_id,scope) VALUES (?1,?2)",
            params![account, scope],
        )?;
    }
    Ok(())
}
pub(super) fn started(c: &Connection, account: &str, id: &str) -> Result<(), StoreError> {
    let run = sync::get(c, account, id)?;
    ensure(c, account)?;
    if run.mode != "fetch" {
        c.execute(
            "UPDATE sync_schedules SET last_run_id=?3 WHERE account_id=?1 AND scope=?2",
            params![account, run.scope, id],
        )?;
    }
    Ok(())
}
pub(super) fn success(c: &Connection, account: &str, id: &str, now: i64) -> Result<(), StoreError> {
    let run = sync::get(c, account, id)?;
    if run.mode == "fetch" {
        return Ok(());
    }
    let interval: i64 = c.query_row(
        "SELECT poll_interval_ms FROM sync_settings WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    c.execute("UPDATE sync_schedules SET last_success_at_ms=?4,next_attempt_at_ms=?5,provider_retry_after_ms=NULL,consecutive_failures=0,error_code=NULL WHERE account_id=?1 AND scope=?2 AND last_run_id=?3",params![account,run.scope,id,now,now.saturating_add(interval)])?;
    Ok(())
}
pub(super) fn failed(c: &Connection, account: &str, id: &str, now: i64) -> Result<(), StoreError> {
    let run = sync::get(c, account, id)?;
    if run.mode == "fetch" {
        return Ok(());
    }
    let (previous,retry):(u32,Option<i64>)=c.query_row("SELECT consecutive_failures,provider_retry_after_ms FROM sync_schedules WHERE account_id=?1 AND scope=?2",params![account,run.scope],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let cancelled = run.state == "cancelled";
    let delay = if cancelled {
        c.query_row(
            "SELECT poll_interval_ms FROM sync_settings WHERE singleton=1",
            [],
            |r| r.get::<_, i64>(0),
        )?
    } else {
        let base = (1000_u64 << previous.min(8)).min(240_000);
        let jitter = (uuid::Uuid::new_v4().as_u128() % u128::from(base / 4 + 1)) as i64;
        i64::try_from(base).map_err(|_| StoreError::InvalidInput)? + jitter
    };
    let next = now.saturating_add(delay).max(retry.unwrap_or(0));
    c.execute("UPDATE sync_schedules SET next_attempt_at_ms=?4,consecutive_failures=?5,error_code=?6 WHERE account_id=?1 AND scope=?2 AND last_run_id=?3",params![account,run.scope,id,next,if cancelled{previous}else{previous.saturating_add(1).min(1024)},run.error_code])?;
    Ok(())
}
impl Store {
    pub async fn configure_poll_interval(&self, interval: u64) -> Result<(), StoreError> {
        if !(10..=86_400_000).contains(&interval) {
            return Err(StoreError::InvalidInput);
        }
        let interval = i64::try_from(interval).map_err(|_| StoreError::InvalidInput)?;
        self.execute(move|c|{
            let tx=c.transaction()?;
            let changed=tx.execute("UPDATE sync_settings SET poll_interval_ms=?1 WHERE singleton=1 AND poll_interval_ms<>?1",[interval])?;
            if changed!=0{
                tx.execute("UPDATE sync_schedules SET next_attempt_at_ms=max(last_success_at_ms+?1,coalesce(provider_retry_after_ms,0)) WHERE last_success_at_ms IS NOT NULL AND error_code IS NULL",[interval])?;
                changes::record(&tx,None,"sync_configuration",None)?;
            }
            tx.commit()?;Ok(())
        }).await
    }
    pub async fn sync_schedules(&self) -> Result<Vec<SyncSchedule>, StoreError> {
        self.execute(|c| list(c)).await
    }
    pub async fn status_with_schedules(
        &self,
    ) -> Result<(super::StoreStatus, Vec<SyncSchedule>), StoreError> {
        self.execute(|c| Ok((super::worker::status(c)?, list(c)?)))
            .await
    }
    pub async fn provider_retry_after(
        &self,
        account: String,
        scope: String,
        deadline: i64,
    ) -> Result<(), StoreError> {
        if deadline < 0 || !matches!(scope.as_str(), "gmail" | "calendar" | "imap") {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;ensure(&tx,&account)?;
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_schedules WHERE account_id=?1 AND scope=?2)",params![account,scope],|r|r.get(0))?;
            if !exists{return Err(StoreError::NotFound);}
            if tx.execute("UPDATE sync_schedules SET provider_retry_after_ms=?3,next_attempt_at_ms=max(next_attempt_at_ms,?3) WHERE account_id=?1 AND scope=?2 AND coalesce(provider_retry_after_ms,0)<?3",params![account,scope,deadline])?!=0 {changes::record(&tx,Some(&account),"sync_schedule",Some(&scope))?;}
            tx.commit()?;Ok(())
        }).await
    }
    pub async fn provider_retry_deadline(
        &self,
        account: String,
        scope: String,
    ) -> Result<Option<i64>, StoreError> {
        self.execute(move|c|c.query_row("SELECT provider_retry_after_ms FROM sync_schedules WHERE account_id=?1 AND scope=?2",params![account,scope],|r|r.get(0)).optional()?.ok_or(StoreError::NotFound)).await
    }
}
fn list(c: &Connection) -> Result<Vec<SyncSchedule>, StoreError> {
    let mut stmt=c.prepare("SELECT s.account_id,s.scope,CASE WHEN a.archived=1 THEN 'archived' WHEN a.paused=1 THEN 'paused' ELSE a.state END,s.last_run_id,r.state,coalesce(r.processed,0),s.last_success_at_ms,s.next_attempt_at_ms,s.provider_retry_after_ms,s.consecutive_failures,s.error_code FROM sync_schedules s JOIN accounts a ON a.id=s.account_id LEFT JOIN sync_runs r ON r.account_id=s.account_id AND r.id=s.last_run_id ORDER BY s.account_id,s.scope LIMIT 301")?;
    let rows = stmt
        .query_map([], |r| {
            let processed: i64 = r.get(5)?;
            Ok(SyncSchedule {
                account_id: r.get(0)?,
                scope: r.get(1)?,
                account_state: r.get(2)?,
                last_run_id: r.get(3)?,
                run_state: r.get(4)?,
                processed: u64::try_from(processed)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(5, processed))?,
                last_success_at_ms: r.get(6)?,
                next_attempt_at_ms: r.get(7)?,
                provider_retry_after_ms: r.get(8)?,
                consecutive_failures: r.get(9)?,
                error_code: r.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > 300 {
        return Err(StoreError::ResultTooLarge);
    }
    Ok(rows)
}
