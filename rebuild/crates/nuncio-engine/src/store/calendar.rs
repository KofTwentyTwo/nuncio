use super::{sync, Store, StoreError};
use crate::domain::calendar::{AgendaWindow, CalendarObject};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub const CANONICAL_QUERY: &str =
    "google-calendar:singleEvents=false:showDeleted=true:maxResults=100:v1";
pub struct CalendarCheckpoint {
    pub cursor: String,
    pub full: bool,
    pub time_zone: String,
}
pub struct CalendarCatalogEntry {
    pub provider_id: String,
    pub summary: Option<String>,
    pub time_zone: Option<String>,
    pub access_role: String,
    pub is_primary: bool,
    pub provider_json: String,
}
#[derive(Clone, Serialize)]
pub struct StoredCalendar {
    pub id: String,
    pub account_id: String,
    pub provider_id: String,
    pub summary: Option<String>,
    pub time_zone: Option<String>,
    pub access_role: String,
    pub is_primary: bool,
    pub retired: bool,
    pub canonical_revision: u64,
}
#[derive(Clone, Serialize)]
pub struct StoredEvent {
    pub id: String,
    pub account_id: String,
    pub calendar_id: String,
    pub recurring_event_id: Option<String>,
    #[serde(flatten)]
    pub object: CalendarObject,
}
#[derive(Clone, Serialize)]
pub struct CalendarCoverage {
    pub calendar_id: String,
    pub state: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub canonical_revision: u64,
    pub cached_revision: Option<u64>,
    pub refreshed_at_ms: Option<i64>,
}
pub(super) fn running(c: &Connection, account: &str, run: &str) -> Result<(), StoreError> {
    let run = sync::get(c, account, run)?;
    if run.state != "running" || run.scope != "calendar" {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}
pub(super) fn scope(calendar: &str) -> String {
    format!("google-calendar:{calendar}")
}
impl Store {
    pub async fn calendar_cursor(
        &self,
        account: String,
        calendar: String,
    ) -> Result<Option<String>, StoreError> {
        self.execute(move|c|Ok(c.query_row("SELECT cursor FROM sync_scopes WHERE account_id=?1 AND scope=?2 AND cursor_kind='google_calendar_events' AND query_fingerprint=?3",params![account,scope(&calendar),CANONICAL_QUERY],|r|r.get(0)).optional()?)).await
    }
    pub async fn clear_calendar_staging(
        &self,
        account: String,
        run: String,
        calendar: String,
        expanded: bool,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{running(c,&account,&run)?;c.execute("DELETE FROM staged_calendar_events WHERE account_id=?1 AND run_id=?2 AND calendar_id=?3 AND kind=?4",params![account,run,calendar,if expanded{"occurrence"}else{"canonical"}])?;Ok(())}).await
    }
    pub async fn finish_calendar_run(
        &self,
        account: String,
        run: String,
        now: i64,
    ) -> Result<(), StoreError> {
        self.execute(move|c|{let tx=c.transaction()?;running(&tx,&account,&run)?;tx.execute("UPDATE sync_runs SET state='succeeded',finished_at_ms=?3,page_token=NULL WHERE account_id=?1 AND id=?2",params![account,run,now])?;super::changes::record(&tx,Some(&account),"sync_run",Some(&run))?;super::schedules::success(&tx,&account,&run,now)?;tx.commit()?;Ok(())}).await
    }
    pub async fn stage_calendar_entry(
        &self,
        account: String,
        run: String,
        entry: CalendarCatalogEntry,
    ) -> Result<(), StoreError> {
        if entry.provider_id.is_empty()
            || entry.provider_id.len() > 2048
            || entry.provider_json.len() > 1024 * 1024
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            running(c,&account,&run)?;
            c.execute("INSERT INTO staged_calendar_catalog(account_id,run_id,provider_id,summary,time_zone,access_role,is_primary,provider_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(account_id,run_id,provider_id) DO UPDATE SET summary=excluded.summary,time_zone=excluded.time_zone,access_role=excluded.access_role,is_primary=excluded.is_primary,provider_json=excluded.provider_json",params![account,run,entry.provider_id,entry.summary,entry.time_zone,entry.access_role,entry.is_primary,entry.provider_json])?;Ok(())
        }).await
    }
    pub async fn promote_calendar_catalog(
        &self,
        account: String,
        run: String,
    ) -> Result<(), StoreError> {
        self.execute(move |c| {
            let tx = c.transaction()?;
            running(&tx, &account, &run)?;
            publish_catalog(&tx, &account, &run, false)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn calendars(&self, account: String) -> Result<Vec<StoredCalendar>, StoreError> {
        self.execute(move|c|{
            let mut stmt=c.prepare("SELECT id,account_id,provider_id,summary,time_zone,access_role,is_primary,retired,canonical_revision FROM calendars WHERE account_id=?1 ORDER BY id LIMIT 1001")?;
            let rows=stmt.query_map([account],calendar_row)?.collect::<Result<Vec<_>,_>>()?;
            if rows.len()>1000{return Err(StoreError::InvalidInput)}Ok(rows)
        }).await
    }
    pub async fn stage_calendar_event(
        &self,
        account: String,
        run: String,
        calendar: String,
        expanded: bool,
        event: CalendarObject,
    ) -> Result<(), StoreError> {
        let json = serde_json::to_string(&event).map_err(|_| StoreError::InvalidInput)?;
        if json.len() > 4 * 1024 * 1024 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            running(c,&account,&run)?;
            c.execute("INSERT INTO staged_calendar_events(account_id,run_id,calendar_id,kind,provider_id,object_json,status,etag,start_ms,end_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(account_id,run_id,calendar_id,kind,provider_id) DO UPDATE SET object_json=excluded.object_json,status=excluded.status,etag=excluded.etag,start_ms=excluded.start_ms,end_ms=excluded.end_ms",params![account,run,calendar,if expanded{"occurrence"}else{"canonical"},event.provider_id,json,event.status,event.etag,event.start_ms,event.end_ms])?;Ok(())
        }).await
    }
    pub async fn promote_calendar_objects(
        &self,
        account: String,
        run: String,
        calendar: String,
        checkpoint: CalendarCheckpoint,
        now: i64,
    ) -> Result<(), StoreError> {
        let CalendarCheckpoint {
            cursor,
            full,
            time_zone,
        } = checkpoint;
        if cursor.is_empty() || cursor.len() > 8192 || time_zone.parse::<chrono_tz::Tz>().is_err() {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move |c| {
            let tx = c.transaction()?;
            running(&tx, &account, &run)?;
            publish_objects(
                &tx,
                &account,
                &run,
                &calendar,
                &CalendarCheckpoint {
                    cursor,
                    full,
                    time_zone,
                },
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn promote_calendar_occurrences(
        &self,
        account: String,
        run: String,
        calendar: String,
        window: AgendaWindow,
        canonical_revision: u64,
        now: i64,
    ) -> Result<(), StoreError> {
        AgendaWindow::new(&window.from, &window.to).map_err(|_| StoreError::InvalidInput)?;
        let canonical_revision =
            i64::try_from(canonical_revision).map_err(|_| StoreError::InvalidInput)?;
        self.execute(move |c| {
            let tx = c.transaction()?;
            running(&tx, &account, &run)?;
            publish_occurrences(
                &tx,
                &account,
                &run,
                &calendar,
                &window,
                canonical_revision,
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn calendar_event_by_provider(
        &self,
        account: String,
        calendar: String,
        provider: String,
    ) -> Result<StoredEvent, StoreError> {
        self.execute(move|c|{
            let (id,json):(String,String)=c.query_row("SELECT i.id,coalesce(c.object_json,o.object_json) FROM calendar_event_ids i LEFT JOIN calendar_objects c USING(account_id,calendar_id,provider_id) LEFT JOIN calendar_occurrences o USING(account_id,calendar_id,provider_id) WHERE i.account_id=?1 AND i.calendar_id=?2 AND i.provider_id=?3 AND (c.object_json IS NOT NULL OR o.object_json IS NOT NULL)",params![account,calendar,provider],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(StoreError::NotFound)?;
            stored_event(c,account,calendar,id,&json)
        }).await
    }
    pub async fn calendar_coverage(
        &self,
        account: String,
        calendar: String,
        window: AgendaWindow,
    ) -> Result<CalendarCoverage, StoreError> {
        AgendaWindow::new(&window.from, &window.to).map_err(|_| StoreError::InvalidInput)?;
        self.execute(move |c| coverage(c, &account, &calendar, Some(&window)))
            .await
    }
}
pub(super) fn stored_event(
    c: &Connection,
    account: String,
    calendar: String,
    id: String,
    json: &str,
) -> Result<StoredEvent, StoreError> {
    let object: CalendarObject =
        serde_json::from_str(json).map_err(|_| StoreError::KeyOrCorrupt)?;
    let recurring_event_id=object.recurring_provider_id.as_ref().map(|provider|{
        c.query_row("SELECT id FROM calendar_event_ids WHERE account_id=?1 AND calendar_id=?2 AND provider_id=?3",params![account,calendar,provider],|r|r.get::<_,String>(0)).optional()
    }).transpose()?.flatten();
    Ok(StoredEvent {
        id,
        account_id: account,
        calendar_id: calendar,
        recurring_event_id,
        object,
    })
}
pub(super) fn calendar_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCalendar> {
    let revision: i64 = r.get(8)?;
    let canonical_revision = u64::try_from(revision)
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(8, revision))?;
    Ok(StoredCalendar {
        id: r.get(0)?,
        account_id: r.get(1)?,
        provider_id: r.get(2)?,
        summary: r.get(3)?,
        time_zone: r.get(4)?,
        access_role: r.get(5)?,
        is_primary: r.get(6)?,
        retired: r.get(7)?,
        canonical_revision,
    })
}
fn changed(c: &Connection, account: &str) -> Result<(), StoreError> {
    super::changes::record(c, Some(account), "calendar", None)
}
fn promote_events(
    c: &Connection,
    account: &str,
    run: &str,
    calendar: &str,
    expanded: bool,
    full: bool,
) -> Result<(), StoreError> {
    let kind = if expanded { "occurrence" } else { "canonical" };
    // Table names come exclusively from this boolean, never from provider input.
    let table = if expanded {
        "calendar_occurrences"
    } else {
        "calendar_objects"
    };
    let mut stmt=c.prepare("SELECT provider_id FROM staged_calendar_events WHERE account_id=?1 AND run_id=?2 AND calendar_id=?3 AND kind=?4")?;
    let mut rows = stmt.query(params![account, run, calendar, kind])?;
    while let Some(row) = rows.next()? {
        let provider: String = row.get(0)?;
        c.execute("INSERT OR IGNORE INTO calendar_event_ids(account_id,calendar_id,provider_id,id) VALUES (?1,?2,?3,?4)",params![account,calendar,provider,uuid::Uuid::new_v4().to_string()])?;
    }
    drop(rows);
    drop(stmt);
    c.execute(&format!("INSERT INTO {table}(account_id,calendar_id,provider_id,object_json,status,etag,start_ms,end_ms) SELECT account_id,calendar_id,provider_id,object_json,status,etag,start_ms,end_ms FROM staged_calendar_events WHERE account_id=?1 AND run_id=?2 AND calendar_id=?3 AND kind=?4 ON CONFLICT(account_id,calendar_id,provider_id) DO UPDATE SET object_json=excluded.object_json,status=excluded.status,etag=excluded.etag,start_ms=excluded.start_ms,end_ms=excluded.end_ms"),params![account,run,calendar,kind])?;
    if full {
        c.execute(&format!("DELETE FROM {table} WHERE account_id=?1 AND calendar_id=?3 AND provider_id NOT IN (SELECT provider_id FROM staged_calendar_events WHERE account_id=?1 AND run_id=?2 AND calendar_id=?3 AND kind=?4)"),params![account,run,calendar,kind])?;
    }
    c.execute("DELETE FROM staged_calendar_events WHERE account_id=?1 AND run_id=?2 AND calendar_id=?3 AND kind=?4",params![account,run,calendar,kind])?;
    Ok(())
}

pub(super) fn coverage(
    c: &Connection,
    account: &str,
    calendar: &str,
    window: Option<&AgendaWindow>,
) -> Result<CalendarCoverage, StoreError> {
    let (revision, retired, access_role): (i64, bool, String) = c
        .query_row(
            "SELECT canonical_revision,retired,access_role FROM calendars WHERE account_id=?1 AND id=?2",
            params![account, calendar],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    let readable = !matches!(access_role.as_str(), "none" | "freeBusyReader");
    let revision = u64::try_from(revision).map_err(|_| StoreError::KeyOrCorrupt)?;
    let cached=c.query_row("SELECT from_date,to_date,state,canonical_revision,refreshed_at_ms FROM agenda_coverage WHERE account_id=?1 AND calendar_id=?2",params![account,calendar],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?))).optional()?;
    let mut coverage = CalendarCoverage {
        calendar_id: calendar.to_owned(),
        state: if retired { "retired" } else { "unavailable" }.into(),
        from: None,
        to: None,
        canonical_revision: revision,
        cached_revision: None,
        refreshed_at_ms: None,
    };
    if let Some((from, to, state, cached, at)) = cached {
        let cached = u64::try_from(cached).map_err(|_| StoreError::KeyOrCorrupt)?;
        if !retired && readable {
            coverage.state = if window.is_some_and(|w| from > w.from || to < w.to) {
                "out_of_window"
            } else if state != "current" || cached != revision {
                "stale"
            } else {
                "current"
            }
            .into();
        }
        coverage.from = Some(from);
        coverage.to = Some(to);
        coverage.cached_revision = Some(cached);
        coverage.refreshed_at_ms = Some(at);
    }
    Ok(coverage)
}

pub(super) fn publish_catalog(
    c: &Connection,
    account: &str,
    run: &str,
    retain_staging: bool,
) -> Result<(), StoreError> {
    c.execute("UPDATE agenda_coverage SET state='stale' WHERE account_id=?1 AND calendar_id IN (SELECT c.id FROM calendars c JOIN staged_calendar_catalog s ON c.account_id=s.account_id AND c.provider_id=s.provider_id WHERE c.account_id=?1 AND s.run_id=?2 AND ((s.time_zone IS NOT NULL AND c.time_zone IS NOT s.time_zone) OR c.access_role IS NOT s.access_role))",params![account,run])?;
    // Calendar zones change all-day instants; permissions can change private
    // event details without changing event versions. Both require a full read.
    c.execute("DELETE FROM sync_scopes WHERE account_id=?1 AND scope IN (SELECT 'google-calendar:'||c.id FROM calendars c JOIN staged_calendar_catalog s ON c.account_id=s.account_id AND c.provider_id=s.provider_id WHERE c.account_id=?1 AND s.run_id=?2 AND ((s.time_zone IS NOT NULL AND c.time_zone IS NOT s.time_zone) OR c.access_role IS NOT s.access_role))",params![account,run])?;
    let mut stmt = c.prepare(
        "SELECT provider_id FROM staged_calendar_catalog WHERE account_id=?1 AND run_id=?2",
    )?;
    let mut rows = stmt.query(params![account, run])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        c.execute("INSERT INTO calendars(account_id,id,provider_id,summary,time_zone,access_role,is_primary,retired,provider_json) SELECT account_id,coalesce((SELECT calendar_id FROM staged_calendar_repair_ids r WHERE r.account_id=staged_calendar_catalog.account_id AND r.run_id=staged_calendar_catalog.run_id AND r.provider_id=staged_calendar_catalog.provider_id),?4),provider_id,summary,time_zone,access_role,is_primary,0,provider_json FROM staged_calendar_catalog WHERE account_id=?1 AND run_id=?2 AND provider_id=?3 ON CONFLICT(account_id,provider_id) DO UPDATE SET summary=excluded.summary,time_zone=coalesce(excluded.time_zone,calendars.time_zone),access_role=excluded.access_role,is_primary=excluded.is_primary,retired=0,provider_json=excluded.provider_json",params![account,run,id,uuid::Uuid::new_v4().to_string()])?;
    }
    drop(rows);
    drop(stmt);
    c.execute("UPDATE calendars SET retired=1 WHERE account_id=?1 AND provider_id NOT IN (SELECT provider_id FROM staged_calendar_catalog WHERE account_id=?1 AND run_id=?2)",params![account,run])?;
    c.execute("DELETE FROM sync_scopes WHERE account_id=?1 AND scope IN (SELECT 'google-calendar:'||id FROM calendars WHERE account_id=?1 AND retired=1)", [account])?;
    if !retain_staging {
        c.execute(
            "DELETE FROM staged_calendar_catalog WHERE account_id=?1 AND run_id=?2",
            params![account, run],
        )?;
    }
    changed(c, account)?;
    Ok(())
}

pub(super) fn publish_objects(
    c: &Connection,
    account: &str,
    run: &str,
    calendar: &str,
    checkpoint: &CalendarCheckpoint,
    now: i64,
) -> Result<(), StoreError> {
    let CalendarCheckpoint {
        cursor,
        full,
        time_zone,
    } = checkpoint;
    let full = *full;
    promote_events(c, account, run, calendar, false, full)?;
    changed(c, account)?;
    c.execute("UPDATE calendars SET time_zone=?3,canonical_revision=(SELECT revision FROM store_meta WHERE singleton=1) WHERE account_id=?1 AND id=?2",params![account,calendar,time_zone])?;
    c.execute(
        "UPDATE agenda_coverage SET state='stale' WHERE account_id=?1 AND calendar_id=?2",
        params![account, calendar],
    )?;
    c.execute("INSERT INTO sync_scopes(account_id,scope,cursor_kind,cursor,query_fingerprint,active_generation,synchronized_at_ms) VALUES (?1,?2,'google_calendar_events',?3,?4,?5,?6) ON CONFLICT(account_id,scope) DO UPDATE SET cursor=excluded.cursor,query_fingerprint=excluded.query_fingerprint,active_generation=excluded.active_generation,synchronized_at_ms=excluded.synchronized_at_ms",params![account,scope(calendar),cursor,CANONICAL_QUERY,run,now])?;
    Ok(())
}

pub(super) fn publish_occurrences(
    c: &Connection,
    account: &str,
    run: &str,
    calendar: &str,
    window: &AgendaWindow,
    canonical_revision: i64,
    now: i64,
) -> Result<(), StoreError> {
    let current: i64 = c
        .query_row(
            "SELECT canonical_revision FROM calendars WHERE account_id=?1 AND id=?2",
            params![account, calendar],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    if current != canonical_revision {
        return Err(StoreError::RefreshRequired);
    }
    promote_events(c, account, run, calendar, true, true)?;
    c.execute("INSERT INTO agenda_coverage(account_id,calendar_id,from_date,to_date,state,canonical_revision,refreshed_at_ms,generation) VALUES (?1,?2,?3,?4,'current',?5,?6,?7) ON CONFLICT(account_id,calendar_id) DO UPDATE SET from_date=excluded.from_date,to_date=excluded.to_date,state=excluded.state,canonical_revision=excluded.canonical_revision,refreshed_at_ms=excluded.refreshed_at_ms,generation=excluded.generation",params![account,calendar,window.from,window.to,canonical_revision,now,run])?;
    changed(c, account)?;
    Ok(())
}
