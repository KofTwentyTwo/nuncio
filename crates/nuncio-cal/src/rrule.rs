//! iCalendar recurrence expansion engine wrapping `rrule`.

use chrono::{DateTime, Utc};
use nuncio_core::model::CalendarEvent;
use rrule::RRuleSet;
use std::str::FromStr;

use crate::parser::CalendarError;

/// Recurrence engine managing RFC 5545 RRULE expansion within date windows.
pub struct RecurrenceEngine;

impl RecurrenceEngine {
    /// Maximum safety limit for expanded instances to prevent infinite loops.
    pub const MAX_EXPANSION_COUNT: u16 = 500;

    /// Expand recurring [`CalendarEvent`] instances within `[start_window, end_window]` timestamp bounds.
    pub fn expand_occurrences(
        event: &CalendarEvent,
        start_window: i64,
        end_window: i64,
    ) -> Result<Vec<CalendarEvent>, CalendarError> {
        let rrule_str = match &event.rrule {
            Some(rule) if !rule.trim().is_empty() => rule,
            _ => return Ok(vec![event.clone()]),
        };

        let start_dt = DateTime::from_timestamp(event.start_time, 0)
            .ok_or_else(|| CalendarError::ParseFailed("invalid start timestamp".to_string()))?;

        let rrule_set_str = format!("DTSTART:{}\nRRULE:{}", start_dt.format("%Y%m%dT%H%M%SZ"), rrule_str);

        let rrule_set: RRuleSet = rrule_set_str
            .parse()
            .map_err(|e: rrule::RRuleError| CalendarError::ParseFailed(e.to_string()))?;

        let window_start_dt = DateTime::from_timestamp(start_window, 0)
            .ok_or_else(|| CalendarError::ParseFailed("invalid window start timestamp".to_string()))?;

        let window_end_dt = DateTime::from_timestamp(end_window, 0)
            .ok_or_else(|| CalendarError::ParseFailed("invalid window end timestamp".to_string()))?;

        let duration = event.end_time - event.start_time;

        let results = rrule_set
            .into_iter()
            .after(window_start_dt)
            .before(window_end_dt)
            .take(Self::MAX_EXPANSION_COUNT as usize);

        let mut occurrences = Vec::new();
        for (index, dt) in results.enumerate() {
            let instance_start = dt.timestamp();
            let mut instance = event.clone();
            instance.id = format!("{}_occ_{}", event.id, index);
            instance.start_time = instance_start;
            instance.end_time = instance_start + duration;
            occurrences.push(instance);
        }

        if occurrences.is_empty() {
            // Return original if no occurrences fall strictly inside window
            Ok(vec![event.clone()])
        } else {
            Ok(occurrences)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_recurring_event() -> CalendarEvent {
        CalendarEvent {
            id: "evt-weekly".to_string(),
            account_id: "acct-1".to_string(),
            calendar_id: "cal-1".to_string(),
            summary: "Weekly Standup".to_string(),
            start_time: 1704067200, // 2024-01-01T00:00:00Z
            end_time: 1704070800,   // +1 hr
            rrule: Some("FREQ=WEEKLY;INTERVAL=1".to_string()),
            location: Some("Zoom".to_string()),
        }
    }

    #[test]
    fn non_recurring_event_returns_single_instance() {
        let mut event = sample_recurring_event();
        event.rrule = None;

        let results = RecurrenceEngine::expand_occurrences(&event, 1704067200, 1709251200)
            .expect("expansion succeeds");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "evt-weekly");
    }

    #[test]
    fn weekly_recurring_event_expands_multiple_instances() {
        let event = sample_recurring_event();
        // Window covers Jan 1 2024 to Feb 1 2024 (4 weeks)
        let results = RecurrenceEngine::expand_occurrences(&event, 1704067200, 1706745600)
            .expect("expansion succeeds");

        assert!(results.len() >= 3);
        assert!(results[0].id.contains("evt-weekly_occ_"));
    }

    #[test]
    fn invalid_rrule_returns_parse_error() {
        let mut event = sample_recurring_event();
        event.rrule = Some("INVALID_RRULE".to_string());

        let err = RecurrenceEngine::expand_occurrences(&event, 1704067200, 1706745600)
            .expect_err("should fail with invalid rrule");
        assert!(matches!(err, CalendarError::ParseFailed(_)));
    }
}
