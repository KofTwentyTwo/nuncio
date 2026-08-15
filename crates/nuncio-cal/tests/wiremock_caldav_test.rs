//! WireMock integration test suite for the real CalDAV `REPORT` transport.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_cal::{CalDavAccountConfig, CalDavClient, CalendarBackend, CalendarError};
use nuncio_core::model::CalendarEvent;
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
        auth_token: "wiremock-app-token".to_string().into(),
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
        auth_token: "wiremock-app-token".to_string().into(),
    };
    let client = CalDavClient::new(config);

    let err = client
        .fetch_remote_events("cal-broken", 1_700_000_000, 1_800_000_000)
        .await
        .expect_err("a 500 response must be a real error, not fabricated empty success");

    assert!(matches!(err, CalendarError::TransportFailed(_)));
}

/// The same two events as [`multistatus_body`], serialized the way a server that binds the DAV
/// and CalDAV namespaces to different prefixes would write them. A substring-matching parser
/// reads zero events out of this and calls it success; a namespace-aware one reads two.
fn multistatus_body_with_uppercase_prefixes() -> String {
    multistatus_body()
        .replace("<d:", "<D:")
        .replace("</d:", "</D:")
        .replace(r#"xmlns:d="DAV:""#, r#"xmlns:D="DAV:""#)
        .replace("<c:", "<CAL:")
        .replace("</c:", "</CAL:")
        .replace(
            r#"xmlns:c="urn:ietf:params:xml:ns:caldav""#,
            r#"xmlns:CAL="urn:ietf:params:xml:ns:caldav""#,
        )
}

/// The same two events again, this time with `DAV:` as the document's default namespace, so no
/// DAV element carries a prefix at all.
fn multistatus_body_with_default_namespace() -> String {
    multistatus_body()
        .replace("<d:", "<")
        .replace("</d:", "</")
        .replace(r#"xmlns:d="DAV:""#, r#"xmlns="DAV:""#)
}

/// Serve `body` from a mock CalDAV endpoint and run one real `REPORT` round trip against it.
async fn fetch_against_body(
    body: String,
    account_id: &str,
) -> Result<Vec<CalendarEvent>, CalendarError> {
    let mock_server = MockServer::start().await;
    Mock::given(method("REPORT"))
        .and(path("/calendars/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(body))
        .mount(&mock_server)
        .await;

    let config = CalDavAccountConfig {
        account_id: account_id.to_string(),
        caldav_url: format!("{}/calendars/work/", mock_server.uri()),
        username: "james.maes".to_string(),
        auth_token: "wiremock-app-token".to_string().into(),
    };
    CalDavClient::new(config)
        .fetch_remote_events("cal-work", 1_700_000_000, 1_800_000_000)
        .await
}

#[tokio::test]
async fn wiremock_caldav_report_reads_the_same_document_under_any_namespace_prefix() {
    for (label, body) in [
        (
            "uppercase prefixes",
            multistatus_body_with_uppercase_prefixes(),
        ),
        (
            "default namespace",
            multistatus_body_with_default_namespace(),
        ),
    ] {
        let events = fetch_against_body(body, "acct-wm-ns")
            .await
            .unwrap_or_else(|e| panic!("{label} fetches: {e}"));
        assert_eq!(events.len(), 2, "{label}");
        assert!(
            events
                .iter()
                .any(|e| e.summary == "Wiremock CalDAV Standup"),
            "{label}"
        );
    }
}

#[tokio::test]
async fn wiremock_caldav_report_ignores_a_property_that_merely_contains_the_searched_name() {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
    <d:response>
        <d:href>/calendars/work/lookalike.ics</d:href>
        <d:propstat>
            <d:prop>
                <c:calendar-data-summary>BEGIN:VCALENDAR
BEGIN:VEVENT
SUMMARY:Decoy
END:VEVENT
END:VCALENDAR</c:calendar-data-summary>
            </d:prop>
            <d:status>HTTP/1.1 200 OK</d:status>
        </d:propstat>
    </d:response>
    <d:response>
        <d:href>/calendars/work/real.ics</d:href>
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
            <d:status>HTTP/1.1 200 OK</d:status>
        </d:propstat>
    </d:response>
</d:multistatus>"#;

    let events = fetch_against_body(body.to_string(), "acct-wm-lookalike")
        .await
        .expect("fetch succeeds");

    assert_eq!(events.len(), 1, "the decoy element is not calendar-data");
    assert_eq!(events[0].summary, "Wiremock UTC Review");
}

#[tokio::test]
async fn wiremock_caldav_report_treats_a_404_propstat_as_no_value() {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
    <d:response>
        <d:href>/calendars/work/gone.ics</d:href>
        <d:propstat>
            <d:prop>
                <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:Should Not Appear
DTSTART:20240301T120000Z
DTEND:20240301T130000Z
END:VEVENT
END:VCALENDAR</c:calendar-data>
            </d:prop>
            <d:status>HTTP/1.1 404 Not Found</d:status>
        </d:propstat>
    </d:response>
    <d:response>
        <d:href>/calendars/work/real.ics</d:href>
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
            <d:status>HTTP/1.1 200 OK</d:status>
        </d:propstat>
    </d:response>
</d:multistatus>"#;

    let events = fetch_against_body(body.to_string(), "acct-wm-404")
        .await
        .expect("fetch succeeds");

    assert_eq!(events.len(), 1, "a 404 propstat carries no value");
    assert_eq!(events[0].summary, "Wiremock UTC Review");
}

#[tokio::test]
async fn wiremock_caldav_report_malformed_body_surfaces_as_an_error_not_an_empty_calendar() {
    // Well-formed HTTP, unreadable body: the `<d:response>` element is never closed.
    let truncated = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:"><d:response><d:href>/calendars/work/a.ics</d:href>"#;

    let err = fetch_against_body(truncated.to_string(), "acct-wm-malformed")
        .await
        .expect_err("a body this client cannot read must not look like an empty calendar");
    assert!(matches!(err, CalendarError::MalformedResponse(_)));

    // A 200 response that is not a multistatus at all (an intercepting captive portal, say).
    let not_dav = "<html><body>Sign in to continue</body></html>";
    let err = fetch_against_body(not_dav.to_string(), "acct-wm-html")
        .await
        .expect_err("a non-multistatus body must not look like an empty calendar");
    assert!(matches!(err, CalendarError::MalformedResponse(_)));
}
