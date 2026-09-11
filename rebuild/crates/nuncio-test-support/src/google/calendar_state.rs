use super::wire::{Reply, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Serialize)]
pub struct NotificationEffect {
    pub event_id: String,
    pub action: String,
    pub policy: String,
    pub recipients: Vec<String>,
}
#[derive(Clone, Serialize)]
pub struct CalendarSnapshot {
    pub events: BTreeMap<String, Value>,
    pub version: u64,
    pub notifications: Vec<NotificationEffect>,
}
pub(super) struct Calendar {
    pub access_role: String,
    pub omitted_fields: std::collections::BTreeSet<String>,
    pub google_guests: std::collections::BTreeSet<String>,
    pub id: String,
    pub primary: bool,
    pub events: BTreeMap<String, Event>,
    pub version: u64,
    pub notifications: Vec<NotificationEffect>,
}
#[derive(Clone)]
pub(super) struct Event {
    pub body: Value,
    pub version: u64,
}
pub(super) struct SyncCursor {
    pub account: String,
    pub calendar: String,
    pub shape: String,
    pub version: u64,
}

impl Calendar {
    pub fn event(&self, id: &str) -> Result<Value> {
        if let Some(event) = self.events.get(id) {
            return Ok(event.body.clone());
        }
        let (master, stamp) = id
            .rsplit_once('_')
            .ok_or_else(|| Reply::error(404, "notFound"))?;
        let original = NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ")
            .map_err(|_| Reply::error(404, "notFound"))?
            .and_utc();
        let day = chrono::Duration::days(1);
        super::calendar_recurrence::expanded(
            self,
            original - day,
            original + day,
            Some(master),
            true,
        )?
        .into_iter()
        .find(|event| event["id"] == id)
        .ok_or_else(|| Reply::error(404, "notFound"))
    }
    pub fn seeded(account: &str, primary: bool) -> Result<Self> {
        let id = if primary {
            account.into()
        } else {
            format!("team-{account}")
        };
        let seed: Vec<Value> = if primary {
            serde_json::from_str(include_str!("../../fixtures/calendar/canonical.json"))
                .map_err(|_| Reply::error(500, "invalidFixture"))?
        } else {
            Vec::new()
        };
        let mut calendar = Self {
            access_role: "owner".into(),
            omitted_fields: Default::default(),
            google_guests: [
                "alpha@example.test",
                "beta@example.test",
                "guest@example.test",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            id,
            primary,
            events: BTreeMap::new(),
            version: 1,
            notifications: Vec::new(),
        };
        for mut body in seed {
            let id = body["id"]
                .as_str()
                .ok_or_else(|| Reply::error(500, "invalidFixture"))?
                .to_string();
            stamp(&mut body, 1);
            calendar.events.insert(id, Event { body, version: 1 });
        }
        Ok(calendar)
    }
    pub fn snapshot(&self) -> CalendarSnapshot {
        CalendarSnapshot {
            events: self
                .events
                .iter()
                .map(|(id, e)| (id.clone(), e.body.clone()))
                .collect(),
            version: self.version,
            notifications: self.notifications.clone(),
        }
    }
    pub fn listing(&self) -> Value {
        let mut value = json!({"id":self.id,"summary":if self.primary {"Personal"} else {"Team"},"primary":self.primary,
        "accessRole":self.access_role,"timeZone":"America/Chicago","kind":"calendar#calendarListEntry","etag":format!("\"cal-{}\"",self.version)});
        if let Some(object) = value.as_object_mut() {
            for field in &self.omitted_fields {
                object.remove(field);
            }
        }
        value
    }
    pub fn put(&mut self, mut body: Value, policy: &str, action: &str) -> Result<Value> {
        let id = body["id"]
            .as_str()
            .ok_or_else(|| Reply::error(400, "invalidArgument"))?
            .to_string();
        self.version += 1;
        stamp(&mut body, self.version);
        let recipients: Vec<_> = body["attendees"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["email"].as_str())
            .filter(|email| {
                policy == "all" || policy == "externalOnly" && !self.google_guests.contains(*email)
            })
            .map(String::from)
            .collect();
        if !recipients.is_empty() {
            self.notifications.push(NotificationEffect {
                event_id: id.clone(),
                action: action.into(),
                policy: policy.into(),
                recipients,
            });
        }
        self.events.insert(
            id,
            Event {
                body: body.clone(),
                version: self.version,
            },
        );
        Ok(body)
    }
}
fn stamp(body: &mut Value, version: u64) {
    body["etag"] = json!(format!("\"event-{version}\""));
    body["kind"] = json!("calendar#event");
    body["updated"] = json!("2026-09-10T00:00:00Z");
}
pub(super) fn instant(value: &Value, calendar_zone: &str) -> Result<DateTime<Utc>> {
    if let Some(date) = value["date"].as_str() {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|_| Reply::error(400, "invalidTime"))?;
        let zone = calendar_zone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| Reply::error(400, "invalidTimeZone"))?;
        return zone
            .from_local_datetime(
                &date
                    .and_hms_opt(0, 0, 0)
                    .ok_or_else(|| Reply::error(400, "invalidTime"))?,
            )
            .single()
            .map(|d| d.with_timezone(&Utc))
            .ok_or_else(|| Reply::error(400, "invalidTime"));
    }
    let text = value["dateTime"]
        .as_str()
        .ok_or_else(|| Reply::error(400, "invalidTime"))?;
    if let Ok(date) = DateTime::parse_from_rfc3339(text) {
        return Ok(date.with_timezone(&Utc));
    }
    let zone = value["timeZone"]
        .as_str()
        .ok_or_else(|| Reply::error(400, "invalidTimeZone"))?
        .parse::<chrono_tz::Tz>()
        .map_err(|_| Reply::error(400, "invalidTimeZone"))?;
    let local = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
        .map_err(|_| Reply::error(400, "invalidTime"))?;
    zone.from_local_datetime(&local)
        .single()
        .map(|d| d.with_timezone(&Utc))
        .ok_or_else(|| Reply::error(400, "invalidTime"))
}
pub(super) fn timestamp(text: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| Reply::error(400, "invalidTime"))
}
pub(super) fn overlap(event: &Value, lower: DateTime<Utc>, upper: DateTime<Utc>) -> Result<bool> {
    let (start, end) = if event["status"] == "cancelled" && event.get("start").is_none() {
        let start = instant(&event["originalStartTime"], "America/Chicago")?;
        (start, start)
    } else {
        (
            instant(&event["start"], "America/Chicago")?,
            instant(&event["end"], "America/Chicago")?,
        )
    };
    Ok(start < upper && end > lower)
}
