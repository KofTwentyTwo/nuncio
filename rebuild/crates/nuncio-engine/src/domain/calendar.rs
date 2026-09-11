use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("Calendar data or event time is invalid")]
    Invalid,
    #[error("Agenda requires increasing ISO dates within a 366-day window")]
    InvalidWindow,
    #[error("Calendar resource exceeds the supported size")]
    TooLarge,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum EventTime {
    Date { date: String },
    DateTime { date_time: ZonedDateTime },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ZonedDateTime {
    pub rfc3339: String,
    pub time_zone: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgendaWindow {
    pub from: String,
    pub to: String,
}
impl AgendaWindow {
    pub fn new(from: &str, to: &str) -> Result<Self, CalendarError> {
        let a = date(from)?;
        let b = date(to)?;
        let days = b.signed_duration_since(a).num_days();
        if !(1..=366).contains(&days) {
            return Err(CalendarError::InvalidWindow);
        }
        Ok(Self {
            from: from.into(),
            to: to.into(),
        })
    }
    pub fn rolling(now_ms: i64) -> Result<Self, CalendarError> {
        let today = DateTime::<Utc>::from_timestamp_millis(now_ms)
            .ok_or(CalendarError::InvalidWindow)?
            .date_naive();
        let from = today
            .checked_sub_signed(Duration::days(30))
            .ok_or(CalendarError::InvalidWindow)?;
        // 'Through 180 days after today' includes that date; upper bounds are exclusive.
        let to = today
            .checked_add_signed(Duration::days(181))
            .ok_or(CalendarError::InvalidWindow)?;
        Self::new(&from.to_string(), &to.to_string())
    }
    pub fn provider_bounds(&self, zone: &str) -> Result<(String, String), CalendarError> {
        Self::new(&self.from, &self.to)?;
        let zone: Tz = zone.parse().map_err(|_| CalendarError::Invalid)?;
        Ok((
            midnight(&self.from, zone)?.to_rfc3339(),
            midnight(&self.to, zone)?.to_rfc3339(),
        ))
    }
    pub fn bounds_ms(&self, zone: &str) -> Result<(i64, i64), CalendarError> {
        let (from, to) = self.provider_bounds(zone)?;
        Ok((
            DateTime::parse_from_rfc3339(&from)
                .map_err(|_| CalendarError::Invalid)?
                .timestamp_millis(),
            DateTime::parse_from_rfc3339(&to)
                .map_err(|_| CalendarError::Invalid)?
                .timestamp_millis(),
        ))
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct CalendarObject {
    pub provider_id: String,
    pub summary: Option<String>,
    pub etag: Option<String>,
    pub status: String,
    pub start: Option<EventTime>,
    pub end: Option<EventTime>,
    pub original_start: Option<EventTime>,
    pub recurring_provider_id: Option<String>,
    pub provider_json: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}
impl CalendarObject {
    pub fn from_google(value: Value, calendar_zone: &str) -> Result<Self, CalendarError> {
        let provider_json = serde_json::to_string(&value).map_err(|_| CalendarError::Invalid)?;
        if provider_json.len() > 1024 * 1024 {
            return Err(CalendarError::TooLarge);
        }
        let provider_id = text(&value, "id")?.ok_or(CalendarError::Invalid)?;
        let status = text(&value, "status")?.unwrap_or_else(|| "confirmed".into());
        if !matches!(status.as_str(), "confirmed" | "tentative" | "cancelled") {
            return Err(CalendarError::Invalid);
        }
        let start = value
            .get("start")
            .map(|v| time(v, calendar_zone))
            .transpose()?;
        let end = value
            .get("end")
            .map(|v| time(v, calendar_zone))
            .transpose()?;
        let original_start = value
            .get("originalStartTime")
            .map(|v| time(v, calendar_zone))
            .transpose()?;
        let recurring_provider_id = text(&value, "recurringEventId")?;
        if status != "cancelled" && (start.is_none() || end.is_none()) {
            return Err(CalendarError::Invalid);
        }
        if recurring_provider_id.is_some() && original_start.is_none() {
            return Err(CalendarError::Invalid);
        }
        let start_ms = start
            .as_ref()
            .or(original_start.as_ref())
            .map(|t| instant(t, calendar_zone))
            .transpose()?;
        let end_ms = end
            .as_ref()
            .map(|t| instant(t, calendar_zone))
            .transpose()?;
        if let (Some(start), Some(end)) = (&start, &end) {
            if matches!(start, EventTime::Date { .. }) != matches!(end, EventTime::Date { .. })
                || start_ms >= end_ms
            {
                return Err(CalendarError::Invalid);
            }
        }
        Ok(Self {
            provider_id,
            summary: text(&value, "summary")?,
            etag: text(&value, "etag")?,
            status,
            start,
            end,
            original_start,
            recurring_provider_id,
            provider_json,
            start_ms,
            end_ms,
        })
    }
}
fn text(value: &Value, key: &str) -> Result<Option<String>, CalendarError> {
    match value.get(key) {
        None => Ok(None),
        Some(Value::String(s))
            if s.len() <= 65536
                && (!matches!(key, "id" | "etag" | "recurringEventId")
                    || !s.is_empty() && s.len() <= 2048 && !s.chars().any(char::is_control)) =>
        {
            Ok(Some(s.clone()))
        }
        _ => Err(CalendarError::Invalid),
    }
}
fn date(value: &str) -> Result<NaiveDate, CalendarError> {
    if value.len() != 10
        || !value.bytes().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
    {
        return Err(CalendarError::InvalidWindow);
    }
    let date =
        NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| CalendarError::InvalidWindow)?;
    if !(1..=9999).contains(&date.year()) {
        return Err(CalendarError::InvalidWindow);
    }
    Ok(date)
}
fn midnight(value: &str, zone: Tz) -> Result<DateTime<Tz>, CalendarError> {
    zone.from_local_datetime(
        &date(value)?
            .and_hms_opt(0, 0, 0)
            .ok_or(CalendarError::Invalid)?,
    )
    .single()
    .ok_or(CalendarError::Invalid)
}
fn time(value: &Value, calendar_zone: &str) -> Result<EventTime, CalendarError> {
    let value = value.as_object().ok_or(CalendarError::Invalid)?;
    if value.contains_key("date") == value.contains_key("dateTime") {
        return Err(CalendarError::Invalid);
    }
    if let Some(date_value) = value.get("date") {
        let s = date_value.as_str().ok_or(CalendarError::Invalid)?;
        date(s)?;
        return Ok(EventTime::Date { date: s.into() });
    }
    let raw = value
        .get("dateTime")
        .and_then(Value::as_str)
        .ok_or(CalendarError::Invalid)?;
    if raw.len() > 64 {
        return Err(CalendarError::Invalid);
    }
    let explicit_zone = value
        .get("timeZone")
        .map(|v| v.as_str().ok_or(CalendarError::Invalid))
        .transpose()?;
    let zone: Tz = explicit_zone
        .unwrap_or(calendar_zone)
        .parse()
        .map_err(|_| CalendarError::Invalid)?;
    let normalized = if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        if !(1..=9999).contains(&parsed.year()) {
            return Err(CalendarError::Invalid);
        }
        raw.to_owned()
    } else {
        if explicit_zone.is_none() {
            return Err(CalendarError::Invalid);
        }
        let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
            .map_err(|_| CalendarError::Invalid)?;
        zone.from_local_datetime(&naive)
            .single()
            .ok_or(CalendarError::Invalid)?
            .to_rfc3339()
    };
    Ok(EventTime::DateTime {
        date_time: ZonedDateTime {
            rfc3339: normalized,
            time_zone: zone.to_string(),
        },
    })
}
fn instant(time: &EventTime, calendar_zone: &str) -> Result<i64, CalendarError> {
    match time {
        EventTime::Date { date } => Ok(midnight(
            date,
            calendar_zone.parse().map_err(|_| CalendarError::Invalid)?,
        )?
        .timestamp_millis()),
        EventTime::DateTime { date_time } => Ok(DateTime::parse_from_rfc3339(&date_time.rfc3339)
            .map_err(|_| CalendarError::Invalid)?
            .timestamp_millis()),
    }
}
