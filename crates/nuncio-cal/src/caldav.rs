//! CalDAV (RFC 4791) / WebDAV REPORT client and XML query generator.

use nuncio_core::model::CalendarEvent;

use crate::parser::{CalendarError, IcalParserAdapter};

/// CalDAV client protocol engine managing WebDAV REPORT queries.
pub struct CalDavClient {
    account_id: String,
}

impl CalDavClient {
    /// Create a new `CalDavClient` bound to an account ID.
    pub fn new(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
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
                    if let Ok(event) =
                        IcalParserAdapter::parse_ical(&event_id, &self.account_id, calendar_id, clean_ics)
                    {
                        events.push(event);
                    }
                }
            }
        }

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_report_query_format() {
        let query = CalDavClient::build_report_query("20240101T000000Z", "20240201T000000Z");
        assert!(query.contains("20240101T000000Z"));
        assert!(query.contains("20240201T000000Z"));
        assert!(query.contains("<c:calendar-query"));
    }

    #[test]
    fn parse_multistatus_response_extracts_events() {
        let client = CalDavClient::new("acct-1");
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
}
