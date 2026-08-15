//! CardDAV (RFC 6352) `REPORT` client and multistatus response parser.

use async_trait::async_trait;
use nuncio_core::dav::{parse_multistatus, CARDDAV_NS};
use nuncio_core::redact::Redacted;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

use crate::backend::ContactsBackend;
use crate::models::Contact;
use crate::parser::VCardParserAdapter;

/// Errors returned by the CardDAV client engine.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum CardDavError {
    /// Failed to parse an RFC 6350 vCard payload embedded in a CardDAV response.
    #[error("failed to parse vCard payload: {0}")]
    ParseFailed(String),

    /// The server answered, but its body is not a WebDAV `multistatus` document this client
    /// can traverse. Distinct from an empty result: a response that cannot be read must never
    /// be reported as an address book with no contacts in it.
    #[error("malformed CardDAV multistatus response: {0}")]
    MalformedResponse(String),

    /// CardDAV network/transport-layer failure (connection, TLS, or an unexpected HTTP
    /// status) distinct from a payload that connected fine but failed to parse.
    #[error("CardDAV transport failure: {0}")]
    TransportFailed(String),
}

/// Configuration for a specific CardDAV address book collection endpoint, mirroring
/// `nuncio_cal::CalDavAccountConfig`.
///
/// `carddav_url` must already resolve to a specific address book collection (e.g.
/// `https://carddav.example.com/dav/addressbooks/user/jmaes/contacts/`) -- PROPFIND-based
/// `addressbook-home-set` auto-discovery is out of scope for this client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CardDavAccountConfig {
    /// Nuncio account identifier this address book collection belongs to.
    pub account_id: String,
    /// Fully-qualified URL of the CardDAV address book collection to query.
    pub carddav_url: String,
    /// Basic-auth username (or app-specific username, per provider).
    pub username: String,
    /// Basic-auth secret (password or app-specific token) resolved from the OS keyring by the
    /// caller -- never stored anywhere else in plaintext. Wrapped in [`Redacted`] so a derived
    /// `Debug`, a `Display`, or `serde` can never disclose it to a log; the raw value is
    /// reachable only through an explicit `expose_secret` call.
    pub auth_token: Redacted<String>,
}

/// CardDAV client protocol engine managing address book `REPORT` queries against a real server.
pub struct CardDavClient {
    config: CardDavAccountConfig,
    http: reqwest::Client,
}

impl CardDavClient {
    /// Create a new `CardDavClient` bound to a specific CardDAV address book collection.
    pub fn new(config: CardDavAccountConfig) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("Nuncio-Contacts-CardDAV/1.0")
                .build()
                .unwrap_or_default(),
            config,
        }
    }

    /// Construct a standard CardDAV `<card:addressbook-query>` XML payload (RFC 6352 Section
    /// 8.6) requesting the full `address-data` of every card in the collection.
    pub fn build_report_query() -> String {
        r#"<?xml version="1.0" encoding="utf-8" ?>
<card:addressbook-query xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <d:prop>
        <d:getetag />
        <card:address-data />
    </d:prop>
</card:addressbook-query>"#
            .to_string()
    }

    /// Parse a CardDAV WebDAV XML `<multistatus>` response containing embedded vCard data.
    ///
    /// The document is traversed with a namespace-aware parser and `address-data` is matched on
    /// `{urn:ietf:params:xml:ns:carddav}address-data`, so the server's choice of prefix (or of a
    /// default namespace) is irrelevant and a body this client cannot read is an error rather
    /// than an empty -- and indistinguishable from genuinely empty -- contact list.
    pub fn parse_multistatus_response(
        &self,
        account_id: &str,
        raw_xml: &str,
    ) -> Result<Vec<Contact>, CardDavError> {
        self.parse_multistatus_response_counted(account_id, raw_xml)
            .map(|(contacts, _dropped)| contacts)
    }

    /// Same parse as [`Self::parse_multistatus_response`], additionally reporting how many
    /// `<response>` elements carried no readable `address-data` so callers can log round-trip
    /// counts. Each one is logged at `warn` as it is skipped -- an address book that came back
    /// short must say so rather than look like an address book that is short.
    ///
    /// An unparseable vCard is still fatal (unlike the CalDAV client, which drops individual
    /// bad VEVENTs): a card that the server returned and this client could not read is an
    /// error, not a contact to omit.
    fn parse_multistatus_response_counted(
        &self,
        account_id: &str,
        raw_xml: &str,
    ) -> Result<(Vec<Contact>, usize), CardDavError> {
        let report = parse_multistatus(raw_xml, CARDDAV_NS, "address-data")
            .map_err(|e| CardDavError::MalformedResponse(e.to_string()))?;

        for skipped in &report.skipped {
            warn!(
                account_id = %account_id,
                href = %skipped.href.as_deref().unwrap_or("unknown"),
                reason = %skipped.reason,
                "dropped CardDAV response with no readable address-data"
            );
        }

        let mut contacts = Vec::new();
        for (index, entry) in report.values.iter().enumerate() {
            let contact_id = format!("carddav-{account_id}-{}", index + 1);
            contacts.push(VCardParserAdapter::parse_vcard(
                &contact_id,
                account_id,
                &entry.value,
            )?);
        }

        Ok((contacts, report.skipped.len()))
    }

    /// Issue a live CardDAV `REPORT` addressbook-query (RFC 6352 Section 8.6) against
    /// `self.config.carddav_url` and parse the `multistatus` response into domain contacts.
    ///
    /// This performs a real network request -- no canned or fabricated data is ever
    /// returned. A server or network failure surfaces as [`CardDavError::TransportFailed`]; a
    /// malformed vCard inside a returned response surfaces as [`CardDavError::ParseFailed`].
    #[instrument(name = "carddav_sync", skip(self), fields(account_id = %account_id))]
    pub async fn fetch_remote_vcards(
        &self,
        account_id: &str,
    ) -> Result<Vec<Contact>, CardDavError> {
        let body = Self::build_report_query();

        // "REPORT" is a fixed, always-valid HTTP token; `from_bytes` cannot fail for it, but
        // the error is still propagated rather than unwrapped so no code path here can panic.
        let report_method = reqwest::Method::from_bytes(b"REPORT")
            .map_err(|e| CardDavError::TransportFailed(format!("invalid HTTP method: {e}")))?;

        debug!(account_id = %account_id, "issuing CardDAV REPORT addressbook-query");

        let response = self
            .http
            .request(report_method, &self.config.carddav_url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "1")
            .basic_auth(
                &self.config.username,
                Some(self.config.auth_token.expose_secret()),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| CardDavError::TransportFailed(e.to_string()))?;

        let status = response.status();
        // RFC 6352 REPORT responses are conventionally 207 Multi-Status; some servers reply
        // 200 OK for a single-collection result, so any 2xx/207 status is accepted.
        if status.as_u16() != 207 && !status.is_success() {
            return Err(CardDavError::TransportFailed(format!(
                "CardDAV server returned unexpected status {status}"
            )));
        }

        let raw_xml = response
            .text()
            .await
            .map_err(|e| CardDavError::TransportFailed(e.to_string()))?;

        // The parse fails fast on the first unparseable vCard, so a successful return here means
        // every card that was fetched parsed cleanly; `dropped` therefore counts only the
        // responses the server returned without readable `address-data`.
        let (contacts, dropped) = self.parse_multistatus_response_counted(account_id, &raw_xml)?;
        info!(
            account_id = %account_id,
            fetched = contacts.len() + dropped,
            parsed = contacts.len(),
            dropped,
            "completed CardDAV sync"
        );

        Ok(contacts)
    }
}

#[async_trait]
impl ContactsBackend for CardDavClient {
    async fn fetch_contacts(&self, account_id: &str) -> Result<Vec<Contact>, CardDavError> {
        self.fetch_remote_vcards(account_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CardDavAccountConfig {
        CardDavAccountConfig {
            account_id: "acct-1".to_string(),
            carddav_url: "https://carddav.example.com/addressbooks/contacts/".to_string(),
            username: "jmaes".to_string(),
            auth_token: Redacted::new("app-token-secret".to_string()),
        }
    }

    #[test]
    fn build_report_query_targets_addressbook_query() {
        let query = CardDavClient::build_report_query();
        assert!(query.contains("<card:addressbook-query"));
        assert!(query.contains("<card:address-data"));
    }

    #[test]
    fn parse_multistatus_response_extracts_contacts() {
        let client = CardDavClient::new(test_config());
        let xml_response = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
            <d:response>
                <d:href>/addressbooks/contacts/alice.vcf</d:href>
                <d:propstat>
                    <d:prop>
                        <card:address-data>BEGIN:VCARD
VERSION:4.0
FN:Alice Dev
EMAIL:alice@nuncio.mx
END:VCARD</card:address-data>
                    </d:prop>
                </d:propstat>
            </d:response>
        </d:multistatus>"#;

        let contacts = client
            .parse_multistatus_response("acct-1", xml_response)
            .expect("parse succeeds");

        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].display_name, "Alice Dev");
        assert_eq!(contacts[0].emails[0].email, "alice@nuncio.mx");
        assert_eq!(contacts[0].account_id.as_deref(), Some("acct-1"));
    }

    #[test]
    fn parse_multistatus_response_with_no_cards_yields_empty_not_fabricated() {
        let client = CardDavClient::new(test_config());
        let xml_response = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav"></d:multistatus>"#;

        let contacts = client
            .parse_multistatus_response("acct-1", xml_response)
            .expect("parse succeeds");
        assert!(contacts.is_empty());
    }

    /// The same document, written the three ways a compliant server may write it. Before
    /// namespace-aware parsing, only the middle one produced any contacts at all.
    #[test]
    fn parse_multistatus_response_reads_any_prefix_binding_of_the_carddav_namespace() {
        let client = CardDavClient::new(test_config());
        let card = "BEGIN:VCARD\nVERSION:4.0\nFN:Alice Dev\nEMAIL:alice@nuncio.mx\nEND:VCARD";

        let uppercase = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:CARD="urn:ietf:params:xml:ns:carddav">
    <D:response><D:href>/alice.vcf</D:href><D:propstat><D:prop>
        <CARD:address-data>{card}</CARD:address-data>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
</D:multistatus>"#
        );
        let default_ns = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <response><href>/alice.vcf</href><propstat><prop>
        <card:address-data>{card}</card:address-data>
    </prop><status>HTTP/1.1 200 OK</status></propstat></response>
</multistatus>"#
        );
        let odd_prefix = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<ns0:multistatus xmlns:ns0="DAV:" xmlns:ns1="urn:ietf:params:xml:ns:carddav">
    <ns0:response><ns0:href>/alice.vcf</ns0:href><ns0:propstat><ns0:prop>
        <ns1:address-data>{card}</ns1:address-data>
    </ns0:prop><ns0:status>HTTP/1.1 200 OK</ns0:status></ns0:propstat></ns0:response>
</ns0:multistatus>"#
        );

        for (label, xml) in [
            ("uppercase prefixes", &uppercase),
            ("default namespace", &default_ns),
            ("generated prefixes", &odd_prefix),
        ] {
            let contacts = client
                .parse_multistatus_response("acct-1", xml)
                .unwrap_or_else(|e| panic!("{label} parses: {e}"));
            assert_eq!(contacts.len(), 1, "{label}");
            assert_eq!(contacts[0].display_name, "Alice Dev", "{label}");
        }
    }

    #[test]
    fn parse_multistatus_response_warns_when_a_response_has_no_readable_address_data() {
        use crate::test_tracing::with_recorder;

        let client = CardDavClient::new(test_config());
        // The first response's property merely *contains* the searched name; the second
        // returns the real property under a 404 propstat, which is not a value.
        let xml_response = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
            <d:response>
                <d:href>/lookalike.vcf</d:href>
                <d:propstat><d:prop>
                    <card:address-data-summary>NOT THE PAYLOAD</card:address-data-summary>
                </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
            </d:response>
            <d:response>
                <d:href>/gone.vcf</d:href>
                <d:propstat><d:prop>
                    <card:address-data>BEGIN:VCARD
END:VCARD</card:address-data>
                </d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
            </d:response>
        </d:multistatus>"#;

        let (recorder, result) =
            with_recorder(|| client.parse_multistatus_response("acct-1", xml_response));
        let contacts = result.expect("an unreadable response is not fatal");
        assert!(contacts.is_empty(), "neither response carries address-data");

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
        assert!(hrefs.contains(&"/lookalike.vcf".to_string()));
        assert!(hrefs.contains(&"/gone.vcf".to_string()));
        assert!(warnings
            .iter()
            .any(|w| w.fields.get("reason").is_some_and(|r| r.contains("404"))));
    }

    #[test]
    fn parse_multistatus_response_rejects_a_body_it_cannot_read_instead_of_returning_empty() {
        let client = CardDavClient::new(test_config());

        // A malformed document: the `<d:response>` element is never closed.
        let truncated = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:"><d:response><d:href>/a.vcf</d:href>"#;
        assert!(matches!(
            client.parse_multistatus_response("acct-1", truncated),
            Err(CardDavError::MalformedResponse(_))
        ));

        // A body that is not a multistatus at all (an intercepting proxy's error page).
        let not_dav = "<html><body>502 Bad Gateway</body></html>";
        assert!(matches!(
            client.parse_multistatus_response("acct-1", not_dav),
            Err(CardDavError::MalformedResponse(_))
        ));
    }

    #[test]
    fn debug_impl_redacts_auth_token() {
        let config = test_config();
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("app-token-secret"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn fetch_remote_vcards_logs_completion_counts() {
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
        <d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
            <d:response>
                <d:propstat>
                    <d:prop>
                        <card:address-data>BEGIN:VCARD
VERSION:4.0
FN:Alice Dev
EMAIL:alice@nuncio.mx
END:VCARD</card:address-data>
                    </d:prop>
                </d:propstat>
            </d:response>
        </d:multistatus>"#;

                Mock::given(method("REPORT"))
                    .and(path("/addressbooks/contacts/"))
                    .respond_with(ResponseTemplate::new(207).set_body_string(xml_response))
                    .mount(&mock_server)
                    .await;

                let mut config = test_config();
                config.carddav_url = format!("{}/addressbooks/contacts/", mock_server.uri());
                let client = CardDavClient::new(config);

                client.fetch_remote_vcards("acct-1").await
            })
        });

        let contacts = result.expect("fetch succeeds against the wiremock server");
        assert_eq!(contacts.len(), 1);

        let completion = recorder
            .events()
            .into_iter()
            .find(|e| e.message().contains("completed CardDAV sync"))
            .expect("a completion event is logged");
        assert_eq!(
            completion.fields.get("fetched").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            completion.fields.get("parsed").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            completion.fields.get("dropped").map(String::as_str),
            Some("0")
        );

        // vCard PII must never appear in telemetry.
        for value in recorder.all_field_values() {
            assert!(!value.contains("Alice Dev"));
            assert!(!value.contains("alice@nuncio.mx"));
        }
    }
}
