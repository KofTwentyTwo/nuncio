//! CalDAV (RFC 4791) / WebDAV REPORT client and XML query generator.

use async_trait::async_trait;
use nuncio_core::dav::{parse_multistatus, CALDAV_NS};
use nuncio_core::model::CalendarEvent;
use nuncio_core::redact::Redacted;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use crate::backend::CalendarBackend;
use crate::parser::{CalendarError, IcalParserAdapter};

/// Configuration for a specific CalDAV calendar collection endpoint, mirroring
/// `nuncio_contacts::CardDavAccountConfig`.
///
/// `caldav_url` must already resolve to a specific calendar collection (e.g.
/// `https://caldav.example.com/dav/calendars/user/jmaes/work/`) -- PROPFIND-based
/// `calendar-home-set` auto-discovery is out of scope for this client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalDavAccountConfig {
    /// Nuncio account identifier this calendar collection belongs to.
    pub account_id: String,
    /// Fully-qualified URL of the CalDAV calendar collection to query.
    pub caldav_url: String,
    /// Basic-auth username (or app-specific username, per provider).
    pub username: String,
    /// Basic-auth secret (password or app-specific token) resolved from the OS keyring by the
    /// caller -- never stored anywhere else in plaintext. Wrapped in [`Redacted`] so a derived
    /// `Debug`, a `Display`, or `serde` can never disclose it to a log; the raw value is
    /// reachable only through an explicit `expose_secret` call.
    pub auth_token: Redacted<String>,
}

/// Best-effort extraction of the `UID` property from a raw (possibly malformed) VEVENT/
/// VCALENDAR block, used only to correlate a dropped-VEVENT log line with the source event --
/// never returns the summary, attendees, description, or any other calendar content.
fn extract_vevent_uid(raw_ics: &str) -> Option<String> {
    raw_ics.lines().find_map(|line| {
        let line = line.trim();
        let (name, value) = line.split_once(':')?;
        let bare_name = name.split(';').next().unwrap_or(name);
        if bare_name.eq_ignore_ascii_case("UID") {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// CalDAV client protocol engine managing WebDAV REPORT queries against a real server.
pub struct CalDavClient {
    config: CalDavAccountConfig,
    http: reqwest::Client,
}

impl CalDavClient {
    /// Create a new `CalDavClient` bound to a specific CalDAV calendar collection.
    pub fn new(config: CalDavAccountConfig) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("Nuncio-Calendar-CalDAV/1.0")
                .build()
                .unwrap_or_default(),
            config,
        }
    }

    /// Construct a standard CalDAV `<c:calendar-query>` XML payload for a time range window.
    pub fn build_report_query(start_iso: &str, end_iso: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
    <d:prop>
        <d:getetag />
        <c:calendar-data />
    </d:prop>
    <c:filter>
        <c:comp-filter name="VCALENDAR">
            <c:comp-filter name="VEVENT">
                <c:time-range start="{}" end="{}"/>
            </c:comp-filter>
        </c:comp-filter>
    </c:filter>
</c:calendar-query>"#,
            start_iso, end_iso
        )
    }

    /// Parse a CalDAV WebDAV XML `<multistatus>` response containing embedded VEVENT data.
    ///
    /// The document is traversed with a namespace-aware parser and `calendar-data` is matched
    /// on `{urn:ietf:params:xml:ns:caldav}calendar-data`, so the server's choice of prefix (or
    /// of a default namespace) is irrelevant and a body this client cannot read is an error
    /// rather than an empty -- and indistinguishable from genuinely empty -- event list.
    ///
    /// A VEVENT block that fails to parse is dropped from the returned events but never
    /// silently: it is logged at `warn` (with its UID, if extractable, and a short parse-error
    /// reason -- never the raw VEVENT body) so a sync that quietly lost calendar data is
    /// visible in telemetry instead of just showing up as a smaller-than-expected event count.
    /// A `<response>` that carried no readable `calendar-data` at all (a `404` propstat, say)
    /// is logged the same way.
    pub fn parse_multistatus_response(
        &self,
        calendar_id: &str,
        raw_xml: &str,
    ) -> Result<Vec<CalendarEvent>, CalendarError> {
        self.parse_multistatus_response_counted(calendar_id, raw_xml)
            .map(|(events, _dropped)| events)
    }

    /// Same parse as [`Self::parse_multistatus_response`], additionally reporting how many
    /// calendar entries were lost -- unparseable VEVENTs plus unreadable responses -- so
    /// callers can log round-trip counts.
    fn parse_multistatus_response_counted(
        &self,
        calendar_id: &str,
        raw_xml: &str,
    ) -> Result<(Vec<CalendarEvent>, usize), CalendarError> {
        let report = parse_multistatus(raw_xml, CALDAV_NS, "calendar-data")
            .map_err(|e| CalendarError::MalformedResponse(e.to_string()))?;

        let mut dropped = report.skipped.len();
        for skipped in &report.skipped {
            warn!(
                account_id = %self.config.account_id,
                calendar_id = %calendar_id,
                href = %skipped.href.as_deref().unwrap_or("unknown"),
                reason = %skipped.reason,
                "dropped CalDAV response with no readable calendar-data"
            );
        }

        let mut events = Vec::new();
        for (index, entry) in report.values.iter().enumerate() {
            let event_id = format!("caldav-evt-{}", index + 1);
            match IcalParserAdapter::parse_ical(
                &event_id,
                &self.config.account_id,
                calendar_id,
                &entry.value,
            ) {
                Ok(event) => events.push(event),
                Err(err) => {
                    dropped += 1;
                    warn!(
                        account_id = %self.config.account_id,
                        calendar_id = %calendar_id,
                        uid = %extract_vevent_uid(&entry.value).unwrap_or_else(|| "unknown".to_string()),
                        reason = %err,
                        "dropped unparseable VEVENT"
                    );
                }
            }
        }

        Ok((events, dropped))
    }

    /// Format a unix timestamp as the `YYYYMMDDTHHMMSSZ` form RFC 4791's
    /// `<c:time-range>` filter expects.
    fn format_query_timestamp(unix_seconds: i64) -> Result<String, CalendarError> {
        chrono::DateTime::from_timestamp(unix_seconds, 0)
            .map(|dt| dt.format("%Y%m%dT%H%M%SZ").to_string())
            .ok_or_else(|| {
                CalendarError::ParseFailed(format!(
                    "invalid unix timestamp for CalDAV time-range window: {unix_seconds}"
                ))
            })
    }

    /// Issue a live CalDAV `REPORT` calendar-query (RFC 4791 Section 7.8) against
    /// `self.config.caldav_url` and parse the `multistatus` response into domain events.
    ///
    /// This performs a real network request -- no canned or fabricated data is ever
    /// returned. A server or network failure surfaces as [`CalendarError::TransportFailed`];
    /// a malformed TZID inside a returned VEVENT surfaces as
    /// [`CalendarError::UnresolvableTimezone`] via [`IcalParserAdapter::parse_ical`].
    #[instrument(
        name = "caldav_sync",
        skip(self),
        fields(account_id = %self.config.account_id, calendar_id = %calendar_id)
    )]
    pub async fn fetch_remote_events(
        &self,
        calendar_id: &str,
        start_window: i64,
        end_window: i64,
    ) -> Result<Vec<CalendarEvent>, CalendarError> {
        let start_iso = Self::format_query_timestamp(start_window)?;
        let end_iso = Self::format_query_timestamp(end_window)?;
        let body = Self::build_report_query(&start_iso, &end_iso);

        // "REPORT" is a fixed, always-valid HTTP token; `from_bytes` cannot fail for it, but
        // the error is still propagated rather than unwrapped so no code path here can panic.
        let report_method = reqwest::Method::from_bytes(b"REPORT")
            .map_err(|e| CalendarError::TransportFailed(format!("invalid HTTP method: {e}")))?;

        debug!(
            account_id = %self.config.account_id,
            calendar_id = %calendar_id,
            "issuing CalDAV REPORT calendar-query"
        );

        let response = self
            .http
            .request(report_method, &self.config.caldav_url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "1")
            .basic_auth(
                &self.config.username,
                Some(self.config.auth_token.expose_secret()),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| CalendarError::TransportFailed(e.to_string()))?;

        let status = response.status();
        // RFC 4791 REPORT responses are conventionally 207 Multi-Status; some servers reply
        // 200 OK for a single-collection result, so any 2xx/207 status is accepted.
        if status.as_u16() != 207 && !status.is_success() {
            return Err(CalendarError::TransportFailed(format!(
                "CalDAV server returned unexpected status {status}"
            )));
        }

        let raw_xml = response
            .text()
            .await
            .map_err(|e| CalendarError::TransportFailed(e.to_string()))?;

        let (events, dropped) = self.parse_multistatus_response_counted(calendar_id, &raw_xml)?;
        info!(
            account_id = %self.config.account_id,
            calendar_id = %calendar_id,
            fetched = events.len() + dropped,
            parsed = events.len(),
            dropped,
            "completed CalDAV sync"
        );

        Ok(events)
    }
}

#[async_trait]
impl CalendarBackend for CalDavClient {
    async fn fetch_events(
        &self,
        calendar_id: &str,
        start_window: i64,
        end_window: i64,
    ) -> Result<Vec<CalendarEvent>, CalendarError> {
        self.fetch_remote_events(calendar_id, start_window, end_window)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CalDavAccountConfig {
        CalDavAccountConfig {
            account_id: "acct-1".to_string(),
            caldav_url: "https://caldav.example.com/calendars/work/".to_string(),
            username: "jmaes".to_string(),
            auth_token: Redacted::new("app-token-secret".to_string()),
        }
    }

    #[test]
    fn build_report_query_format() {
        let query = CalDavClient::build_report_query("20240101T000000Z", "20240201T000000Z");
        assert!(query.contains("20240101T000000Z"));
        assert!(query.contains("20240201T000000Z"));
        assert!(query.contains("<c:calendar-query"));
    }

    #[test]
    fn parse_multistatus_response_extracts_events() {
        let client = CalDavClient::new(test_config());
        let xml_response = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
            <d:response>
                <d:href>/calendars/user/work/evt1.ics</d:href>
                <d:propstat>
                    <d:prop>
                        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:Product Planning
LOCATION:Boardroom
END:VEVENT
END:VCALENDAR</c:calendar-data>
                    </d:prop>
                </d:propstat>
            </d:response>
        </d:multistatus>"#;

        let events = client
            .parse_multistatus_response("cal-work", xml_response)
            .expect("parse succeeds");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Product Planning");
        assert_eq!(events[0].location, Some("Boardroom".to_string()));
    }

    #[test]
    fn parse_multistatus_response_surfaces_a_warn_for_a_dropped_vevent_instead_of_silence() {
        use crate::test_tracing::with_recorder;

        let client = CalDavClient::new(test_config());
        // A TZID that cannot be resolved to a real IANA zone; the VEVENT fails to parse but
        // the one alongside it (plain UTC) must still come through.
        let xml_response = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
            <d:response>
                <d:propstat>
                    <d:prop>
                        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:evt-unresolvable-tzid
SUMMARY:Confidential Merger Talks
DTSTART;TZID=Not/ARealZone:20240101T090000
DTEND;TZID=Not/ARealZone:20240101T100000
END:VEVENT
END:VCALENDAR</c:calendar-data>
                    </d:prop>
                </d:propstat>
            </d:response>
            <d:response>
                <d:propstat>
                    <d:prop>
                        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:Good Event
DTSTART:20240301T120000Z
DTEND:20240301T130000Z
END:VEVENT
END:VCALENDAR</c:calendar-data>
                    </d:prop>
                </d:propstat>
            </d:response>
        </d:multistatus>"#;

        let (recorder, result) =
            with_recorder(|| client.parse_multistatus_response("cal-work", xml_response));
        let events = result.expect("the unparseable VEVENT is dropped, not fatal");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Good Event");

        let warnings: Vec<_> = recorder
            .events()
            .into_iter()
            .filter(|e| e.level == tracing::Level::WARN)
            .collect();
        assert_eq!(warnings.len(), 1, "exactly one VEVENT should be dropped");
        assert_eq!(
            warnings[0].fields.get("uid").map(String::as_str),
            Some("evt-unresolvable-tzid")
        );
        assert!(warnings[0].message().contains("dropped"));

        // The dropped VEVENT's summary (potential PII) must never appear in telemetry.
        for value in recorder.all_field_values() {
            assert!(!value.contains("Confidential Merger Talks"));
        }
    }

    /// The same document, written the three ways a compliant server may write it. Before
    /// namespace-aware parsing, only the middle one produced any events at all.
    #[test]
    fn parse_multistatus_response_reads_any_prefix_binding_of_the_caldav_namespace() {
        let client = CalDavClient::new(test_config());
        let event_ics = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nSUMMARY:Product Planning\nEND:VEVENT\nEND:VCALENDAR";

        let uppercase = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:CAL="urn:ietf:params:xml:ns:caldav">
    <D:response><D:href>/evt1.ics</D:href><D:propstat><D:prop>
        <CAL:calendar-data>{event_ics}</CAL:calendar-data>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
</D:multistatus>"#
        );
        let default_ns = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
    <response><href>/evt1.ics</href><propstat><prop>
        <c:calendar-data>{event_ics}</c:calendar-data>
    </prop><status>HTTP/1.1 200 OK</status></propstat></response>
</multistatus>"#
        );
        let odd_prefix = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<ns0:multistatus xmlns:ns0="DAV:" xmlns:ns1="urn:ietf:params:xml:ns:caldav">
    <ns0:response><ns0:href>/evt1.ics</ns0:href><ns0:propstat><ns0:prop>
        <ns1:calendar-data>{event_ics}</ns1:calendar-data>
    </ns0:prop><ns0:status>HTTP/1.1 200 OK</ns0:status></ns0:propstat></ns0:response>
</ns0:multistatus>"#
        );

        for (label, xml) in [
            ("uppercase prefixes", &uppercase),
            ("default namespace", &default_ns),
            ("generated prefixes", &odd_prefix),
        ] {
            let events = client
                .parse_multistatus_response("cal-work", xml)
                .unwrap_or_else(|e| panic!("{label} parses: {e}"));
            assert_eq!(events.len(), 1, "{label}");
            assert_eq!(events[0].summary, "Product Planning", "{label}");
        }
    }

    #[test]
    fn parse_multistatus_response_warns_when_a_response_has_no_readable_calendar_data() {
        use crate::test_tracing::with_recorder;

        let client = CalDavClient::new(test_config());
        // The first response's property merely *contains* the searched name; the second
        // returns the real property under a 404 propstat, which is not a value.
        let xml_response = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
            <d:response>
                <d:href>/lookalike.ics</d:href>
                <d:propstat><d:prop>
                    <c:calendar-data-summary>NOT THE PAYLOAD</c:calendar-data-summary>
                </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
            </d:response>
            <d:response>
                <d:href>/gone.ics</d:href>
                <d:propstat><d:prop>
                    <c:calendar-data>BEGIN:VCALENDAR
END:VCALENDAR</c:calendar-data>
                </d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
            </d:response>
        </d:multistatus>"#;

        let (recorder, result) =
            with_recorder(|| client.parse_multistatus_response("cal-work", xml_response));
        let events = result.expect("an unreadable response is not fatal");
        assert!(events.is_empty(), "neither response carries calendar-data");

        let warnings: Vec<_> = recorder
            .events()
            .into_iter()
            .filter(|e| e.level == tracing::Level::WARN)
            .collect();
        assert_eq!(warnings.len(), 2, "both omissions must be visible");
        let hrefs: Vec<_> = warnings
            .iter()
            .filter_map(|w| w.fields.get("href").cloned())
            .collect();
        assert!(hrefs.contains(&"/lookalike.ics".to_string()));
        assert!(hrefs.contains(&"/gone.ics".to_string()));
        assert!(warnings
            .iter()
            .any(|w| w.fields.get("reason").is_some_and(|r| r.contains("404"))));
    }

    #[test]
    fn parse_multistatus_response_rejects_a_body_it_cannot_read_instead_of_returning_empty() {
        let client = CalDavClient::new(test_config());

        // A malformed document: the `<d:response>` element is never closed.
        let truncated = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:"><d:response><d:href>/a.ics</d:href>"#;
        assert!(matches!(
            client.parse_multistatus_response("cal-work", truncated),
            Err(CalendarError::MalformedResponse(_))
        ));

        // A body that is not a multistatus at all (an intercepting proxy's error page).
        let not_dav = "<html><body>502 Bad Gateway</body></html>";
        assert!(matches!(
            client.parse_multistatus_response("cal-work", not_dav),
            Err(CalendarError::MalformedResponse(_))
        ));
    }

    #[test]
    fn format_query_timestamp_produces_ical_utc_form() {
        let formatted =
            CalDavClient::format_query_timestamp(1_704_067_200).expect("valid timestamp");
        assert_eq!(formatted, "20240101T000000Z");
    }

    #[test]
    fn debug_impl_redacts_auth_token() {
        let config = test_config();
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("app-token-secret"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn fetch_remote_events_logs_completion_counts_including_a_drop() {
        use crate::test_tracing::with_recorder;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime");

        let (recorder, result) = with_recorder(|| {
            runtime.block_on(async {
                let mock_server = MockServer::start().await;
                let xml_response = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
            <d:response>
                <d:propstat>
                    <d:prop>
                        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:evt-bad
DTSTART;TZID=Not/ARealZone:20240101T090000
END:VEVENT
END:VCALENDAR</c:calendar-data>
                    </d:prop>
                </d:propstat>
            </d:response>
            <d:response>
                <d:propstat>
                    <d:prop>
                        <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:Good Event
DTSTART:20240301T120000Z
DTEND:20240301T130000Z
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

                let mut config = test_config();
                config.caldav_url = format!("{}/calendars/work/", mock_server.uri());
                let client = CalDavClient::new(config);

                client
                    .fetch_remote_events("cal-work", 1_700_000_000, 1_800_000_000)
                    .await
            })
        });

        let events = result.expect("one good VEVENT still comes through");
        assert_eq!(events.len(), 1);

        let completion = recorder
            .events()
            .into_iter()
            .find(|e| e.message().contains("completed CalDAV sync"))
            .expect("a completion event is logged");
        assert_eq!(
            completion.fields.get("fetched").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            completion.fields.get("parsed").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            completion.fields.get("dropped").map(String::as_str),
            Some("1")
        );
    }
}
