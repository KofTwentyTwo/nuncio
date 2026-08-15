//! Namespace-aware WebDAV `multistatus` response parsing (RFC 4918 Section 13),
//! shared by the CalDAV (RFC 4791) and CardDAV (RFC 6352) clients.
//!
//! Both protocols return the same document shape --
//! `multistatus`/`response`/`propstat`/`prop` -- and differ only in which property
//! carries the payload (`calendar-data` vs `address-data`), so the traversal lives
//! here once rather than being reimplemented per protocol.
//!
//! Elements are matched on their resolved `{namespace}local-name`, never on a prefix
//! and never on a raw substring of the serialized document. `<D:response>`,
//! `<d:response>` and `<response xmlns="DAV:">` are therefore the same element, while
//! an element whose name merely *contains* a searched name (say
//! `<c:calendar-data-summary>`) is correctly a different one.
//!
//! Nothing is dropped in silence. A body that cannot be understood is an error, and a
//! `<response>` that carries no readable value for the requested property is reported
//! in [`MultistatusReport::skipped`] with the reason, so the caller can log the
//! omission instead of quietly returning a short list.

use std::fmt;

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::NsReader;
use thiserror::Error;

/// The WebDAV core namespace (RFC 4918).
pub const DAV_NS: &str = "DAV:";
/// The CalDAV namespace (RFC 4791).
pub const CALDAV_NS: &str = "urn:ietf:params:xml:ns:caldav";
/// The CardDAV namespace (RFC 6352).
pub const CARDDAV_NS: &str = "urn:ietf:params:xml:ns:carddav";

/// Failure to read a WebDAV `multistatus` body at all.
///
/// Both variants are returned instead of an empty result set: a server whose response
/// this parser cannot understand must never look to the caller like a server with
/// nothing to return.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MultistatusError {
    /// The body is not well-formed XML, or it nests markup where the payload property
    /// is defined to hold character data only.
    #[error("malformed WebDAV multistatus XML: {0}")]
    Malformed(String),

    /// The body parsed as XML but its root element is not `{DAV:}multistatus` -- for
    /// example an HTML error page, or a `multistatus` in no namespace at all.
    #[error("response root element is `{0}`, not `{{DAV:}}multistatus`")]
    NotMultistatus(String),
}

/// Why a `<response>` yielded no value for the requested property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The property was named under a `<propstat>` (or the `<response>` itself carried
    /// a status) whose HTTP status was not 2xx, so its content is not a value. A `404`
    /// propstat is the usual case (RFC 4918 Section 13).
    Status(String),
    /// The property was returned under a successful status but held no content.
    EmptyValue,
    /// No `<propstat>` in the response named the requested property at all.
    Absent,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => write!(f, "non-success propstat status: {status}"),
            Self::EmptyValue => write!(f, "property returned empty"),
            Self::Absent => write!(f, "property absent from the response"),
        }
    }
}

/// One readable value of the requested property, with the `<href>` of the resource it
/// came from when the server supplied one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyValue {
    /// The enclosing `<response>`'s `<href>`, if it had one.
    pub href: Option<String>,
    /// The property's character content, trimmed of surrounding whitespace.
    pub value: String,
}

/// A `<response>` that produced no value, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedResponse {
    /// The `<response>`'s `<href>`, if it had one.
    pub href: Option<String>,
    /// Why no value was taken from it.
    pub reason: SkipReason,
}

/// The outcome of reading one `multistatus` body: everything readable, plus an
/// explicit account of everything that was not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MultistatusReport {
    /// Values of the requested property, in document order.
    pub values: Vec<PropertyValue>,
    /// Responses that carried no readable value for the requested property.
    pub skipped: Vec<SkippedResponse>,
}

/// A qualified name resolved to `(namespace, local-name)`. A `None` namespace means the
/// element was in no namespace at all, which never matches a DAV element.
type Qname = (Option<String>, String);

/// Does this resolved name equal `{namespace}local`?
fn matches(name: &Qname, namespace: &str, local: &str) -> bool {
    name.0.as_deref() == Some(namespace) && name.1 == local
}

/// Render a resolved name in Clark notation for diagnostics.
fn render(name: &Qname) -> String {
    match &name.0 {
        Some(ns) => format!("{{{ns}}}{}", name.1),
        None => name.1.clone(),
    }
}

/// `<D:status>` carries a full HTTP status line ("HTTP/1.1 200 OK"). Its content counts
/// as a value only when that line's code is 2xx; a status line with no readable code is
/// treated as a failure rather than optimistically accepted.
fn is_success_status(status_line: &str) -> bool {
    status_line
        .split_whitespace()
        .find_map(|token| token.parse::<u16>().ok())
        .is_some_and(|code| (200..300).contains(&code))
}

/// Which piece of character data the walker is currently accumulating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureKind {
    Href,
    ResponseStatus,
    PropstatStatus,
    Property,
}

/// One `<propstat>` in progress.
#[derive(Debug, Default)]
struct PropstatAcc {
    status: Option<String>,
    values: Vec<String>,
}

/// One `<response>` in progress.
#[derive(Debug, Default)]
struct ResponseAcc {
    href: Option<String>,
    status: Option<String>,
    values: Vec<String>,
    rejected_status: Option<String>,
    saw_empty_value: bool,
}

impl ResponseAcc {
    /// Fold a finished `<propstat>` into this response, honouring its HTTP status.
    fn absorb(&mut self, propstat: PropstatAcc) {
        if propstat.values.is_empty() {
            return;
        }
        // RFC 4918 requires a `<status>`; a server that omits one is given the benefit of
        // the doubt, since only an explicitly non-2xx status makes the content a non-value.
        let succeeded = propstat.status.as_deref().is_none_or(is_success_status);
        if !succeeded {
            // Keep the first rejecting status so the caller can say *why* nothing came back.
            if self.rejected_status.is_none() {
                self.rejected_status = propstat.status;
            }
            return;
        }
        for value in propstat.values {
            if value.is_empty() {
                self.saw_empty_value = true;
            } else {
                self.values.push(value);
            }
        }
    }

    /// Emit this response's values, or a single explanation of why it produced none.
    fn finish(self, report: &mut MultistatusReport) {
        let Self {
            href,
            status,
            values,
            rejected_status,
            saw_empty_value,
        } = self;

        if !values.is_empty() {
            report
                .values
                .extend(values.into_iter().map(|value| PropertyValue {
                    href: href.clone(),
                    value,
                }));
            return;
        }

        let reason = match (rejected_status, saw_empty_value, status) {
            (Some(status), _, _) => SkipReason::Status(status),
            (None, true, _) => SkipReason::EmptyValue,
            (None, false, Some(status)) if !is_success_status(&status) => {
                SkipReason::Status(status)
            }
            _ => SkipReason::Absent,
        };
        report.skipped.push(SkippedResponse { href, reason });
    }
}

/// Streaming state for one pass over a `multistatus` document.
struct Walker<'p> {
    property_ns: &'p str,
    property_local: &'p str,
    stack: Vec<Qname>,
    saw_root: bool,
    response: Option<ResponseAcc>,
    propstat: Option<PropstatAcc>,
    capture: Option<CaptureKind>,
    capture_depth: usize,
    buf: String,
    report: MultistatusReport,
}

impl<'p> Walker<'p> {
    fn new(property_ns: &'p str, property_local: &'p str) -> Self {
        Self {
            property_ns,
            property_local,
            stack: Vec::new(),
            saw_root: false,
            response: None,
            propstat: None,
            capture: None,
            capture_depth: 0,
            buf: String::new(),
            report: MultistatusReport::default(),
        }
    }

    fn begin_capture(&mut self, kind: CaptureKind) {
        self.capture = Some(kind);
        self.capture_depth = self.stack.len() + 1;
        self.buf.clear();
    }

    /// Is the element `levels` above the element about to be opened the given name?
    /// `levels == 1` is its parent.
    fn ancestor_is(&self, levels: usize, namespace: &str, local: &str) -> bool {
        self.stack
            .len()
            .checked_sub(levels)
            .and_then(|index| self.stack.get(index))
            .is_some_and(|name| matches(name, namespace, local))
    }

    fn start(&mut self, name: Qname) -> Result<(), MultistatusError> {
        if !self.saw_root {
            if !matches(&name, DAV_NS, "multistatus") {
                return Err(MultistatusError::NotMultistatus(render(&name)));
            }
            self.saw_root = true;
            self.stack.push(name);
            return Ok(());
        }

        // The payload property is defined as character data only; nested markup would be
        // dropped by a text-only capture, which is exactly the silent loss being avoided.
        if self.capture == Some(CaptureKind::Property) {
            return Err(MultistatusError::Malformed(format!(
                "unexpected element `{}` inside the `{}` property, which holds character data only",
                render(&name),
                self.property_local
            )));
        }

        if self.ancestor_is(1, DAV_NS, "multistatus") && matches(&name, DAV_NS, "response") {
            self.response = Some(ResponseAcc::default());
        } else if self.ancestor_is(1, DAV_NS, "response") {
            if matches(&name, DAV_NS, "href") {
                self.begin_capture(CaptureKind::Href);
            } else if matches(&name, DAV_NS, "status") {
                self.begin_capture(CaptureKind::ResponseStatus);
            } else if matches(&name, DAV_NS, "propstat") {
                self.propstat = Some(PropstatAcc::default());
            }
        } else if self.ancestor_is(1, DAV_NS, "propstat") && matches(&name, DAV_NS, "status") {
            self.begin_capture(CaptureKind::PropstatStatus);
        } else if self.ancestor_is(1, DAV_NS, "prop")
            && self.ancestor_is(2, DAV_NS, "propstat")
            && matches(&name, self.property_ns, self.property_local)
        {
            self.begin_capture(CaptureKind::Property);
        }

        self.stack.push(name);
        Ok(())
    }

    fn end(&mut self, name: &Qname) {
        if let Some(kind) = self.capture {
            if self.stack.len() == self.capture_depth {
                let text = std::mem::take(&mut self.buf);
                let trimmed = text.trim().to_string();
                match kind {
                    CaptureKind::Href => {
                        if let Some(response) = self.response.as_mut() {
                            response.href = Some(trimmed);
                        }
                    }
                    CaptureKind::ResponseStatus => {
                        if let Some(response) = self.response.as_mut() {
                            response.status = Some(trimmed);
                        }
                    }
                    CaptureKind::PropstatStatus => {
                        if let Some(propstat) = self.propstat.as_mut() {
                            propstat.status = Some(trimmed);
                        }
                    }
                    CaptureKind::Property => {
                        if let Some(propstat) = self.propstat.as_mut() {
                            propstat.values.push(trimmed);
                        }
                    }
                }
                self.capture = None;
            }
        }

        self.stack.pop();

        if matches(name, DAV_NS, "propstat") && self.ancestor_is(1, DAV_NS, "response") {
            if let (Some(propstat), Some(response)) = (self.propstat.take(), self.response.as_mut())
            {
                response.absorb(propstat);
            }
        } else if matches(name, DAV_NS, "response") && self.ancestor_is(1, DAV_NS, "multistatus") {
            if let Some(response) = self.response.take() {
                response.finish(&mut self.report);
            }
        }
    }

    fn push_text(&mut self, text: &str) {
        if self.capture.is_some() {
            self.buf.push_str(text);
        }
    }
}

/// Resolve one element's qualified name, refusing an undeclared prefix outright: a
/// prefix with no in-scope binding is not well-formed XML and must not be guessed at.
fn resolved_name(
    resolve: &ResolveResult<'_>,
    element: &BytesStart<'_>,
) -> Result<Qname, MultistatusError> {
    let local = String::from_utf8_lossy(element.local_name().into_inner()).into_owned();
    let namespace = match resolve {
        ResolveResult::Bound(ns) => Some(String::from_utf8_lossy(ns.0).into_owned()),
        ResolveResult::Unbound => None,
        ResolveResult::Unknown(prefix) => {
            return Err(MultistatusError::Malformed(format!(
                "undeclared namespace prefix `{}` on element `{local}`",
                String::from_utf8_lossy(prefix)
            )))
        }
    };
    Ok((namespace, local))
}

/// Read a WebDAV `multistatus` body, extracting every value of
/// `{property_namespace}property_local_name` returned under a successful `<propstat>`.
///
/// Returns [`MultistatusError`] rather than an empty report when the body is not a
/// `multistatus` document this parser can traverse, and records every `<response>` that
/// produced no value in [`MultistatusReport::skipped`] so the caller can surface it.
pub fn parse_multistatus(
    raw_xml: &str,
    property_namespace: &str,
    property_local_name: &str,
) -> Result<MultistatusReport, MultistatusError> {
    let mut reader = NsReader::from_str(raw_xml);
    let mut walker = Walker::new(property_namespace, property_local_name);

    loop {
        let (resolve, event) = reader
            .read_resolved_event()
            .map_err(|e| MultistatusError::Malformed(e.to_string()))?;

        match &event {
            Event::Start(element) => {
                let name = resolved_name(&resolve, element)?;
                walker.start(name)?;
            }
            Event::Empty(element) => {
                let name = resolved_name(&resolve, element)?;
                walker.start(name.clone())?;
                walker.end(&name);
            }
            Event::End(_) => {
                // The reader already checked that this end tag matches its start tag, so the
                // name on the walker's stack is authoritative and the resolver -- whose
                // bindings have by now gone out of scope -- is not consulted again here.
                let name = walker.stack.last().cloned().unwrap_or_default();
                walker.end(&name);
            }
            Event::Text(text) => {
                let decoded = text
                    .decode()
                    .map_err(|e| MultistatusError::Malformed(e.to_string()))?;
                walker.push_text(&decoded);
            }
            Event::CData(cdata) => {
                let decoded = cdata
                    .decode()
                    .map_err(|e| MultistatusError::Malformed(e.to_string()))?;
                walker.push_text(&decoded);
            }
            Event::GeneralRef(reference) => {
                let name = reference
                    .decode()
                    .map_err(|e| MultistatusError::Malformed(e.to_string()))?;
                if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|e| MultistatusError::Malformed(e.to_string()))?
                {
                    walker.push_text(&character.to_string());
                } else if let Some(expansion) = resolve_predefined_entity(&name) {
                    walker.push_text(expansion);
                } else {
                    // Nuncio declares no DTD, so an unknown entity has no expansion; a
                    // best-effort guess would corrupt the payload it appears in.
                    return Err(MultistatusError::Malformed(format!(
                        "unresolvable entity reference `&{name};`"
                    )));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if !walker.saw_root {
        return Err(MultistatusError::NotMultistatus("<none>".to_string()));
    }
    if !walker.stack.is_empty() {
        return Err(MultistatusError::Malformed(format!(
            "document ended with {} element(s) still open",
            walker.stack.len()
        )));
    }

    Ok(walker.report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAL_DATA: &str = "calendar-data";

    fn parse_cal(xml: &str) -> Result<MultistatusReport, MultistatusError> {
        parse_multistatus(xml, CALDAV_NS, CAL_DATA)
    }

    #[test]
    fn matches_the_same_document_under_any_prefix() {
        let lower = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response><d:href>/a.ics</d:href><d:propstat><d:prop>
    <c:calendar-data>ICS-A</c:calendar-data>
  </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"#;
        let upper = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:" xmlns:CAL="urn:ietf:params:xml:ns:caldav">
  <D:response><D:href>/a.ics</D:href><D:propstat><D:prop>
    <CAL:calendar-data>ICS-A</CAL:calendar-data>
  </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
</D:multistatus>"#;
        let defaulted = r#"<?xml version="1.0"?>
<multistatus xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <response><href>/a.ics</href><propstat><prop>
    <c:calendar-data>ICS-A</c:calendar-data>
  </prop><status>HTTP/1.1 200 OK</status></propstat></response>
</multistatus>"#;

        for (label, xml) in [("lower", lower), ("upper", upper), ("default", defaulted)] {
            let report = parse_cal(xml).unwrap_or_else(|e| panic!("{label} parses: {e}"));
            assert_eq!(report.values.len(), 1, "{label}");
            assert_eq!(report.values[0].value, "ICS-A", "{label}");
            assert_eq!(report.values[0].href.as_deref(), Some("/a.ics"), "{label}");
            assert!(report.skipped.is_empty(), "{label}");
        }
    }

    #[test]
    fn a_default_namespace_on_the_payload_property_is_matched_too() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response><d:propstat><d:prop>
    <calendar-data xmlns="urn:ietf:params:xml:ns:caldav">ICS-A</calendar-data>
  </d:prop></d:propstat></d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert_eq!(report.values.len(), 1);
        assert_eq!(report.values[0].value, "ICS-A");
    }

    #[test]
    fn an_element_whose_name_merely_contains_the_searched_name_is_not_the_property() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response><d:href>/a.ics</d:href><d:propstat><d:prop>
    <c:calendar-data-summary>NOT-THE-PAYLOAD</c:calendar-data-summary>
  </d:prop></d:propstat></d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert!(report.values.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].reason, SkipReason::Absent);
    }

    #[test]
    fn the_same_local_name_in_a_foreign_namespace_is_not_the_property() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:x="http://example.invalid/other">
  <d:response><d:propstat><d:prop>
    <x:calendar-data>WRONG-NAMESPACE</x:calendar-data>
  </d:prop></d:propstat></d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert!(report.values.is_empty());
        assert_eq!(report.skipped[0].reason, SkipReason::Absent);
    }

    #[test]
    fn a_non_success_propstat_is_not_a_value() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/gone.ics</d:href>
    <d:propstat>
      <d:prop><c:calendar-data>STALE</c:calendar-data></d:prop>
      <d:status>HTTP/1.1 404 Not Found</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert!(report.values.is_empty(), "a 404 propstat is not a value");
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].href.as_deref(), Some("/gone.ics"));
        assert_eq!(
            report.skipped[0].reason,
            SkipReason::Status("HTTP/1.1 404 Not Found".to_string())
        );
    }

    #[test]
    fn a_success_propstat_alongside_a_404_propstat_still_yields_its_value() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/a.ics</d:href>
    <d:propstat>
      <d:prop><c:calendar-data>ICS-A</c:calendar-data></d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
    <d:propstat>
      <d:prop><d:getcontentlanguage/></d:prop>
      <d:status>HTTP/1.1 404 Not Found</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert_eq!(report.values.len(), 1);
        assert_eq!(report.values[0].value, "ICS-A");
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn a_response_level_failure_status_is_reported_not_dropped() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/missing.ics</d:href>
    <d:status>HTTP/1.1 424 Failed Dependency</d:status>
  </d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert_eq!(
            report.skipped[0].reason,
            SkipReason::Status("HTTP/1.1 424 Failed Dependency".to_string())
        );
    }

    #[test]
    fn an_empty_property_is_reported_rather_than_passed_on_as_a_value() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response><d:href>/blank.ics</d:href><d:propstat><d:prop>
    <c:calendar-data/>
  </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert!(report.values.is_empty());
        assert_eq!(report.skipped[0].reason, SkipReason::EmptyValue);
    }

    #[test]
    fn a_root_that_is_not_a_dav_multistatus_is_an_error_not_an_empty_result() {
        let html = "<html><body>502 Bad Gateway</body></html>";
        assert!(matches!(
            parse_cal(html),
            Err(MultistatusError::NotMultistatus(_))
        ));

        // A `multistatus` in no namespace at all is not the DAV: element.
        let unnamespaced = "<multistatus><response/></multistatus>";
        assert!(matches!(
            parse_cal(unnamespaced),
            Err(MultistatusError::NotMultistatus(_))
        ));

        assert!(matches!(
            parse_cal("   "),
            Err(MultistatusError::NotMultistatus(_))
        ));
    }

    #[test]
    fn malformed_documents_are_errors_not_silent_empties() {
        // Mismatched end tag.
        let mismatched =
            r#"<d:multistatus xmlns:d="DAV:"><d:response></d:respons></d:multistatus>"#;
        assert!(matches!(
            parse_cal(mismatched),
            Err(MultistatusError::Malformed(_))
        ));

        // Truncated mid-document (a connection cut short).
        let truncated = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/a.ics</d:href>"#;
        assert!(matches!(
            parse_cal(truncated),
            Err(MultistatusError::Malformed(_))
        ));

        // Undeclared prefix: the document names a namespace it never bound.
        let undeclared = r#"<d:multistatus xmlns:d="DAV:"><q:response/></d:multistatus>"#;
        assert!(matches!(
            parse_cal(undeclared),
            Err(MultistatusError::Malformed(_))
        ));
    }

    #[test]
    fn markup_nested_inside_the_payload_property_is_an_error() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response><d:propstat><d:prop>
    <c:calendar-data>BEGIN<d:oops/>END</c:calendar-data>
  </d:prop></d:propstat></d:response>
</d:multistatus>"#;
        assert!(matches!(
            parse_cal(xml),
            Err(MultistatusError::Malformed(_))
        ));
    }

    #[test]
    fn entity_references_and_cdata_reach_the_value_intact() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response><d:propstat><d:prop>
    <c:calendar-data>SUMMARY:Tea &amp; Toast &#65;</c:calendar-data>
  </d:prop></d:propstat></d:response>
  <d:response><d:propstat><d:prop>
    <c:calendar-data><![CDATA[SUMMARY:5 < 6]]></c:calendar-data>
  </d:prop></d:propstat></d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert_eq!(report.values[0].value, "SUMMARY:Tea & Toast A");
        assert_eq!(report.values[1].value, "SUMMARY:5 < 6");
    }

    #[test]
    fn an_unresolvable_entity_reference_is_an_error() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response><d:propstat><d:prop>
    <c:calendar-data>&nosuchentity;</c:calendar-data>
  </d:prop></d:propstat></d:response>
</d:multistatus>"#;
        assert!(matches!(
            parse_cal(xml),
            Err(MultistatusError::Malformed(_))
        ));
    }

    #[test]
    fn carddav_address_data_uses_the_same_traversal() {
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:" xmlns:CARD="urn:ietf:params:xml:ns:carddav">
  <D:response><D:href>/alice.vcf</D:href><D:propstat><D:prop>
    <CARD:address-data>BEGIN:VCARD</CARD:address-data>
  </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
</D:multistatus>"#;
        let report = parse_multistatus(xml, CARDDAV_NS, "address-data").expect("parses");
        assert_eq!(report.values.len(), 1);
        assert_eq!(report.values[0].value, "BEGIN:VCARD");
    }

    #[test]
    fn a_property_outside_a_propstat_prop_is_not_taken_as_a_value() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <c:calendar-data>NOT-IN-A-PROPSTAT</c:calendar-data>
  </d:response>
</d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert!(report.values.is_empty());
        assert_eq!(report.skipped[0].reason, SkipReason::Absent);
    }

    #[test]
    fn status_lines_are_classified_by_their_numeric_code() {
        assert!(is_success_status("HTTP/1.1 200 OK"));
        assert!(is_success_status("HTTP/2 207 Multi-Status"));
        assert!(!is_success_status("HTTP/1.1 404 Not Found"));
        assert!(!is_success_status("HTTP/1.1 500 Internal Server Error"));
        assert!(!is_success_status("nonsense with no code"));
    }

    #[test]
    fn skip_reasons_render_for_logging() {
        assert!(SkipReason::Status("HTTP/1.1 404 Not Found".to_string())
            .to_string()
            .contains("404"));
        assert!(!SkipReason::EmptyValue.to_string().is_empty());
        assert!(!SkipReason::Absent.to_string().is_empty());
    }

    #[test]
    fn an_empty_multistatus_yields_an_empty_report_not_an_error() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"></d:multistatus>"#;
        let report = parse_cal(xml).expect("parses");
        assert!(report.values.is_empty());
        assert!(report.skipped.is_empty());
    }
}
