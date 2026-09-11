use super::{
    calendar::{calendar_row, coverage, stored_event},
    CalendarCoverage, Store, StoreError, StoredCalendar, StoredEvent,
};
use crate::domain::calendar::AgendaWindow;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Serialize)]
pub struct CalendarCatalogPage {
    pub revision: u64,
    pub items: Vec<StoredCalendar>,
    pub next_page_token: Option<String>,
}
#[derive(Serialize)]
pub struct CalendarAgendaPage {
    pub revision: u64,
    pub items: Vec<StoredEvent>,
    pub coverage: Vec<CalendarCoverage>,
    pub next_page_token: Option<String>,
}
#[derive(Serialize)]
pub struct CalendarEventResult {
    pub revision: u64,
    pub event: StoredEvent,
    pub coverage: CalendarCoverage,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    fingerprint: String,
    revision: u64,
    time: i64,
    calendar: String,
    provider: String,
}
fn fingerprint(
    account: &str,
    kind: &str,
    window: Option<&AgendaWindow>,
    size: u32,
) -> Result<String, StoreError> {
    Ok(hex::encode(Sha256::digest(
        serde_json::to_vec(&(account, kind, window, size)).map_err(|_| StoreError::InvalidInput)?,
    )))
}
fn decode(token: Option<String>, expected: &str) -> Result<Option<Cursor>, StoreError> {
    token
        .map(|t| {
            if t.len() > 4096 {
                return Err(StoreError::InvalidInput);
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(t)
                .map_err(|_| StoreError::InvalidInput)?;
            let cursor: Cursor =
                serde_json::from_slice(&bytes).map_err(|_| StoreError::InvalidInput)?;
            if cursor.fingerprint != expected
                || cursor.calendar.len() > 128
                || cursor.provider.len() > 2048
            {
                return Err(StoreError::InvalidInput);
            }
            Ok(cursor)
        })
        .transpose()
}
fn encode(cursor: Cursor) -> Result<String, StoreError> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).map_err(|_| StoreError::InvalidInput)?))
}
fn revision(c: &Connection, account: &str, cursor: Option<&Cursor>) -> Result<u64, StoreError> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1)",
        [account],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(StoreError::NotFound);
    }
    let revision: i64 = c.query_row(
        "SELECT revision FROM store_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let revision = u64::try_from(revision).map_err(|_| StoreError::KeyOrCorrupt)?;
    if cursor.is_some_and(|p| p.revision != revision) {
        return Err(StoreError::RefreshRequired);
    }
    Ok(revision)
}
impl Store {
    pub async fn calendar_catalog_page(
        &self,
        account: String,
        size: u32,
        token: Option<String>,
    ) -> Result<CalendarCatalogPage, StoreError> {
        if !(1..=100).contains(&size) {
            return Err(StoreError::InvalidInput);
        }
        let fingerprint = fingerprint(&account, "calendars", None, size)?;
        let cursor = decode(token, &fingerprint)?;
        self.execute(move|c|{
            let revision=revision(c,&account,cursor.as_ref())?;
            let mut stmt=c.prepare("SELECT id,account_id,provider_id,summary,time_zone,access_role,is_primary,retired,canonical_revision FROM calendars WHERE account_id=?1 AND id>?2 ORDER BY id LIMIT ?3")?;
            let mut items=stmt.query_map(params![account,cursor.map(|p|p.calendar).unwrap_or_default(),size+1],calendar_row)?.collect::<Result<Vec<_>,_>>()?;
            if items.iter().map(|c|c.summary.as_ref().map_or(0,String::len)+c.provider_id.len()).sum::<usize>()>8*1024*1024{return Err(StoreError::ResultTooLarge)}
            let more=items.len()>size as usize;items.truncate(size as usize);
            let next_page_token=if more {Some(encode(Cursor{fingerprint,revision,time:0,calendar:items.last().ok_or(StoreError::KeyOrCorrupt)?.id.clone(),provider:String::new()})?)}else{None};
            Ok(CalendarCatalogPage{revision,items,next_page_token})
        }).await
    }
    pub async fn calendar_event(
        &self,
        account: String,
        calendar: String,
        id: String,
    ) -> Result<CalendarEventResult, StoreError> {
        self.execute(move|c|{
            let revision=revision(c,&account,None)?;
            let json:String=c.query_row("SELECT coalesce(c.object_json,o.object_json) FROM calendar_event_ids i LEFT JOIN calendar_objects c USING(account_id,calendar_id,provider_id) LEFT JOIN calendar_occurrences o USING(account_id,calendar_id,provider_id) WHERE i.account_id=?1 AND i.calendar_id=?2 AND i.id=?3 AND (c.object_json IS NOT NULL OR o.object_json IS NOT NULL)",params![account,calendar,id],|r|r.get(0)).optional()?.ok_or(StoreError::NotFound)?;
            let coverage=coverage(c,&account,&calendar,None)?;
            Ok(CalendarEventResult{revision,event:stored_event(c,account,calendar,id,&json)?,coverage})
        }).await
    }
    pub async fn calendar_agenda(
        &self,
        account: String,
        window: AgendaWindow,
        size: u32,
        token: Option<String>,
    ) -> Result<CalendarAgendaPage, StoreError> {
        AgendaWindow::new(&window.from, &window.to).map_err(|_| StoreError::InvalidInput)?;
        if !(1..=100).contains(&size) {
            return Err(StoreError::InvalidInput);
        }
        let fingerprint = fingerprint(&account, "agenda", Some(&window), size)?;
        let cursor = decode(token, &fingerprint)?;
        self.execute(move|c|{
            let revision=revision(c,&account,cursor.as_ref())?;
            let mut stmt=c.prepare("SELECT id,account_id,provider_id,summary,time_zone,access_role,is_primary,retired,canonical_revision FROM calendars WHERE account_id=?1 ORDER BY id LIMIT 1001")?;
            let calendars=stmt.query_map([&account],calendar_row)?.collect::<Result<Vec<_>,_>>()?;
            if calendars.len()>1000{return Err(StoreError::ResultTooLarge)}
            let mut coverages=Vec::new();let mut values:Vec<Value>=Vec::new();let mut placeholders=Vec::new();
            for cal in calendars {
                let mut status=coverage(c,&account,&cal.id,Some(&window))?;
                if !cal.retired {
                    if let Some(zone)=&cal.time_zone {
                        let (from,to)=window.bounds_ms(zone).map_err(|_|StoreError::KeyOrCorrupt)?;
                        placeholders.push("(?,?,?)");values.extend([Value::Text(cal.id),Value::Integer(from),Value::Integer(to)]);
                    }else{status.state="unavailable".into();}
                }
                coverages.push(status);
            }
            if values.is_empty(){return Ok(CalendarAgendaPage{revision,items:vec![],coverage:coverages,next_page_token:None})}
            let after=cursor.unwrap_or(Cursor{fingerprint:String::new(),revision,time:i64::MIN,calendar:String::new(),provider:String::new()});
            values.extend([Value::Text(account.clone()),Value::Integer(after.time),Value::Text(after.calendar),Value::Text(after.provider),Value::Integer(i64::from(size)+1)]);
            // Only generated placeholders enter SQL. Every account, calendar,
            // window boundary and cursor value is a bound parameter.
            let sql=format!("WITH bounds(calendar_id,lower_ms,upper_ms) AS (VALUES {}) SELECT i.id,o.calendar_id,o.object_json FROM calendar_occurrences o JOIN bounds b ON b.calendar_id=o.calendar_id JOIN calendar_event_ids i ON i.account_id=o.account_id AND i.calendar_id=o.calendar_id AND i.provider_id=o.provider_id WHERE o.account_id=? AND o.start_ms<b.upper_ms AND coalesce(o.end_ms,o.start_ms+1)>b.lower_ms AND (coalesce(o.start_ms,0),o.calendar_id,o.provider_id)>(?,?,?) ORDER BY coalesce(o.start_ms,0),o.calendar_id,o.provider_id LIMIT ?",placeholders.join(","));
            let mut stmt=c.prepare(&sql)?;let mut rows=stmt.query(params_from_iter(values))?;
            let mut items=Vec::new();let mut bytes=0_usize;
            while let Some(row)=rows.next()? {
                let json:String=row.get(2)?;bytes+=json.len();if bytes>8*1024*1024{return Err(StoreError::ResultTooLarge)}
                items.push(stored_event(c,account.clone(),row.get(1)?,row.get(0)?,&json)?);
            }
            let more=items.len()>size as usize;items.truncate(size as usize);
            let next_page_token=if more {let last=items.last().ok_or(StoreError::KeyOrCorrupt)?;Some(encode(Cursor{fingerprint,revision,time:last.object.start_ms.unwrap_or(0),calendar:last.calendar_id.clone(),provider:last.object.provider_id.clone()})?)}else{None};
            Ok(CalendarAgendaPage{revision,items,coverage:coverages,next_page_token})
        }).await
    }
}
