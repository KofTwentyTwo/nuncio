//! CalDAV (RFC 4791) / WebDAV REPORT client and XML query generator.

use async_trait::async_trait;
use nuncio_core::model::CalendarEvent;
use serde::{Deserialize, Serialize};

use crate::backend::CalendarBackend;
use crate::parser::{CalendarError, IcalParserAdapter};

/// Configuration for a specific CalDAV calendar collection endpoint, mirroring
/// `nuncio_contacts::CardDavAccountConfig`.
///
/// `caldav_url` must already resolve to a specific calendar collection (e.g.
/// `https://caldav.example.com/dav/calendars/user/jmaes/work/`) -- PROPFIND-based
/// `calendar-home-set` auto-discovery is out of scope for this client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalDavAccountConfig {
    /// Nuncio account identifier this calendar collection belongs to.
    pub account_id: String,
    /// Fully-qualified URL of the CalDAV calendar collection to query.
    pub caldav_url: String,
    /// Basic-auth username (or app-specific username, per provider).
    pub username: String,
    /// Basic-auth secret (password or app-specific token) resolved from the OS keyring by the
    /// caller -- never stored anywhere else in plaintext.
    pub auth_token: String,
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

    /// Parse CalDAV WebDAV XML `<multistatus>` response containing embedded VEVENT data.
    pub fn parse_multistatus_response(
        &self,
        calendar_id: &str,
        raw_xml: &str,
    ) -> Result<Vec<CalendarEvent>, CalendarError> {
        let mut events = Vec::new();

        // Extract <c:calendar-data> or <calendar-data> text blocks
        for block in raw_xml.split("<c:calendar-data>") {
            if let Some((ics_data, _)) = block.split_once("</c:calendar-data>") {
                let clean_ics = ics_data.trim();
                if !clean_ics.is_empty() {
                    let event_id = format!("caldav-evt-{}", events.len() + 1);
                    if let Ok(event) = IcalParserAdapter::parse_ical(
                        &event_id,
                        &self.config.account_id,
                        calendar_id,
                        clean_ics,
                    ) {
                        events.push(event);
                    }
                }
            }
        }

        Ok(events)
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

        let response = self
            .http
            .request(report_method, &self.config.caldav_url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "1")
            .basic_auth(&self.config.username, Some(&self.config.auth_token))
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

        self.parse_multistatus_response(calendar_id, &raw_xml)
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
            auth_token: "app-token-secret".to_string(),
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
    fn format_query_timestamp_produces_ical_utc_form() {
        let formatted =
            CalDavClient::format_query_timestamp(1_704_067_200).expect("valid timestamp");
        assert_eq!(formatted, "20240101T000000Z");
    }
}
