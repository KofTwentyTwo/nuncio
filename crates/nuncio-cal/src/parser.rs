//! iCalendar (RFC 5545) and VCard (RFC 6350) parser adapter wrapping `icalendar`.

use icalendar::{Component, EventLike};
use nuncio_core::model::CalendarEvent;
use thiserror::Error;

/// Errors returned by the iCalendar parsing engine.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum CalendarError {
    /// Failed to parse RFC 5545 iCalendar payload.
    #[error("failed to parse iCalendar payload: {0}")]
    ParseFailed(String),
}

/// iCalendar parser adapter converting raw `.ics` payloads into Nuncio [`CalendarEvent`] domain entities.
pub struct IcalParserAdapter;

impl IcalParserAdapter {
    /// Parse a raw iCalendar RFC 5545 string into a [`CalendarEvent`] entity.
    pub fn parse_ical(
        id: &str,
        account_id: &str,
        calendar_id: &str,
        raw_ics: &str,
    ) -> Result<CalendarEvent, CalendarError> {
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

                let start_time = event
                    .get_start()
                    .map(|d| match d {
                        icalendar::DatePerhapsTime::Date(date) => date
                            .and_hms_opt(0, 0, 0)
                            .map_or(0, |dt| dt.and_utc().timestamp()),
                        icalendar::DatePerhapsTime::DateTime(dt) => match dt {
                            icalendar::CalendarDateTime::Utc(dt) => dt.timestamp(),
                            icalendar::CalendarDateTime::Floating(dt) => dt.and_utc().timestamp(),
                            _ => 0,
                        },
                    })
                    .unwrap_or(0);

                let end_time = event
                    .get_end()
                    .map(|d| match d {
                        icalendar::DatePerhapsTime::Date(date) => date
                            .and_hms_opt(23, 59, 59)
                            .map_or(0, |dt| dt.and_utc().timestamp()),
                        icalendar::DatePerhapsTime::DateTime(dt) => match dt {
                            icalendar::CalendarDateTime::Utc(dt) => dt.timestamp(),
                            icalendar::CalendarDateTime::Floating(dt) => dt.and_utc().timestamp(),
                            _ => 0,
                        },
                    })
                    .unwrap_or(start_time + 3600);

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
}
