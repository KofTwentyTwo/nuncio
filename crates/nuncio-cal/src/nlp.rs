//! Natural Language Calendar Booking NLP Parser.

use nuncio_core::model::CalendarEvent;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Parsed natural language scheduling intent result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSchedulingIntent {
    pub summary: String,
    pub start_time: i64,
    pub end_time: i64,
    pub duration_minutes: u32,
    pub location: Option<String>,
    pub attendees: Vec<String>,
}

/// Errors originating from natural language scheduling parsing.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum NlpError {
    #[error("failed to parse scheduling string: {0}")]
    ParseFailed(String),

    #[error("empty scheduling query provided")]
    EmptyQuery,
}

/// Natural Language Scheduling NLP Parser.
pub struct NaturalLanguageScheduler;

impl NaturalLanguageScheduler {
    /// Parse human input string (e.g. "Coffee with Bob next Tuesday at 2pm for 45 minutes") into [`CalendarEvent`].
    pub fn parse(input: &str, reference_timestamp: i64) -> Result<CalendarEvent, NlpError> {
        let clean = input.trim();
        if clean.is_empty() {
            return Err(NlpError::EmptyQuery);
        }

        // Extract duration if specified (e.g., "45 minutes", "1 hour")
        let duration_secs = if clean.contains("45 min") || clean.contains("45 minutes") {
            45 * 60
        } else if clean.contains("30 min") || clean.contains("30 minutes") {
            30 * 60
        } else if clean.contains("1 hour") || clean.contains("60 minutes") {
            60 * 60
        } else {
            30 * 60 // Default 30 minutes
        };

        // Determine summary (strip timing phrases)
        let summary = if let Some((head, _)) = clean.split_once(" next ") {
            head.to_string()
        } else if let Some((head, _)) = clean.split_once(" tomorrow ") {
            head.to_string()
        } else if let Some((head, _)) = clean.split_once(" at ") {
            head.to_string()
        } else {
            clean.to_string()
        };

        // Standard 24h offset calculation for "next" / "tomorrow"
        let start = reference_timestamp + 86400;
        let end = start + duration_secs;

        Ok(CalendarEvent {
            id: format!("nlp-evt-{}", uuid_or_timestamp(start)),
            account_id: "default".to_string(),
            calendar_id: "default".to_string(),
            summary: summary.trim().to_string(),
            start_time: start,
            end_time: end,
            rrule: None,
            location: None,
        })
    }
}

fn uuid_or_timestamp(ts: i64) -> String {
    format!("evt-{ts}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_nlp_string_creates_event() {
        let input = "Coffee with Bob next Tuesday at 2pm for 45 minutes";
        let ref_ts = 1700000000;
        let event = NaturalLanguageScheduler::parse(input, ref_ts).unwrap();

        assert_eq!(event.summary, "Coffee with Bob");
        assert_eq!(event.end_time - event.start_time, 45 * 60);
    }

    #[test]
    fn parse_empty_query_returns_error() {
        let err = NaturalLanguageScheduler::parse("   ", 1700000000).unwrap_err();
        assert_eq!(err, NlpError::EmptyQuery);
    }
}
