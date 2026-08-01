//! CardDAV (RFC 6352) `REPORT` client and multistatus response parser.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument};

use crate::backend::ContactsBackend;
use crate::models::Contact;
use crate::parser::VCardParserAdapter;

/// Errors returned by the CardDAV client engine.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum CardDavError {
    /// Failed to parse an RFC 6350 vCard payload embedded in a CardDAV response.
    #[error("failed to parse vCard payload: {0}")]
    ParseFailed(String),

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
#[derive(Clone, Serialize, Deserialize)]
pub struct CardDavAccountConfig {
    /// Nuncio account identifier this address book collection belongs to.
    pub account_id: String,
    /// Fully-qualified URL of the CardDAV address book collection to query.
    pub carddav_url: String,
    /// Basic-auth username (or app-specific username, per provider).
    pub username: String,
    /// Basic-auth secret (password or app-specific token) resolved from the OS keyring by the
    /// caller -- never stored anywhere else in plaintext.
    pub auth_token: String,
}

impl std::fmt::Debug for CardDavAccountConfig {
    /// Manual `Debug` impl that redacts `auth_token` -- a derived impl would print the raw
    /// secret verbatim the moment anything (a `debug!` log, an assertion failure message,
    /// a panic payload) formats this config with `{:?}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardDavAccountConfig")
            .field("account_id", &self.account_id)
            .field("carddav_url", &self.carddav_url)
            .field("username", &self.username)
            .field("auth_token", &"<redacted>")
            .finish()
    }
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
    pub fn parse_multistatus_response(
        &self,
        account_id: &str,
        raw_xml: &str,
    ) -> Result<Vec<Contact>, CardDavError> {
        let mut contacts = Vec::new();

        for block in raw_xml.split("<card:address-data>") {
            if let Some((vcard_data, _)) = block.split_once("</card:address-data>") {
                let clean_vcard = vcard_data.trim();
                if !clean_vcard.is_empty() {
                    let contact_id = format!("carddav-{account_id}-{}", contacts.len() + 1);
                    let contact =
                        VCardParserAdapter::parse_vcard(&contact_id, account_id, clean_vcard)?;
                    contacts.push(contact);
                }
            }
        }

        Ok(contacts)
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
            .basic_auth(&self.config.username, Some(&self.config.auth_token))
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

        // `parse_multistatus_response` fails fast on the first unparseable vCard (unlike the
        // CalDAV client, which drops individual bad VEVENTs), so a successful return here means
        // every card that was fetched parsed cleanly -- `dropped` is always 0 on this path.
        let contacts = self.parse_multistatus_response(account_id, &raw_xml)?;
        info!(
            account_id = %account_id,
            fetched = contacts.len(),
            parsed = contacts.len(),
            dropped = 0,
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
            auth_token: "app-token-secret".to_string(),
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
