//! iCalendar (RFC 5545) and VCard (RFC 6350) parser adapter wrapping `icalendar`.

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use icalendar::{Component, EventLike};
use nuncio_core::model::CalendarEvent;
use std::str::FromStr;
use thiserror::Error;

/// Errors returned by the iCalendar parsing engine.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum CalendarError {
    /// Failed to parse RFC 5545 iCalendar payload.
    #[error("failed to parse iCalendar payload: {0}")]
    ParseFailed(String),

    /// A `DTSTART`/`DTEND` carried a `TZID` parameter (RFC 5545 3.3.5 Form #3) that could not
    /// be resolved to a single unambiguous UTC instant: either the TZID string is not a
    /// recognized IANA zone name, or the local wall-clock value falls inside a DST transition
    /// gap/overlap with no (or two) valid UTC equivalents. Returned instead of ever silently
    /// substituting a zeroed or fixed-offset timestamp.
    #[error("unresolvable TZID timezone reference: {0}")]
    UnresolvableTimezone(String),

    /// CalDAV network/transport-layer failure (connection, TLS, or an unexpected HTTP status)
    /// distinct from a payload that connected fine but failed to parse.
    #[error("CalDAV transport failure: {0}")]
    TransportFailed(String),
}

/// Resolve an iCalendar `DATE`-or-`DATE-TIME` value to a UTC unix timestamp.
///
/// `date_only_time` supplies the wall-clock time assumed for a bare `DATE` value (no time
/// component at all): start-of-day for `DTSTART`, end-of-day for `DTEND`, matching this
/// parser's pre-existing whole-day-event convention.
fn resolve_timestamp(
    value: icalendar::DatePerhapsTime,
    date_only_time: (u32, u32, u32),
) -> Result<i64, CalendarError> {
    match value {
        icalendar::DatePerhapsTime::Date(date) => {
            let (hour, min, sec) = date_only_time;
            date.and_hms_opt(hour, min, sec)
                .map(|naive| naive.and_utc().timestamp())
                .ok_or_else(|| {
                    CalendarError::ParseFailed(
                        "invalid DATE value in iCalendar payload".to_string(),
                    )
                })
        }
        icalendar::DatePerhapsTime::DateTime(dt) => match dt {
            icalendar::CalendarDateTime::Utc(dt) => Ok(dt.timestamp()),
            // Floating times (RFC 5545 3.3.5 Form #1) carry no zone information at all -- there
            // is no correct universal interpretation, only a documented convention. Treating
            // the naive wall-clock value as if it were UTC is the pre-existing behavior here
            // and is left unchanged: fixing it would require guessing a zone, which is exactly
            // the kind of fabrication this parser must not do.
            icalendar::CalendarDateTime::Floating(dt) => Ok(dt.and_utc().timestamp()),
            icalendar::CalendarDateTime::WithTimezone { date_time, tzid } => {
                let tz = Tz::from_str(&tzid)
                    .map_err(|_| CalendarError::UnresolvableTimezone(tzid.clone()))?;
                // `.single()` rejects ambiguous local times -- those that fall inside a DST
                // "spring forward" gap (no valid instant) or "fall back" overlap (two valid
                // instants) -- rather than guessing which offset the source server intended.
                tz.from_local_datetime(&date_time)
                    .single()
                    .map(|resolved| resolved.with_timezone(&Utc).timestamp())
                    .ok_or(CalendarError::UnresolvableTimezone(tzid))
            }
        },
    }
}

/// iCalendar parser adapter converting raw `.ics` payloads into Nuncio [`CalendarEvent`] domain entities.
pub struct IcalParserAdapter;

impl IcalParserAdapter {
    /// Maximum allowed iCalendar payload size (25MB).
    pub const MAX_PAYLOAD_BYTES: usize = 25 * 1024 * 1024;

    /// Parse a raw iCalendar RFC 5545 string into a [`CalendarEvent`] entity.
    pub fn parse_ical(
        id: &str,
        account_id: &str,
        calendar_id: &str,
        raw_ics: &str,
    ) -> Result<CalendarEvent, CalendarError> {
        if raw_ics.len() > Self::MAX_PAYLOAD_BYTES {
            return Err(CalendarError::ParseFailed(
                "iCalendar payload exceeds maximum allowed limit of 25MB".to_string(),
            ));
        }

        let calendar: icalendar::Calendar = raw_ics
            .parse()
            .map_err(|e: String| CalendarError::ParseFailed(e))?;

        for component in &calendar.components {
            if let icalendar::CalendarComponent::Event(event) = component {
                let summary = event.get_summary().unwrap_or("No Summary").to_string();
                let location = event.get_location().map(|l| l.to_string());
                let rrule = event
                    .properties()
                    .get("RRULE")
                    .map(|p| p.value().to_string());

                let start_time = match event.get_start() {
                    Some(value) => resolve_timestamp(value, (0, 0, 0))?,
                    None => 0,
                };

                let end_time = match event.get_end() {
                    Some(value) => resolve_timestamp(value, (23, 59, 59))?,
                    None => start_time + 3600,
                };

                return Ok(CalendarEvent {
                    id: id.to_string(),
                    account_id: account_id.to_string(),
                    calendar_id: calendar_id.to_string(),
                    summary,
                    start_time,
                    end_time,
                    rrule,
                    location,
                });
            }
        }

        Err(CalendarError::ParseFailed(
            "no VEVENT component found in iCalendar payload".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_vevent_ics() {
        let ics = "BEGIN:VCALENDAR\r\n\
                   VERSION:2.0\r\n\
                   BEGIN:VEVENT\r\n\
                   SUMMARY:Architecture Review\r\n\
                   LOCATION:Conference Room 101\r\n\
                   RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
                   END:VEVENT\r\n\
                   END:VCALENDAR";

        let event = IcalParserAdapter::parse_ical("evt-100", "acct-1", "cal-1", ics)
            .expect("parse succeeds");

        assert_eq!(event.id, "evt-100");
        assert_eq!(event.summary, "Architecture Review");
        assert_eq!(event.location, Some("Conference Room 101".to_string()));
        assert_eq!(event.rrule, Some("FREQ=WEEKLY;BYDAY=MO".to_string()));
    }

    #[test]
    fn parse_ics_without_vevent_fails() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR";
        let err = IcalParserAdapter::parse_ical("evt-101", "acct-1", "cal-1", ics)
            .expect_err("should fail without VEVENT");
        assert_eq!(
            err,
            CalendarError::ParseFailed(
                "no VEVENT component found in iCalendar payload".to_string()
            )
        );
        assert_eq!(
            err.to_string(),
            "failed to parse iCalendar payload: no VEVENT component found in iCalendar payload"
        );
    }

    #[test]
    fn parse_oversized_ics_returns_error() {
        let oversized = "A".repeat(IcalParserAdapter::MAX_PAYLOAD_BYTES + 1);
        let err = IcalParserAdapter::parse_ical("evt-huge", "acct-1", "cal-1", &oversized)
            .expect_err("should fail for oversized payload");
        assert!(err
            .to_string()
            .contains("exceeds maximum allowed limit of 25MB"));
    }

    /// Regression for the TZID bug: `DTSTART;TZID=America/New_York:...` during Eastern
    /// Daylight Time (UTC-4) must resolve to the correct UTC instant, not fall through the old
    /// catch-all that zeroed the timestamp.
    #[test]
    fn parse_tzid_dtstart_during_edt_resolves_correct_utc_epoch() {
        let ics = "BEGIN:VCALENDAR\r\n\
                   VERSION:2.0\r\n\
                   BEGIN:VEVENT\r\n\
                   SUMMARY:Standup EDT\r\n\
                   DTSTART;TZID=America/New_York:20240715T090000\r\n\
                   DTEND;TZID=America/New_York:20240715T100000\r\n\
                   END:VEVENT\r\n\
                   END:VCALENDAR";

        let event = IcalParserAdapter::parse_ical("evt-tz-edt", "acct-1", "cal-1", ics)
            .expect("parse succeeds");

        assert_ne!(
            event.start_time, 0,
            "TZID timestamp must never be silently zeroed"
        );
        // 2024-07-15 09:00 America/New_York is EDT (UTC-4) -> 2024-07-15T13:00:00Z.
        assert_eq!(event.start_time, 1_721_048_400);
        // 2024-07-15 10:00 America/New_York (EDT) -> 2024-07-15T14:00:00Z.
        assert_eq!(event.end_time, 1_721_052_000);
    }

    /// Same wall-clock hour as the EDT fixture above but on the winter side of the DST
    /// boundary: America/New_York is EST (UTC-5) there, so the resolved UTC instant must
    /// differ by an hour from the summer fixture -- proving the fix performs a genuine
    /// chrono_tz calendar-aware lookup rather than assuming a fixed UTC offset.
    #[test]
    fn parse_tzid_dtstart_during_est_resolves_correct_utc_epoch_across_dst_boundary() {
        let ics = "BEGIN:VCALENDAR\r\n\
                   VERSION:2.0\r\n\
                   BEGIN:VEVENT\r\n\
                   SUMMARY:Standup EST\r\n\
                   DTSTART;TZID=America/New_York:20240115T090000\r\n\
                   DTEND;TZID=America/New_York:20240115T100000\r\n\
                   END:VEVENT\r\n\
                   END:VCALENDAR";

        let event = IcalParserAdapter::parse_ical("evt-tz-est", "acct-1", "cal-1", ics)
            .expect("parse succeeds");

        assert_ne!(
            event.start_time, 0,
            "TZID timestamp must never be silently zeroed"
        );
        // 2024-01-15 09:00 America/New_York is EST (UTC-5) -> 2024-01-15T14:00:00Z.
        assert_eq!(event.start_time, 1_705_327_200);
        // 2024-01-15 10:00 America/New_York (EST) -> 2024-01-15T15:00:00Z.
        assert_eq!(event.end_time, 1_705_330_800);
    }

    /// Plain `Z`-suffixed UTC datetimes must keep working exactly as before the TZID fix.
    #[test]
    fn parse_plain_utc_z_dtstart_has_no_regression() {
        let ics = "BEGIN:VCALENDAR\r\n\
                   VERSION:2.0\r\n\
                   BEGIN:VEVENT\r\n\
                   SUMMARY:UTC Event\r\n\
                   DTSTART:20240301T120000Z\r\n\
                   DTEND:20240301T130000Z\r\n\
                   END:VEVENT\r\n\
                   END:VCALENDAR";

        let event = IcalParserAdapter::parse_ical("evt-utc", "acct-1", "cal-1", ics)
            .expect("parse succeeds");

        assert_eq!(event.start_time, 1_709_294_400);
        assert_eq!(event.end_time, 1_709_298_000);
    }

    /// An unresolvable TZID (not a real IANA zone) must surface as a genuine error, never a
    /// fabricated zero timestamp.
    #[test]
    fn parse_unknown_tzid_returns_error_not_a_fabricated_zero() {
        let ics = "BEGIN:VCALENDAR\r\n\
                   VERSION:2.0\r\n\
                   BEGIN:VEVENT\r\n\
                   SUMMARY:Bogus TZID Event\r\n\
                   DTSTART;TZID=Not/A_Real_Zone:20240115T090000\r\n\
                   END:VEVENT\r\n\
                   END:VCALENDAR";

        let err = IcalParserAdapter::parse_ical("evt-bad-tz", "acct-1", "cal-1", ics)
            .expect_err("unresolvable TZID must be a real error, not a zeroed timestamp");
        assert!(
            matches!(err, CalendarError::UnresolvableTimezone(ref tz) if tz == "Not/A_Real_Zone")
        );
    }

    /// A local time inside a DST "spring forward" gap has no valid instant, so
    /// `from_local_datetime(..).single()` yields `None`. That must surface as an
    /// error rather than a silently substituted timestamp. In America/New_York,
    /// 2024-03-10 02:30 never occurs (clocks jump 02:00 -> 03:00).
    #[test]
    fn parse_tzid_in_dst_gap_returns_error_not_a_fabricated_timestamp() {
        let ics = "BEGIN:VCALENDAR\r\n\
                   VERSION:2.0\r\n\
                   BEGIN:VEVENT\r\n\
                   SUMMARY:Nonexistent Local Time\r\n\
                   DTSTART;TZID=America/New_York:20240310T023000\r\n\
                   END:VEVENT\r\n\
                   END:VCALENDAR";

        let err = IcalParserAdapter::parse_ical("evt-gap", "acct-1", "cal-1", ics).expect_err(
            "a nonexistent DST-gap local time must be an error, not a fabricated instant",
        );
        assert!(
            matches!(err, CalendarError::UnresolvableTimezone(ref tz) if tz == "America/New_York")
        );
    }
}
