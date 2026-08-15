//! A deliberately small, exact natural-language scheduling parser.
//!
//! The grammar below is the *entire* set of phrasings this parser understands.
//! Anything outside it returns an error. That is the point: a scheduler that
//! guesses produces a plausible-looking event at the wrong instant, and the user
//! only discovers the mistake by missing the meeting. Refusing is recoverable;
//! silently booking the wrong time is not.
//!
//! # Grammar
//!
//! ```text
//! <summary> <day> at <time> for <duration>
//! ```
//!
//! * `<summary>` — free text, must be non-empty. The `at`/`for` separators are
//!   matched from the right, so a summary may itself contain them
//!   ("Prep for review tomorrow at 9am for 30 minutes").
//! * `<day>` — `today`, `tomorrow`, or an ISO calendar date `YYYY-MM-DD`.
//! * `<time>` — `noon`, `midnight`, `3pm`, `3:15pm`, `9am`, `9:05am`, or a
//!   24-hour `HH:MM`. A bare number (`at 3`) is rejected as ambiguous.
//! * `<duration>` — `<n> minute(s)|min(s)` or `<n> hour(s)|hr(s)`, non-zero and
//!   no longer than 24 hours.
//!
//! Both `at <time>` and `for <duration>` are mandatory. Without a time there is
//! no instant to resolve, and a defaulted duration is a guess about the user's
//! intent, so neither is invented.
//!
//! # Deliberately unsupported
//!
//! Weekday phrasings (`next Tuesday`, `this Friday`, `on Tuesday`) are rejected
//! as ambiguous rather than resolved. English speakers do not agree on whether
//! "next Tuesday" means the Tuesday that is coming or the Tuesday of the
//! following week, and the two readings differ by a whole week. There is no
//! interpretation this parser could pick that would not be wrong for a large
//! share of users, so it asks instead.
//!
//! # Time zones
//!
//! Resolution requires an explicit IANA time zone: "3pm" is not an instant until
//! you know where the user is, and a Unix timestamp alone does not carry that.
//! Wall-clock times are resolved through the zone's DST rules, and the two
//! local-time anomalies are reported rather than papered over:
//!
//! * A time inside a spring-forward gap does not exist → [`NlpError::NonexistentLocalTime`].
//! * A time inside a fall-back overlap occurs twice → [`NlpError::AmbiguousLocalTime`].
//!
//! Durations are added as absolute elapsed seconds, so an event that spans a DST
//! transition keeps its real length and its wall-clock end time shifts, which is
//! what a physical meeting actually does.

use chrono::{Duration, LocalResult, NaiveDate, TimeZone};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The longest event this parser will accept, in minutes.
const MAX_DURATION_MINUTES: u32 = 24 * 60;

/// A successfully parsed scheduling request.
///
/// This is a parse result, not a calendar event: it deliberately carries no
/// account or calendar identity, because the parser has no way to know which
/// calendar the user meant and must not invent one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedSchedulingIntent {
    /// Event title, with the trailing scheduling phrase removed.
    pub summary: String,
    /// Start instant, Unix seconds UTC.
    pub start_time: i64,
    /// End instant, Unix seconds UTC (`start_time` plus the parsed duration).
    pub end_time: i64,
    /// Parsed duration in minutes.
    pub duration_minutes: u32,
}

/// Why a scheduling string could not be resolved to an exact instant.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum NlpError {
    #[error("empty scheduling query provided")]
    EmptyQuery,

    #[error("no event title found before the scheduling phrase")]
    MissingSummary,

    #[error("no time of day given; expected `... at <time> ...`")]
    MissingTime,

    #[error("no duration given; expected `... for <n> minutes`")]
    MissingDuration,

    #[error(
        "`{0}` is ambiguous: it can mean either the coming weekday or the one a \
         week later. Use `today`, `tomorrow`, or an explicit YYYY-MM-DD date"
    )]
    AmbiguousWeekday(String),

    #[error("unsupported day `{0}`; expected `today`, `tomorrow`, or YYYY-MM-DD")]
    UnsupportedDay(String),

    #[error("unsupported time `{0}`; expected `noon`, `midnight`, `3pm`, `3:15pm`, or `HH:MM`")]
    UnsupportedTime(String),

    #[error("unsupported duration `{0}`; expected `<n> minutes` or `<n> hours`, up to 24 hours")]
    UnsupportedDuration(String),

    #[error("{date} {time} does not exist in {zone}: the clocks skip forward over it")]
    NonexistentLocalTime {
        date: String,
        time: String,
        zone: String,
    },

    #[error("{date} {time} occurs twice in {zone}: the clocks fall back over it")]
    AmbiguousLocalTime {
        date: String,
        time: String,
        zone: String,
    },

    #[error("the reference timestamp {0} is not a representable instant")]
    InvalidReference(i64),

    #[error("the resolved instant overflows the representable range")]
    InstantOutOfRange,
}

/// Parser for the small scheduling grammar documented at the module level.
pub struct NaturalLanguageScheduler;

impl NaturalLanguageScheduler {
    /// Resolve `input` to an exact instant, relative to `reference_timestamp`
    /// (Unix seconds) interpreted in `zone`.
    ///
    /// `zone` is required rather than defaulted: relative days and wall-clock
    /// times are meaningless without it, and assuming a zone is the specific
    /// mistake that produces confidently wrong bookings.
    ///
    /// Returns [`NlpError`] for every phrasing outside the documented grammar.
    /// It never falls back to an approximation.
    pub fn parse(
        input: &str,
        reference_timestamp: i64,
        zone: Tz,
    ) -> Result<ParsedSchedulingIntent, NlpError> {
        let original = input.trim();
        if original.is_empty() {
            return Err(NlpError::EmptyQuery);
        }

        // ASCII lowercasing preserves byte length, so byte offsets found in the
        // lowered copy stay valid in `original` and the summary keeps its case.
        let lowered = original.to_ascii_lowercase();

        let (head, duration_text) =
            split_last(&lowered, original, " for ").ok_or(NlpError::MissingDuration)?;
        let duration_minutes = parse_duration(&duration_text.to_ascii_lowercase())?;

        let lowered_head = head.to_ascii_lowercase();
        let (summary_and_day, time_text) =
            split_last(&lowered_head, head, " at ").ok_or(NlpError::MissingTime)?;
        let (hour, minute) = parse_time_of_day(&time_text.to_ascii_lowercase())?;

        let reference = zone
            .timestamp_opt(reference_timestamp, 0)
            .single()
            .ok_or(NlpError::InvalidReference(reference_timestamp))?;

        let (summary, date) = split_day(summary_and_day, reference.date_naive())?;
        if summary.is_empty() {
            return Err(NlpError::MissingSummary);
        }

        let start = resolve_local(zone, date, hour, minute)?;
        let end = start
            .checked_add_signed(Duration::minutes(i64::from(duration_minutes)))
            .ok_or(NlpError::InstantOutOfRange)?;

        Ok(ParsedSchedulingIntent {
            summary,
            start_time: start.timestamp(),
            end_time: end.timestamp(),
            duration_minutes,
        })
    }
}

/// Split `original` at the last occurrence of `sep` located in `lowered`.
///
/// `lowered` must be the ASCII-lowercased form of `original` so their byte
/// offsets line up.
fn split_last<'a>(lowered: &str, original: &'a str, sep: &str) -> Option<(&'a str, &'a str)> {
    let at = lowered.rfind(sep)?;
    Some((&original[..at], &original[at + sep.len()..]))
}

/// Turn the trailing day word of `text` into a concrete date, returning the
/// remaining text as the summary.
fn split_day(text: &str, reference_date: NaiveDate) -> Result<(String, NaiveDate), NlpError> {
    let trimmed = text.trim_end();
    let (head, last) = match trimmed.rsplit_once(char::is_whitespace) {
        Some((head, last)) => (head, last),
        None => ("", trimmed),
    };
    let lowered = last.to_ascii_lowercase();

    let date = match lowered.as_str() {
        "today" => reference_date,
        "tomorrow" => reference_date
            .succ_opt()
            .ok_or(NlpError::InstantOutOfRange)?,
        other => {
            if let Ok(explicit) = NaiveDate::parse_from_str(other, "%Y-%m-%d") {
                explicit
            } else if is_weekday(other) {
                // Rebuild the phrase with any qualifier ("next", "this", "on")
                // so the error quotes back what the user actually wrote.
                let qualifier = head
                    .rsplit_once(char::is_whitespace)
                    .map_or(head, |(_, word)| word);
                let phrase = if matches!(
                    qualifier.to_ascii_lowercase().as_str(),
                    "next" | "this" | "on" | "coming"
                ) {
                    format!("{qualifier} {last}")
                } else {
                    last.to_string()
                };
                return Err(NlpError::AmbiguousWeekday(phrase));
            } else {
                return Err(NlpError::UnsupportedDay(last.to_string()));
            }
        }
    };

    Ok((head.trim().to_string(), date))
}

fn is_weekday(word: &str) -> bool {
    matches!(
        word,
        "monday"
            | "mon"
            | "tuesday"
            | "tue"
            | "tues"
            | "wednesday"
            | "wed"
            | "thursday"
            | "thu"
            | "thurs"
            | "friday"
            | "fri"
            | "saturday"
            | "sat"
            | "sunday"
            | "sun"
    )
}

/// Parse `noon`, `midnight`, a 12-hour `am`/`pm` time, or a 24-hour `HH:MM`.
///
/// A bare number is rejected: "at 3" gives no way to tell 03:00 from 15:00.
fn parse_time_of_day(text: &str) -> Result<(u32, u32), NlpError> {
    let raw = text.trim();
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let unsupported = || NlpError::UnsupportedTime(raw.to_string());

    match compact.as_str() {
        "noon" => return Ok((12, 0)),
        "midnight" => return Ok((0, 0)),
        _ => {}
    }

    if let Some(clock) = compact
        .strip_suffix("am")
        .map(|c| (c, false))
        .or_else(|| compact.strip_suffix("pm").map(|c| (c, true)))
    {
        let (digits, is_pm) = clock;
        let (hour, minute) = split_clock(digits, 1..=12, unsupported)?;
        let hour = match (hour, is_pm) {
            (12, false) => 0,
            (12, true) => 12,
            (h, false) => h,
            (h, true) => h + 12,
        };
        return Ok((hour, minute));
    }

    // 24-hour form must carry the colon, so a bare hour never parses.
    if !compact.contains(':') {
        return Err(unsupported());
    }
    split_clock(&compact, 0..=23, unsupported)
}

/// Split `H`, `H:MM`, or `HH:MM` into an hour within `hours` and a minute.
fn split_clock(
    text: &str,
    hours: std::ops::RangeInclusive<u32>,
    unsupported: impl Fn() -> NlpError,
) -> Result<(u32, u32), NlpError> {
    let (hour_text, minute_text) = match text.split_once(':') {
        Some((h, m)) => (h, m),
        None => (text, "0"),
    };
    if hour_text.is_empty() || !hour_text.chars().all(|c| c.is_ascii_digit()) {
        return Err(unsupported());
    }
    // A minute field is always exactly two digits; "3:5" is a typo, not 03:05.
    if text.contains(':')
        && (minute_text.len() != 2 || !minute_text.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(unsupported());
    }
    let hour: u32 = hour_text.parse().map_err(|_| unsupported())?;
    let minute: u32 = minute_text.parse().map_err(|_| unsupported())?;
    if !hours.contains(&hour) || minute > 59 {
        return Err(unsupported());
    }
    Ok((hour, minute))
}

/// Parse `<n> minutes` / `<n> hours` into a minute count.
fn parse_duration(text: &str) -> Result<u32, NlpError> {
    let raw = text.trim();
    let unsupported = || NlpError::UnsupportedDuration(raw.to_string());

    let mut parts = raw.split_whitespace();
    let (Some(count_text), Some(unit), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(unsupported());
    };

    let count: u32 = count_text.parse().map_err(|_| unsupported())?;
    let minutes = match unit {
        "minute" | "minutes" | "min" | "mins" => count,
        "hour" | "hours" | "hr" | "hrs" => count.checked_mul(60).ok_or_else(unsupported)?,
        _ => return Err(unsupported()),
    };

    if minutes == 0 || minutes > MAX_DURATION_MINUTES {
        return Err(unsupported());
    }
    Ok(minutes)
}

/// Resolve a wall-clock time in `zone`, reporting both DST anomalies honestly.
fn resolve_local(
    zone: Tz,
    date: NaiveDate,
    hour: u32,
    minute: u32,
) -> Result<chrono::DateTime<Tz>, NlpError> {
    let local = date
        .and_hms_opt(hour, minute, 0)
        .ok_or(NlpError::InstantOutOfRange)?;

    match zone.from_local_datetime(&local) {
        LocalResult::Single(instant) => Ok(instant),
        LocalResult::None => Err(NlpError::NonexistentLocalTime {
            date: date.to_string(),
            time: format!("{hour:02}:{minute:02}"),
            zone: zone.name().to_string(),
        }),
        LocalResult::Ambiguous(_, _) => Err(NlpError::AmbiguousLocalTime {
            date: date.to_string(),
            time: format!("{hour:02}:{minute:02}"),
            zone: zone.name().to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::{America::New_York, Europe::London};

    // 2026-03-05 10:00 EST — a plain winter weekday, well clear of any DST edge.
    const REF_NY: i64 = 1_772_722_800;
    // 2026-03-07 12:00 EST — the day before US DST begins (2026-03-08 02:00).
    const REF_NY_BEFORE_SPRING_FORWARD: i64 = 1_772_902_800;
    // 2026-10-31 12:00 EDT — the day before US DST ends (2026-11-01 02:00).
    const REF_NY_BEFORE_FALL_BACK: i64 = 1_793_462_400;
    // 2026-03-05 10:00 GMT — same wall clock as REF_NY, a different instant.
    const REF_LONDON: i64 = 1_772_704_800;

    fn parse(input: &str, reference: i64, zone: Tz) -> ParsedSchedulingIntent {
        NaturalLanguageScheduler::parse(input, reference, zone).unwrap()
    }

    fn err(input: &str, reference: i64, zone: Tz) -> NlpError {
        NaturalLanguageScheduler::parse(input, reference, zone).unwrap_err()
    }

    // --- supported grammar: exact instants -------------------------------

    #[test]
    fn tomorrow_at_a_pm_time_resolves_to_the_exact_instant() {
        let intent = parse(
            "Coffee with Bob tomorrow at 2pm for 45 minutes",
            REF_NY,
            New_York,
        );
        assert_eq!(intent.summary, "Coffee with Bob");
        // 2026-03-06 14:00 EST.
        assert_eq!(intent.start_time, 1_772_823_600);
        assert_eq!(intent.end_time, 1_772_823_600 + 45 * 60);
        assert_eq!(intent.duration_minutes, 45);
    }

    #[test]
    fn noon_and_midnight_resolve_to_the_exact_instant() {
        let noon = parse("Lunch tomorrow at noon for 1 hour", REF_NY, New_York);
        assert_eq!(noon.start_time, 1_772_816_400); // 2026-03-06 12:00 EST
        assert_eq!(noon.duration_minutes, 60);

        let midnight = parse("Deploy tomorrow at midnight for 30 mins", REF_NY, New_York);
        assert_eq!(midnight.start_time, 1_772_773_200); // 2026-03-06 00:00 EST
    }

    #[test]
    fn today_resolves_against_the_reference_date_not_the_wall_clock() {
        let intent = parse("Retro today at 9:30am for 30 minutes", REF_NY, New_York);
        assert_eq!(intent.summary, "Retro");
        // 2026-03-05 09:30 EST — before the reference instant, and still exact.
        assert_eq!(intent.start_time, 1_772_721_000);
    }

    #[test]
    fn an_explicit_iso_date_resolves_to_the_exact_instant() {
        let intent = parse(
            "Standup 2026-03-08 at 01:30 for 120 minutes",
            REF_NY,
            New_York,
        );
        assert_eq!(intent.summary, "Standup");
        assert_eq!(intent.start_time, 1_772_951_400); // 2026-03-08 01:30 EST
    }

    #[test]
    fn separators_are_matched_from_the_right_so_summaries_may_contain_them() {
        let intent = parse(
            "Prep for review at HQ tomorrow at 2pm for 45 minutes",
            REF_NY,
            New_York,
        );
        assert_eq!(intent.summary, "Prep for review at HQ");
        assert_eq!(intent.start_time, 1_772_823_600);
    }

    #[test]
    fn every_documented_duration_unit_is_accepted() {
        for (phrase, expected) in [
            ("15 minutes", 15),
            ("15 minute", 15),
            ("15 mins", 15),
            ("15 min", 15),
            ("2 hours", 120),
            ("1 hour", 60),
            ("2 hrs", 120),
            ("1 hr", 60),
        ] {
            let input = format!("Sync tomorrow at 2pm for {phrase}");
            assert_eq!(
                parse(&input, REF_NY, New_York).duration_minutes,
                expected,
                "{phrase}"
            );
        }
    }

    #[test]
    fn every_documented_time_form_is_accepted() {
        for (phrase, expected_hour, expected_minute) in [
            ("3pm", 15, 0),
            ("3:15pm", 15, 15),
            ("9am", 9, 0),
            ("9:05am", 9, 5),
            ("12am", 0, 0),
            ("12pm", 12, 0),
            ("00:00", 0, 0),
            ("23:59", 23, 59),
            ("noon", 12, 0),
            ("midnight", 0, 0),
        ] {
            let input = format!("Sync tomorrow at {phrase} for 30 minutes");
            let intent = parse(&input, REF_NY, New_York);
            let expected = New_York
                .with_ymd_and_hms(2026, 3, 6, expected_hour, expected_minute, 0)
                .unwrap()
                .timestamp();
            assert_eq!(intent.start_time, expected, "{phrase}");
        }
    }

    // --- time zones -------------------------------------------------------

    #[test]
    fn the_same_phrase_in_a_different_zone_is_a_different_instant() {
        let input = "Sync tomorrow at 2pm for 30 minutes";
        let ny = parse(input, REF_NY, New_York);
        let london = parse(input, REF_LONDON, London);

        assert_eq!(ny.start_time, 1_772_823_600); // 2026-03-06 14:00 EST
        assert_eq!(london.start_time, 1_772_805_600); // 2026-03-06 14:00 GMT
                                                      // Same wall clock, five hours apart — the zone is load-bearing, so a
                                                      // parser that ignored it would be wrong for one of these callers.
        assert_eq!(ny.start_time - london.start_time, 5 * 3600);
    }

    #[test]
    fn a_time_inside_the_spring_forward_gap_is_refused() {
        let error = err(
            "Sync tomorrow at 2:30am for 30 minutes",
            REF_NY_BEFORE_SPRING_FORWARD,
            New_York,
        );
        assert_eq!(
            error,
            NlpError::NonexistentLocalTime {
                date: "2026-03-08".to_string(),
                time: "02:30".to_string(),
                zone: "America/New_York".to_string(),
            }
        );
    }

    #[test]
    fn a_time_just_after_the_spring_forward_gap_resolves_exactly() {
        let intent = parse(
            "Sync tomorrow at 3am for 30 minutes",
            REF_NY_BEFORE_SPRING_FORWARD,
            New_York,
        );
        // 2026-03-08 03:00 EDT — one hour of real time after 01:00 EST, not two.
        assert_eq!(intent.start_time, 1_772_953_200);
    }

    #[test]
    fn a_time_inside_the_fall_back_overlap_is_refused() {
        let error = err(
            "Sync tomorrow at 1:30am for 30 minutes",
            REF_NY_BEFORE_FALL_BACK,
            New_York,
        );
        assert_eq!(
            error,
            NlpError::AmbiguousLocalTime {
                date: "2026-11-01".to_string(),
                time: "01:30".to_string(),
                zone: "America/New_York".to_string(),
            }
        );
    }

    #[test]
    fn a_duration_spanning_a_dst_transition_keeps_its_real_length() {
        let intent = parse(
            "Standup 2026-03-08 at 01:30 for 120 minutes",
            REF_NY,
            New_York,
        );
        // Elapsed time is exactly two hours...
        assert_eq!(intent.end_time - intent.start_time, 120 * 60);
        // ...but the clocks jumped, so the wall-clock end is 04:30, not 03:30.
        let end = New_York.timestamp_opt(intent.end_time, 0).unwrap();
        assert_eq!(
            end.format("%Y-%m-%d %H:%M %Z").to_string(),
            "2026-03-08 04:30 EDT"
        );
    }

    // --- refusals ---------------------------------------------------------

    #[test]
    fn empty_and_blank_queries_are_refused() {
        assert_eq!(err("", REF_NY, New_York), NlpError::EmptyQuery);
        assert_eq!(err("   ", REF_NY, New_York), NlpError::EmptyQuery);
    }

    #[test]
    fn weekday_phrasings_are_refused_as_ambiguous_rather_than_guessed() {
        for (input, quoted) in [
            ("Coffee next Tuesday at 2pm for 45 minutes", "next Tuesday"),
            ("Coffee this Friday at 2pm for 45 minutes", "this Friday"),
            ("Coffee on Monday at 2pm for 45 minutes", "on Monday"),
            ("Coffee Tuesday at 2pm for 45 minutes", "Tuesday"),
        ] {
            assert_eq!(
                err(input, REF_NY, New_York),
                NlpError::AmbiguousWeekday(quoted.to_string()),
                "{input}"
            );
        }
    }

    #[test]
    fn a_missing_time_or_duration_is_refused_rather_than_defaulted() {
        assert_eq!(
            err("Coffee tomorrow at 2pm", REF_NY, New_York),
            NlpError::MissingDuration
        );
        assert_eq!(
            err("Coffee tomorrow for 45 minutes", REF_NY, New_York),
            NlpError::MissingTime
        );
        assert_eq!(
            err("Coffee with Bob", REF_NY, New_York),
            NlpError::MissingDuration
        );
    }

    #[test]
    fn a_missing_summary_is_refused() {
        assert_eq!(
            err("tomorrow at 2pm for 45 minutes", REF_NY, New_York),
            NlpError::MissingSummary
        );
    }

    #[test]
    fn unsupported_day_phrasings_are_refused() {
        for phrase in [
            "next week",
            "in 3 days",
            "the 5th",
            "eod",
            "yesterday",
            "fortnight",
        ] {
            let input = format!("Sync {phrase} at 2pm for 30 minutes");
            let error = err(&input, REF_NY, New_York);
            assert!(
                matches!(error, NlpError::UnsupportedDay(_)),
                "{phrase} produced {error:?}"
            );
        }
    }

    #[test]
    fn a_bare_hour_is_refused_because_it_cannot_be_disambiguated() {
        for phrase in [
            "3",
            "15",
            "half past three",
            "teatime",
            "3 o'clock",
            "25:00",
            "13pm",
        ] {
            let input = format!("Sync tomorrow at {phrase} for 30 minutes");
            let error = err(&input, REF_NY, New_York);
            assert!(
                matches!(error, NlpError::UnsupportedTime(_)),
                "{phrase} produced {error:?}"
            );
        }
    }

    #[test]
    fn malformed_minute_fields_are_refused() {
        for phrase in ["3:5pm", "3:60pm", "3:", ":30", "3:015"] {
            let input = format!("Sync tomorrow at {phrase} for 30 minutes");
            let error = err(&input, REF_NY, New_York);
            assert!(
                matches!(error, NlpError::UnsupportedTime(_)),
                "{phrase} produced {error:?}"
            );
        }
    }

    #[test]
    fn unsupported_or_out_of_range_durations_are_refused() {
        for phrase in [
            "a while",
            "0 minutes",
            "1.5 hours",
            "25 hours",
            "1441 minutes",
            "30",
            "half an hour",
            "2 days",
        ] {
            let input = format!("Sync tomorrow at 2pm for {phrase}");
            let error = err(&input, REF_NY, New_York);
            assert!(
                matches!(error, NlpError::UnsupportedDuration(_)),
                "{phrase} produced {error:?}"
            );
        }
    }

    #[test]
    fn an_invalid_calendar_date_is_refused() {
        let error = err("Sync 2026-02-30 at 2pm for 30 minutes", REF_NY, New_York);
        assert!(matches!(error, NlpError::UnsupportedDay(_)), "{error:?}");
    }

    #[test]
    fn the_boundary_duration_of_exactly_24_hours_is_accepted() {
        let intent = parse("Hackathon tomorrow at noon for 24 hours", REF_NY, New_York);
        assert_eq!(intent.duration_minutes, MAX_DURATION_MINUTES);
        assert_eq!(intent.end_time - intent.start_time, 24 * 3600);
    }
}
