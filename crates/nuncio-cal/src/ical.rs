//! iCalendar (RFC 5545) VEVENT serializer -- the write-back inverse of
//! [`crate::parser::IcalParserAdapter`].
//!
//! [`CalendarEvent`] stores `start_time`/`end_time` as resolved UTC unix timestamps (the parser
//! already collapses any `TZID`/floating/`Z` form down to a single UTC instant), so this
//! serializer always emits `Z`-suffixed UTC `DATE-TIME` values. That is a lossless encoding of
//! what the model actually holds: re-parsing the emitted text yields the same UTC instant no
//! matter which form the original payload used.

use chrono::{DateTime, Utc};
use nuncio_core::model::CalendarEvent;

/// Adds RFC 5545 serialization to [`CalendarEvent`].
///
/// `CalendarEvent` is defined in `nuncio-core`, so this is an extension trait rather than an
/// inherent impl -- Rust forbids inherent impls on types owned by another crate. Importing the
/// trait makes `event.to_ical()` read identically to an inherent method at call sites.
pub trait ToIcal {
    /// Serialize this event as a complete iCalendar payload: a single `VEVENT` wrapped in a
    /// `VCALENDAR`, the shape a CalDAV `PUT` request body expects.
    fn to_ical(&self) -> String;
}

impl ToIcal for CalendarEvent {
    fn to_ical(&self) -> String {
        let dtstamp = format_utc_datetime(Utc::now().timestamp());
        let dtstart = format_utc_datetime(self.start_time);
        let dtend = format_utc_datetime(self.end_time);

        let mut vevent = String::new();
        vevent.push_str("BEGIN:VEVENT\r\n");
        vevent.push_str(&format!("UID:{}\r\n", escape_text(&self.id)));
        vevent.push_str(&format!("DTSTAMP:{}\r\n", dtstamp));
        vevent.push_str(&format!("DTSTART:{}\r\n", dtstart));
        vevent.push_str(&format!("DTEND:{}\r\n", dtend));
        vevent.push_str(&format!("SUMMARY:{}\r\n", escape_text(&self.summary)));
        if let Some(location) = &self.location {
            vevent.push_str(&format!("LOCATION:{}\r\n", escape_text(location)));
        }
        if let Some(rrule) = &self.rrule {
            // RRULE is a structured value (RFC 5545 3.3.10), not TEXT -- it must not go through
            // TEXT escaping, and the parser reads it back verbatim via `properties().get`.
            vevent.push_str(&format!("RRULE:{}\r\n", rrule));
        }
        vevent.push_str("END:VEVENT\r\n");

        let mut vcalendar = String::new();
        vcalendar.push_str("BEGIN:VCALENDAR\r\n");
        vcalendar.push_str("VERSION:2.0\r\n");
        vcalendar.push_str("PRODID:-//Nuncio//Calendar Engine//EN\r\n");
        vcalendar.push_str(&vevent);
        vcalendar.push_str("END:VCALENDAR\r\n");
        vcalendar
    }
}

/// Format a UTC unix timestamp as an RFC 5545 `DATE-TIME` in UTC form (`YYYYMMDDTHHMMSSZ`).
fn format_utc_datetime(unix_ts: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix_ts, 0)
        .map(|dt| dt.format("%Y%m%dT%H%M%SZ").to_string())
        // `from_timestamp` only returns `None` for a timestamp outside chrono's representable
        // range; falling back to the Unix epoch keeps this a total function without panicking,
        // since a corrupt in-memory timestamp is not something this serializer can repair.
        .unwrap_or_else(|| "19700101T000000Z".to_string())
}

/// Escape a `TEXT` value per RFC 5545 3.3.11: backslash, semicolon, and comma are escaped with a
/// leading backslash, and embedded newlines become the two-character literal `\n`. Order matters
/// -- backslash must be escaped first so the escaping added for the other characters is not
/// itself re-escaped.
fn escape_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            ';' => escaped.push_str("\\;"),
            ',' => escaped.push_str("\\,"),
            '\n' => escaped.push_str("\\n"),
            '\r' => {}
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::IcalParserAdapter;

    fn event(
        summary: &str,
        start_time: i64,
        end_time: i64,
        rrule: Option<&str>,
        location: Option<&str>,
    ) -> CalendarEvent {
        CalendarEvent {
            id: "evt-roundtrip".to_string(),
            account_id: "acct-1".to_string(),
            calendar_id: "cal-1".to_string(),
            summary: summary.to_string(),
            start_time,
            end_time,
            rrule: rrule.map(str::to_string),
            location: location.map(str::to_string),
        }
    }

    fn roundtrip(original: &CalendarEvent) -> CalendarEvent {
        let ics = original.to_ical();
        IcalParserAdapter::parse_ical(
            &original.id,
            &original.account_id,
            &original.calendar_id,
            &ics,
        )
        .expect("serialized iCalendar must parse back")
    }

    /// Proves the serializer is the true inverse of [`IcalParserAdapter::parse_ical`] across the
    /// event shapes CalDAV write-back must support: a plain timed event, an all-day event
    /// (whole-day UTC window), an event whose UTC instant originated from a `TZID` source (the
    /// model has already resolved it to UTC, so round-tripping through `to_ical` must preserve
    /// that same instant), and a recurring event carrying an `RRULE`.
    #[test]
    fn to_ical_roundtrips_through_parse_ical_for_all_event_shapes() {
        let timed = event(
            "Standup",
            1_709_294_400,
            1_709_298_000,
            None,
            Some("Room 5"),
        );
        assert_eq!(roundtrip(&timed), timed);

        // Whole-day window: 2024-03-01T00:00:00Z .. 2024-03-01T23:59:59Z, matching the parser's
        // own DATE-only convention (start of day / end of day).
        let all_day = event("Conference", 1_709_251_200, 1_709_337_599, None, None);
        assert_eq!(roundtrip(&all_day), all_day);

        // 2024-07-15 09:00 America/New_York (EDT, UTC-4) resolves to this UTC instant; the model
        // only ever stores the resolved UTC epoch, so serializing and re-parsing must land on
        // exactly the same instant even though the TZID itself is not retained.
        let from_tzid = event("Standup EDT", 1_721_048_400, 1_721_052_000, None, None);
        assert_eq!(roundtrip(&from_tzid), from_tzid);

        let recurring = event(
            "Weekly Sync",
            1_709_294_400,
            1_709_298_000,
            Some("FREQ=WEEKLY;BYDAY=MO"),
            Some("Conference Room 101"),
        );
        assert_eq!(roundtrip(&recurring), recurring);
    }

    #[test]
    fn to_ical_escapes_rfc5545_special_characters_in_text_fields() {
        let special = event(
            "Comma, semicolon; backslash\\ and\nnewline",
            1_709_294_400,
            1_709_298_000,
            None,
            Some("Loc; with, chars"),
        );
        let ics = special.to_ical();
        assert!(ics.contains("SUMMARY:Comma\\, semicolon\\; backslash\\\\ and\\nnewline\r\n"));
        assert!(ics.contains("LOCATION:Loc\\; with\\, chars\r\n"));
        assert_eq!(roundtrip(&special), special);
    }

    #[test]
    fn to_ical_wraps_a_single_vevent_in_a_vcalendar_shell() {
        let plain = event("Solo Event", 1_709_294_400, 1_709_298_000, None, None);
        let ics = plain.to_ical();
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(ics.contains("VERSION:2.0\r\n"));
        assert!(ics.trim_end().ends_with("END:VCALENDAR"));
        assert_eq!(ics.matches("BEGIN:VEVENT").count(), 1);
    }
}
