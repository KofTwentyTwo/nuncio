//! WireMock integration test suite for the real CalDAV `REPORT` transport.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_cal::{CalDavAccountConfig, CalDavClient, CalendarBackend, CalendarError};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A canned CalDAV `multistatus` response containing one TZID-qualified VEVENT and one plain
/// UTC VEVENT, exercising both the real HTTP transport and the TZID resolution fix together.
fn multistatus_body() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
    <d:response>
        <d:href>/calendars/work/standup.ics</d:href>
        <d:propstat>
            <d:prop>
                <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:Wiremock CalDAV Standup
LOCATION:Conference Room B
DTSTART;TZID=America/New_York:20240715T090000
DTEND;TZID=America/New_York:20240715T100000
END:VEVENT
END:VCALENDAR</c:calendar-data>
            </d:prop>
        </d:propstat>
    </d:response>
    <d:response>
        <d:href>/calendars/work/utc-review.ics</d:href>
        <d:propstat>
            <d:prop>
                <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:Wiremock UTC Review
DTSTART:20240301T120000Z
DTEND:20240301T130000Z
END:VEVENT
END:VCALENDAR</c:calendar-data>
            </d:prop>
        </d:propstat>
    </d:response>
</d:multistatus>"#
        .to_string()
}

#[tokio::test]
async fn wiremock_caldav_report_fetches_and_resolves_tzid_events() {
    let mock_server = MockServer::start().await;

    Mock::given(method("REPORT"))
        .and(path("/calendars/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(multistatus_body()))
        .mount(&mock_server)
        .await;

    let config = CalDavAccountConfig {
        account_id: "acct-wm-1".to_string(),
        caldav_url: format!("{}/calendars/work/", mock_server.uri()),
        username: "james.maes".to_string(),
        auth_token: "wiremock-app-token".to_string(),
    };
    let client = CalDavClient::new(config);

    // Drive the real client through the `CalendarBackend` trait, exactly how the daemon will
    // invoke either this real client or `MockCalendarBackend` behind the same seam.
    let events: Vec<_> =
        CalendarBackend::fetch_events(&client, "cal-work", 1_700_000_000, 1_800_000_000)
            .await
            .expect("fetch_events succeeds against the wiremock server");

    assert_eq!(events.len(), 2);

    let tzid_event = events
        .iter()
        .find(|e| e.summary == "Wiremock CalDAV Standup")
        .expect("TZID event present");
    assert_eq!(tzid_event.account_id, "acct-wm-1");
    assert_eq!(tzid_event.calendar_id, "cal-work");
    assert_eq!(tzid_event.location, Some("Conference Room B".to_string()));
    // 2024-07-15 09:00 America/New_York is EDT (UTC-4) -> 2024-07-15T13:00:00Z.
    assert_eq!(tzid_event.start_time, 1_721_048_400);
    assert_eq!(tzid_event.end_time, 1_721_052_000);

    let utc_event = events
        .iter()
        .find(|e| e.summary == "Wiremock UTC Review")
        .expect("UTC event present");
    assert_eq!(utc_event.start_time, 1_709_294_400);
    assert_eq!(utc_event.end_time, 1_709_298_000);
}

#[tokio::test]
async fn wiremock_caldav_report_server_error_surfaces_as_transport_failure() {
    let mock_server = MockServer::start().await;

    Mock::given(method("REPORT"))
        .and(path("/calendars/broken/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let config = CalDavAccountConfig {
        account_id: "acct-wm-2".to_string(),
        caldav_url: format!("{}/calendars/broken/", mock_server.uri()),
        username: "james.maes".to_string(),
        auth_token: "wiremock-app-token".to_string(),
    };
    let client = CalDavClient::new(config);

    let err = client
        .fetch_remote_events("cal-broken", 1_700_000_000, 1_800_000_000)
        .await
        .expect_err("a 500 response must be a real error, not fabricated empty success");

    assert!(matches!(err, CalendarError::TransportFailed(_)));
}
