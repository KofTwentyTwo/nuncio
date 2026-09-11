use crate::{store::StoreError, sync_error::SyncError};
use chrono::{DateTime, FixedOffset};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct FreeBusyRequest {
    pub account_id: String,
    pub from: String,
    pub to: String,
    pub time_zone: String,
    // Provider calendar identifiers; arbitrary groups are not expanded.
    pub calendar_ids: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct FreeBusyResult {
    pub from: String,
    pub to: String,
    pub time_zone: String,
    pub checked_at_ms: i64,
    pub complete: bool,
    pub calendars: Vec<CalendarAvailability>,
}
#[derive(Debug, Serialize)]
pub struct CalendarAvailability {
    pub provider_calendar_id: String,
    pub complete: bool,
    pub busy: Vec<BusyPeriod>,
    pub errors: Vec<AvailabilityError>,
}
#[derive(Debug, Serialize)]
pub struct BusyPeriod {
    pub from: String,
    pub to: String,
}
#[derive(Debug, Serialize)]
pub struct AvailabilityError {
    pub domain: String,
    pub reason: String,
}

impl FreeBusyRequest {
    pub fn validate(&self) -> Result<(), StoreError> {
        let invalid = StoreError::InvalidInput;
        let lower = timestamp(&self.from).map_err(|_| StoreError::InvalidInput)?;
        let upper = timestamp(&self.to).map_err(|_| StoreError::InvalidInput)?;
        if lower >= upper
            || upper - lower > chrono::Duration::days(366)
            || self.time_zone.len() > 128
            || self.time_zone.parse::<chrono_tz::Tz>().is_err()
            || self.calendar_ids.is_empty()
            || self.calendar_ids.len() > 50
            || self.calendar_ids.iter().any(|id| {
                id.is_empty()
                    || id.len() > 2048
                    || id.chars().any(|c| c.is_control() || c.is_whitespace())
            })
            || self.calendar_ids.iter().collect::<BTreeSet<_>>().len() != self.calendar_ids.len()
        {
            return Err(invalid);
        }
        Ok(())
    }
    pub fn provider_body(&self) -> Result<Value, StoreError> {
        self.validate()?;
        Ok(
            json!({"timeMin":self.from,"timeMax":self.to,"timeZone":self.time_zone,"items":self.calendar_ids.iter().map(|id|json!({"id":id})).collect::<Vec<_>>()}),
        )
    }
}
impl FreeBusyResult {
    pub fn from_google(
        q: &FreeBusyRequest,
        value: Value,
        checked_at_ms: i64,
    ) -> Result<Self, SyncError> {
        q.validate()?;
        let lower = timestamp(&q.from)?;
        let upper = timestamp(&q.to)?;
        if value["kind"] != "calendar#freeBusy"
            || timestamp(text(&value, "timeMin")?)? != lower
            || timestamp(text(&value, "timeMax")?)? != upper
        {
            return Err(SyncError::Provider);
        }
        if value
            .get("groups")
            .is_some_and(|v| v.as_object().is_none_or(|g| !g.is_empty()))
        {
            return Err(SyncError::Provider);
        }
        let map = value["calendars"].as_object().ok_or(SyncError::Provider)?;
        if map.len() != q.calendar_ids.len() {
            return Err(SyncError::Provider);
        }
        let zone = q
            .time_zone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| SyncError::Provider)?;
        let mut calendars = Vec::new();
        let mut total = 0;
        for id in &q.calendar_ids {
            let item = map
                .get(id)
                .filter(|v| v.is_object())
                .ok_or(SyncError::Provider)?;
            let mut errors = Vec::new();
            if let Some(v) = item.get("errors") {
                let entries = v
                    .as_array()
                    .filter(|a| a.len() <= 100)
                    .ok_or(SyncError::Provider)?;
                for error in entries {
                    errors.push(AvailabilityError {
                        domain: text(error, "domain")?.into(),
                        reason: text(error, "reason")?.into(),
                    });
                }
            }
            let mut busy = Vec::new();
            match item.get("busy") {
                Some(v) => {
                    let periods = v.as_array().ok_or(SyncError::Provider)?;
                    total += periods.len();
                    if total > 20_000 {
                        return Err(SyncError::TooLarge);
                    }
                    for period in periods {
                        let from = timestamp(text(period, "start")?)?;
                        let to = timestamp(text(period, "end")?)?;
                        if from >= to || from >= upper || to <= lower {
                            return Err(SyncError::Provider);
                        }
                        busy.push(BusyPeriod {
                            from: from.max(lower).with_timezone(&zone).to_rfc3339(),
                            to: to.min(upper).with_timezone(&zone).to_rfc3339(),
                        });
                    }
                }
                None if !errors.is_empty() => {}
                None => return Err(SyncError::Provider),
            }
            calendars.push(CalendarAvailability {
                provider_calendar_id: id.clone(),
                complete: errors.is_empty(),
                busy,
                errors,
            });
        }
        Ok(Self {
            from: q.from.clone(),
            to: q.to.clone(),
            time_zone: q.time_zone.clone(),
            checked_at_ms,
            complete: calendars.iter().all(|c| c.complete),
            calendars,
        })
    }
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, SyncError> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 2048 && !s.chars().any(char::is_control))
        .ok_or(SyncError::Provider)
}
fn timestamp(v: &str) -> Result<DateTime<FixedOffset>, SyncError> {
    if v.len() > 128 {
        return Err(SyncError::Provider);
    }
    DateTime::parse_from_rfc3339(v).map_err(|_| SyncError::Provider)
}
