use super::{calendar, CalendarCheckpoint, Store, StoreError, StoredCalendar};
use crate::domain::calendar::{AgendaWindow, CalendarObject};
use rusqlite::{params, OptionalExtension};
use std::collections::BTreeSet;
#[derive(Clone)]
pub struct CalendarRepairCheckpoint {
    pub calendar_id: String,
    pub cursor: String,
    pub time_zone: String,
}
impl Store {
    pub async fn calendar_repair_targets(
        &self,
        account: String,
        run: String,
    ) -> Result<Vec<StoredCalendar>, StoreError> {
        self.execute(move|c|{
            let tx=c.transaction()?;calendar::running(&tx,&account,&run)?;
            let providers={let mut q=tx.prepare("SELECT provider_id FROM staged_calendar_catalog WHERE account_id=?1 AND run_id=?2 ORDER BY provider_id LIMIT 1001")?;
                let rows=q.query_map(params![account,run],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;rows};
            if providers.len()>1000 {return Err(StoreError::ResultTooLarge);}
            for provider in providers {
                let old:Option<String>=tx.query_row("SELECT id FROM calendars WHERE account_id=?1 AND provider_id=?2",params![account,provider],|r|r.get(0)).optional()?;
                tx.execute("INSERT OR IGNORE INTO staged_calendar_repair_ids(account_id,run_id,provider_id,calendar_id) VALUES(?1,?2,?3,?4)",params![account,run,provider,old.unwrap_or_else(||uuid::Uuid::new_v4().to_string())])?;
            }
            let rows={let mut q=tx.prepare("SELECT r.calendar_id,s.account_id,s.provider_id,s.summary,s.time_zone,s.access_role,s.is_primary,0,coalesce(c.canonical_revision,0) FROM staged_calendar_catalog s JOIN staged_calendar_repair_ids r USING(account_id,run_id,provider_id) LEFT JOIN calendars c ON c.account_id=s.account_id AND c.id=r.calendar_id WHERE s.account_id=?1 AND s.run_id=?2 ORDER BY s.provider_id")?;
                let rows=q.query_map(params![account,run],calendar::calendar_row)?.collect::<Result<Vec<_>,_>>()?;rows};
            tx.commit()?;Ok(rows)
        }).await
    }
    pub async fn stage_calendar_repair_event(
        &self,
        account: String,
        run: String,
        calendar: String,
        expanded: bool,
        event: CalendarObject,
    ) -> Result<(), StoreError> {
        let object = serde_json::to_string(&event).map_err(|_| StoreError::InvalidInput)?;
        if object.len() > 4 * 1024 * 1024 {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            calendar::running(c,&account,&run)?;
            c.execute("INSERT INTO staged_calendar_repair_events(account_id,run_id,calendar_id,kind,provider_id,object_json,status,etag,start_ms,end_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(account_id,run_id,calendar_id,kind,provider_id) DO UPDATE SET object_json=excluded.object_json,status=excluded.status,etag=excluded.etag,start_ms=excluded.start_ms,end_ms=excluded.end_ms",params![account,run,calendar,if expanded{"occurrence"}else{"canonical"},event.provider_id,object,event.status,event.etag,event.start_ms,event.end_ms])?;
            Ok(())
        }).await
    }
    pub async fn promote_calendar_repair(
        &self,
        account: String,
        run: String,
        window: AgendaWindow,
        checkpoints: Vec<CalendarRepairCheckpoint>,
        now: i64,
    ) -> Result<(), StoreError> {
        AgendaWindow::new(&window.from, &window.to).map_err(|_| StoreError::InvalidInput)?;
        if now < 0
            || checkpoints.len() > 1000
            || checkpoints.iter().any(|p| {
                p.cursor.is_empty()
                    || p.cursor.len() > 8192
                    || p.time_zone.parse::<chrono_tz::Tz>().is_err()
            })
        {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move|c|{
            let tx=c.transaction()?;calendar::running(&tx,&account,&run)?;
            let expected={let mut q=tx.prepare("SELECT r.calendar_id FROM staged_calendar_catalog s LEFT JOIN staged_calendar_repair_ids r USING(account_id,run_id,provider_id) WHERE s.account_id=?1 AND s.run_id=?2 AND s.access_role NOT IN ('none','freeBusyReader') LIMIT 1001")?;
                let rows=q.query_map(params![account,run],|r|r.get::<_,Option<String>>(0))?.collect::<Result<Vec<_>,_>>()?;rows};
            if expected.len()!=checkpoints.len() || expected.iter().any(Option::is_none) {return Err(StoreError::InvalidInput);}
            let expected=expected.into_iter().flatten().collect::<BTreeSet<_>>();
            let actual=checkpoints.iter().map(|p|p.calendar_id.clone()).collect::<BTreeSet<_>>();
            if expected!=actual || actual.len()!=checkpoints.len() {return Err(StoreError::InvalidInput);}
            calendar::publish_catalog(&tx,&account,&run,true)?;
            tx.execute("DELETE FROM staged_calendar_events WHERE account_id=?1 AND run_id=?2",params![account,run])?;
            tx.execute("INSERT INTO staged_calendar_events SELECT * FROM staged_calendar_repair_events WHERE account_id=?1 AND run_id=?2",params![account,run])?;
            for p in checkpoints {
                calendar::publish_objects(&tx,&account,&run,&p.calendar_id,&CalendarCheckpoint{cursor:p.cursor,full:true,time_zone:p.time_zone},now)?;
                let revision:i64=tx.query_row("SELECT canonical_revision FROM calendars WHERE account_id=?1 AND id=?2",params![account,p.calendar_id],|r|r.get(0))?;
                calendar::publish_occurrences(&tx,&account,&run,&p.calendar_id,&window,revision,now)?;
            }
            tx.execute("DELETE FROM staged_calendar_catalog WHERE account_id=?1 AND run_id=?2",params![account,run])?;
            tx.execute("UPDATE sync_runs SET state='succeeded',finished_at_ms=?3,page_token=NULL WHERE account_id=?1 AND id=?2",params![account,run,now])?;
            super::changes::record(&tx,Some(&account),"projection_repair",Some(&run))?;
            super::schedules::success(&tx,&account,&run,now)?;
            tx.commit()?;Ok(())
        }).await
    }
}
