use super::{OperationAttempt, Store, StoreError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize)]
pub struct OperationSummary {
    pub id: String,
    pub account_id: String,
    pub request_id: String,
    pub kind: String,
    pub resource_id: String,
    pub state: String,
    pub version: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub next_attempt_at_ms: Option<i64>,
    pub needs_reconciliation: bool,
    pub error_code: Option<String>,
    pub disposition: Option<String>,
}
pub struct OperationPage {
    pub revision: u64,
    pub items: Vec<OperationSummary>,
    pub next_page_token: Option<String>,
}
pub struct OperationAttemptPage {
    pub revision: u64,
    pub items: Vec<OperationAttempt>,
    pub next_page_token: Option<String>,
}
pub struct ReadyOperation {
    pub kind: String,
    pub account_id: String,
    pub id: String,
    pub state: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    revision: u64,
    fingerprint: String,
    position: i64,
    id: String,
}
impl Store {
    pub async fn ready_operations(&self, now: i64) -> Result<Vec<ReadyOperation>, StoreError> {
        if now < 0 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let mut stmt=c.prepare("SELECT account_id,id,state,kind FROM (SELECT o.account_id,o.id,o.state,o.kind,row_number() OVER (PARTITION BY o.account_id ORDER BY o.created_at_ms,o.id) AS position FROM operations o JOIN accounts a ON a.id=o.account_id WHERE ((a.provider='google' AND o.kind IN ('send','mail_change','calendar_change')) OR (a.provider='imap' AND (o.kind='mail_change' OR (o.kind='send' AND EXISTS(SELECT 1 FROM smtp_submissions s WHERE s.account_id=o.account_id AND s.operation_id=o.id))))) AND a.state='connected' AND NOT EXISTS(SELECT 1 FROM mail_change_payloads current JOIN mail_change_payloads prior ON prior.account_id=current.account_id AND prior.provider_message_id=current.provider_message_id AND prior.sequence<current.sequence JOIN operations earlier ON earlier.account_id=prior.account_id AND earlier.id=prior.operation_id WHERE current.account_id=o.account_id AND current.operation_id=o.id AND earlier.state NOT IN ('applied','failed','cancelled') AND earlier.disposition IS NULL) AND NOT EXISTS(SELECT 1 FROM calendar_change_payloads current JOIN calendar_change_payloads prior ON prior.account_id=current.account_id AND prior.provider_calendar_id=current.provider_calendar_id AND prior.provider_event_id=current.provider_event_id AND prior.sequence<current.sequence JOIN operations earlier ON earlier.account_id=prior.account_id AND earlier.id=prior.operation_id WHERE current.account_id=o.account_id AND current.operation_id=o.id AND earlier.state NOT IN ('applied','failed','cancelled') AND earlier.disposition IS NULL) AND o.disposition IS NULL AND (o.state='queued' OR (o.state='retry_wait' AND o.next_attempt_at_ms<=?1) OR (o.state='uncertain' AND o.needs_reconciliation=1 AND (o.next_attempt_at_ms IS NULL OR o.next_attempt_at_ms<=?1)))) WHERE position=1 ORDER BY account_id LIMIT 100")?;
            let rows=stmt.query_map([now],|r|Ok(ReadyOperation{account_id:r.get(0)?,id:r.get(1)?,state:r.get(2)?,kind:r.get(3)?}))?;
            rows.collect::<Result<Vec<_>,_>>().map_err(StoreError::from)
        }).await
    }
    pub async fn query_operations(
        &self,
        account: String,
        page_size: u32,
        token: Option<String>,
    ) -> Result<OperationPage, StoreError> {
        let fingerprint = fingerprint("operations", &account, None, page_size)?;
        let cursor = cursor(token, &fingerprint)?;
        self.execute(move|c|{
            super::accounts::get(c,&account)?.ok_or(StoreError::NotFound)?;
            let revision=revision(c,cursor.as_ref())?;
            let mut stmt=c.prepare("SELECT id,request_id,kind,resource_id,state,version,created_at_ms,updated_at_ms,next_attempt_at_ms,needs_reconciliation,error_code,disposition FROM operations WHERE account_id=?1 AND (?2 IS NULL OR created_at_ms<?2 OR (created_at_ms=?2 AND id>?3)) ORDER BY created_at_ms DESC,id LIMIT ?4")?;
            let mut items=stmt.query_map(params![account,cursor.as_ref().map(|c|c.position),cursor.as_ref().map(|c|&c.id),page_size+1],|r|Ok(OperationSummary{id:r.get(0)?,account_id:account.clone(),request_id:r.get(1)?,kind:r.get(2)?,resource_id:r.get(3)?,state:r.get(4)?,version:u64::try_from(r.get::<_,i64>(5)?).map_err(|_|rusqlite::Error::InvalidQuery)?,created_at_ms:r.get(6)?,updated_at_ms:r.get(7)?,next_attempt_at_ms:r.get(8)?,needs_reconciliation:r.get(9)?,error_code:r.get(10)?,disposition:r.get(11)?}))?.collect::<Result<Vec<_>,_>>()?;
            let more=items.len()>page_size as usize;items.truncate(page_size as usize);
            let next_page_token=if more {let last=items.last().ok_or(StoreError::KeyOrCorrupt)?;Some(encode(Cursor{revision,fingerprint,position:last.created_at_ms,id:last.id.clone()})?)}else{None};
            Ok(OperationPage{revision,items,next_page_token})
        }).await
    }
    pub async fn query_operation_attempts(
        &self,
        account: String,
        id: String,
        page_size: u32,
        token: Option<String>,
    ) -> Result<OperationAttemptPage, StoreError> {
        let fingerprint = fingerprint("operation_attempts", &account, Some(&id), page_size)?;
        let cursor = cursor(token, &fingerprint)?;
        let before = cursor
            .as_ref()
            .map(|c| u32::try_from(c.position).map_err(|_| StoreError::InvalidInput))
            .transpose()?;
        if before.is_some_and(|n| !(1..=1000).contains(&n)) {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move |c| {
            super::operations::get(c, &account, &id)?;
            let revision = revision(c, cursor.as_ref())?;
            let mut items =
                super::operation_attempts::read_attempts(c, &account, &id, before, page_size + 1)?;
            let more = items.len() > page_size as usize;
            items.truncate(page_size as usize);
            let next_page_token = if more {
                let last = items.last().ok_or(StoreError::KeyOrCorrupt)?;
                Some(encode(Cursor {
                    revision,
                    fingerprint,
                    position: i64::from(last.ordinal),
                    id: String::new(),
                })?)
            } else {
                None
            };
            Ok(OperationAttemptPage {
                revision,
                items,
                next_page_token,
            })
        })
        .await
    }
}
fn fingerprint(
    kind: &str,
    account: &str,
    id: Option<&str>,
    size: u32,
) -> Result<String, StoreError> {
    if account.len() > 128 || id.is_some_and(|id| id.len() > 128) || !(1..=100).contains(&size) {
        return Err(StoreError::InvalidInput);
    }
    Ok(hex::encode(Sha256::digest(
        serde_json::to_vec(&(kind, account, id, size)).map_err(|_| StoreError::InvalidInput)?,
    )))
}
fn cursor(token: Option<String>, fingerprint: &str) -> Result<Option<Cursor>, StoreError> {
    let cursor: Option<Cursor> = token
        .map(|t| {
            if t.len() > 4096 {
                return Err(StoreError::InvalidInput);
            }
            serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(t)
                    .map_err(|_| StoreError::InvalidInput)?,
            )
            .map_err(|_| StoreError::InvalidInput)
        })
        .transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|c| c.fingerprint != fingerprint || c.position < 0 || c.id.len() > 128)
    {
        return Err(StoreError::InvalidInput);
    }
    Ok(cursor)
}
fn revision(c: &Connection, cursor: Option<&Cursor>) -> Result<u64, StoreError> {
    let revision: i64 = c.query_row(
        "SELECT revision FROM store_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let revision = u64::try_from(revision).map_err(|_| StoreError::KeyOrCorrupt)?;
    if cursor.is_some_and(|c| c.revision != revision) {
        return Err(StoreError::RefreshRequired);
    }
    Ok(revision)
}
fn encode(cursor: Cursor) -> Result<String, StoreError> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).map_err(|_| StoreError::KeyOrCorrupt)?))
}
