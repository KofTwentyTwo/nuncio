use super::{payload, CalendarChangePayload, CalendarObject, CalendarWriteKind};
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub enum CalendarRetryEvidence {
    Unchanged(Value),
    Missing { provider_event_id: String },
}
impl CalendarChangePayload {
    pub fn permits_retry(&self, evidence: &CalendarRetryEvidence) -> bool {
        match evidence {
            CalendarRetryEvidence::Missing { provider_event_id } => {
                self.kind == CalendarWriteKind::Create
                    && *provider_event_id == self.provider_event_id
            }
            CalendarRetryEvidence::Unchanged(value) => {
                self.kind != CalendarWriteKind::Create
                    && value["id"] == self.provider_event_id
                    && value["status"] != "cancelled"
                    && self.expected_etag.is_some()
                    && value["etag"].as_str() == self.expected_etag.as_deref()
                    && CalendarObject::from_google(value.clone(), &self.time_zone).is_ok()
            }
        }
    }
}
pub enum CalendarChangeReceipt {
    Event(Value),
    Deleted { provider_event_id: String },
}
impl CalendarChangePayload {
    pub fn satisfied_by(&self, receipt: &CalendarChangeReceipt) -> bool {
        match receipt {
            CalendarChangeReceipt::Deleted { provider_event_id } => {
                self.kind == CalendarWriteKind::Delete
                    && *provider_event_id == self.provider_event_id
            }
            CalendarChangeReceipt::Event(event) => {
                if event["id"] != self.provider_event_id {
                    return false;
                }
                if self.kind == CalendarWriteKind::Delete {
                    return event["status"] == "cancelled";
                }
                if event["status"] == "cancelled"
                    || event["etag"]
                        .as_str()
                        .is_none_or(|e| e.is_empty() || self.expected_etag.as_deref() == Some(e))
                {
                    return false;
                }
                if self.kind == CalendarWriteKind::Respond {
                    let desired = &self.patch["attendees"][0];
                    return event["attendees"].as_array().is_some_and(|a| {
                        let matches: Vec<_> = a.iter().filter(|a| same_email(a, desired)).collect();
                        matches.len() == 1 && contains(matches[0], desired)
                    });
                }
                contains(event, &self.patch)
            }
        }
    }
}
// Compare the named intent, allowing response-only fields and time normalization.
fn contains(observed: &Value, desired: &Value) -> bool {
    match desired {
        Value::Object(fields) => fields.iter().all(|(key, value)| {
            if key == "attendees" {
                return value.as_array().zip(observed[key].as_array()).is_some_and(
                    |(wanted, actual)| {
                        wanted.len() == actual.len()
                            && wanted.iter().all(|w| {
                                actual.iter().filter(|a| same_email(a, w)).count() == 1
                                    && actual.iter().any(|a| same_email(a, w) && contains(a, w))
                            })
                    },
                );
            }
            if key == "dateTime" {
                return time_instant(desired)
                    .zip(time_instant(observed))
                    .is_some_and(|(a, b)| a == b)
                    || value == &observed[key];
            }
            if key == "email" {
                return same_email(observed, desired);
            }
            contains(&observed[key], value)
        }),
        _ => observed == desired,
    }
}
fn time_instant(time: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::TimeZone;
    let raw = time["dateTime"].as_str()?;
    if let Ok(instant) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(instant.with_timezone(&chrono::Utc));
    }
    let zone: chrono_tz::Tz = time["timeZone"].as_str()?.parse().ok()?;
    let local = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    zone.from_local_datetime(&local)
        .single()
        .map(|v| v.with_timezone(&chrono::Utc))
}
fn same_email(a: &Value, b: &Value) -> bool {
    a["email"]
        .as_str()
        .zip(b["email"].as_str())
        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
}
pub(in crate::store) fn apply(
    c: &Connection,
    account: &str,
    id: &str,
    receipt: &CalendarChangeReceipt,
) -> Result<(String, Option<String>, bool), StoreError> {
    let intent = payload(c, account, id)?;
    if !intent.satisfied_by(receipt) {
        return Err(StoreError::InvalidInput);
    }
    let body = match receipt {
        CalendarChangeReceipt::Event(value) => value.clone(),
        CalendarChangeReceipt::Deleted { provider_event_id } => {
            let mut tombstone = intent.base.clone().unwrap_or(json!({}));
            tombstone["id"] = json!(provider_event_id);
            tombstone["status"] = json!("cancelled");
            if let Some(o) = tombstone.as_object_mut() {
                o.remove("etag");
            }
            tombstone
        }
    };
    let (provider, etag) = observe(c, account, &intent, body)?;
    Ok((provider, etag, intent.notifications != "none"))
}
pub(in crate::store) fn observe(
    c: &Connection,
    account: &str,
    intent: &CalendarChangePayload,
    body: Value,
) -> Result<(String, Option<String>), StoreError> {
    if body["id"] != intent.provider_event_id {
        return Err(StoreError::InvalidInput);
    }
    let event = CalendarObject::from_google(body, &intent.time_zone)
        .map_err(|_| StoreError::InvalidInput)?;
    let calendar: Option<String> = c
        .query_row(
            "SELECT id FROM calendars WHERE account_id=?1 AND provider_id=?2",
            params![account, intent.provider_calendar_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(calendar) = calendar {
        c.execute("INSERT OR IGNORE INTO calendar_event_ids(account_id,calendar_id,provider_id,id) VALUES (?1,?2,?3,?4)",params![account,calendar,event.provider_id,intent.local_event_id])?;
        let encoded = serde_json::to_string(&event).map_err(|_| StoreError::InvalidInput)?;
        c.execute("INSERT INTO calendar_objects(account_id,calendar_id,provider_id,object_json,status,etag,start_ms,end_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(account_id,calendar_id,provider_id) DO UPDATE SET object_json=excluded.object_json,status=excluded.status,etag=excluded.etag,start_ms=excluded.start_ms,end_ms=excluded.end_ms",params![account,calendar,event.provider_id,encoded,event.status,event.etag,event.start_ms,event.end_ms])?;
        c.execute("UPDATE calendars SET canonical_revision=canonical_revision+1 WHERE account_id=?1 AND id=?2",params![account,calendar])?;
        c.execute(
            "UPDATE agenda_coverage SET state='stale' WHERE account_id=?1 AND calendar_id=?2",
            params![account, calendar],
        )?;
        super::super::changes::record(c, Some(account), "calendar", Some(&intent.local_event_id))?;
    }
    Ok((event.provider_id, event.etag))
}
