//! Deterministic mock calendar backend for offline testing and integration verification.

use nuncio_core::model::CalendarEvent;
use std::sync::{Arc, Mutex};

use crate::parser::CalendarError;

/// Thread-safe mock calendar backend for offline testing.
#[derive(Debug, Clone, Default)]
pub struct MockCalendarBackend {
    events: Arc<Mutex<Vec<CalendarEvent>>>,
    should_fail: Arc<Mutex<bool>>,
}

impl MockCalendarBackend {
    /// Create a new `MockCalendarBackend`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the mock to simulate network failure errors.
    pub fn set_should_fail(&self, fail: bool) {
        if let Ok(mut flag) = self.should_fail.lock() {
            *flag = fail;
        }
    }

    /// Add a mock calendar event to storage.
    pub fn add_event(&self, event: CalendarEvent) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(event);
        }
    }

    /// Retrieve calendar events for a specific calendar collection within a time window.
    pub fn list_events(
        &self,
        calendar_id: &str,
        start_window: i64,
        end_window: i64,
    ) -> Result<Vec<CalendarEvent>, CalendarError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| CalendarError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(CalendarError::ParseFailed(
                "simulated CalDAV network failure".to_string(),
            ));
        }

        let guard = self
            .events
            .lock()
            .map_err(|e| CalendarError::ParseFailed(e.to_string()))?;
        let matches = guard
            .iter()
            .filter(|e| {
                e.calendar_id == calendar_id
                    && e.start_time >= start_window
                    && e.end_time <= end_window
            })
            .cloned()
            .collect();

        Ok(matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn mock_calendar_backend_operations() {
        let mock = MockCalendarBackend::new();
        let event = CalendarEvent {
            id: "evt-m1".to_string(),
            account_id: "acct-1".to_string(),
            calendar_id: "cal-work".to_string(),
            summary: "Mock Planning".to_string(),
            start_time: 1700000000,
            end_time: 1700003600,
            rrule: None,
            location: None,
        };
        mock.add_event(event.clone());

        let events = mock
            .list_events("cal-work", 1699999000, 1700004000)
            .expect("list events succeeds");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "evt-m1");

        mock.set_should_fail(true);
        assert!(mock
            .list_events("cal-work", 1699999000, 1700004000)
            .is_err());
    }

    #[tokio::test]
    async fn wiremock_caldav_report_simulation() {
        let mock_server = MockServer::start().await;

        let xml_response = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
            <d:response>
                <d:href>/calendars/work/evt.ics</d:href>
                <d:propstat>
                    <d:prop>
                        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:Wiremock Event
END:VEVENT
END:VCALENDAR</c:calendar-data>
                    </d:prop>
                </d:propstat>
            </d:response>
        </d:multistatus>"#;

        Mock::given(method("REPORT"))
            .and(path("/calendars/work/"))
            .respond_with(ResponseTemplate::new(207).set_body_string(xml_response))
            .mount(&mock_server)
            .await;

        let client = reqwest::Client::new();
        let resp = client
            .request(
                reqwest::Method::from_bytes(b"REPORT").unwrap(),
                format!("{}/calendars/work/", mock_server.uri()),
            )
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 207);
    }
}
