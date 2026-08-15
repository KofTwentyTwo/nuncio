//! WireMock integration test suite for the real CardDAV `REPORT` transport.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_contacts::{
    CardDavAccountConfig, CardDavClient, CardDavError, Contact, ContactsBackend,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A canned CardDAV `multistatus` response containing two real vCards, exercising the real
/// HTTP transport together with the vCard parser.
fn multistatus_body() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <d:response>
        <d:href>/addressbooks/contacts/james.vcf</d:href>
        <d:propstat>
            <d:prop>
                <card:address-data>BEGIN:VCARD
VERSION:4.0
FN:James Maes
N:Maes;James;;;
ORG:KofTwentyTwo
TITLE:Founder
EMAIL;TYPE=WORK:james.maes@kof22.com
TEL;TYPE=MOBILE:+1-555-0199
END:VCARD</card:address-data>
            </d:prop>
        </d:propstat>
    </d:response>
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
</d:multistatus>"#
        .to_string()
}

#[tokio::test]
async fn wiremock_carddav_report_fetches_and_parses_vcards() {
    let mock_server = MockServer::start().await;

    Mock::given(method("REPORT"))
        .and(path("/addressbooks/contacts/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(multistatus_body()))
        .mount(&mock_server)
        .await;

    let config = CardDavAccountConfig {
        account_id: "acct-wm-1".to_string(),
        carddav_url: format!("{}/addressbooks/contacts/", mock_server.uri()),
        username: "james.maes".to_string(),
        auth_token: "wiremock-app-token".to_string().into(),
    };
    let client = CardDavClient::new(config);

    // Drive the real client through the `ContactsBackend` trait, exactly how the daemon will
    // invoke either this real client or `MockContactsBackend` behind the same seam.
    let contacts: Vec<_> = ContactsBackend::fetch_contacts(&client, "acct-wm-1")
        .await
        .expect("fetch_contacts succeeds against the wiremock server");

    assert_eq!(contacts.len(), 2);

    let james = contacts
        .iter()
        .find(|c| c.display_name == "James Maes")
        .expect("James Maes contact present");
    assert_eq!(james.account_id.as_deref(), Some("acct-wm-1"));
    assert_eq!(james.given_name.as_deref(), Some("James"));
    assert_eq!(james.family_name.as_deref(), Some("Maes"));
    assert_eq!(james.organization.as_deref(), Some("KofTwentyTwo"));
    assert_eq!(james.job_title.as_deref(), Some("Founder"));
    assert_eq!(james.emails[0].email, "james.maes@kof22.com");
    assert_eq!(james.phones[0].phone, "+1-555-0199");

    let alice = contacts
        .iter()
        .find(|c| c.display_name == "Alice Dev")
        .expect("Alice Dev contact present");
    assert_eq!(alice.emails[0].email, "alice@nuncio.mx");
}

#[tokio::test]
async fn wiremock_carddav_report_server_error_surfaces_as_transport_failure() {
    let mock_server = MockServer::start().await;

    Mock::given(method("REPORT"))
        .and(path("/addressbooks/broken/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let config = CardDavAccountConfig {
        account_id: "acct-wm-2".to_string(),
        carddav_url: format!("{}/addressbooks/broken/", mock_server.uri()),
        username: "james.maes".to_string(),
        auth_token: "wiremock-app-token".to_string().into(),
    };
    let client = CardDavClient::new(config);

    let err = client
        .fetch_remote_vcards("acct-wm-2")
        .await
        .expect_err("a 500 response must be a real error, not fabricated empty success");

    assert!(matches!(err, CardDavError::TransportFailed(_)));
}

#[tokio::test]
async fn wiremock_carddav_report_malformed_vcard_surfaces_as_parse_failure() {
    let mock_server = MockServer::start().await;

    let malformed_body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <d:response>
        <d:href>/addressbooks/malformed/ghost.vcf</d:href>
        <d:propstat>
            <d:prop>
                <card:address-data>BEGIN:VCARD
VERSION:4.0
ORG:No Name Or Email
END:VCARD</card:address-data>
            </d:prop>
        </d:propstat>
    </d:response>
</d:multistatus>"#;

    Mock::given(method("REPORT"))
        .and(path("/addressbooks/malformed/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(malformed_body))
        .mount(&mock_server)
        .await;

    let config = CardDavAccountConfig {
        account_id: "acct-wm-3".to_string(),
        carddav_url: format!("{}/addressbooks/malformed/", mock_server.uri()),
        username: "james.maes".to_string(),
        auth_token: "wiremock-app-token".to_string().into(),
    };
    let client = CardDavClient::new(config);

    let err = client
        .fetch_remote_vcards("acct-wm-3")
        .await
        .expect_err("a vCard with neither FN nor EMAIL must be a real parse error");

    assert!(matches!(err, CardDavError::ParseFailed(_)));
}

/// The same two cards as [`multistatus_body`], serialized the way a server that binds the DAV
/// and CardDAV namespaces to different prefixes would write them. A substring-matching parser
/// reads zero contacts out of this and calls it success; a namespace-aware one reads two.
fn multistatus_body_with_uppercase_prefixes() -> String {
    multistatus_body()
        .replace("<d:", "<D:")
        .replace("</d:", "</D:")
        .replace(r#"xmlns:d="DAV:""#, r#"xmlns:D="DAV:""#)
        .replace("<card:", "<CARD:")
        .replace("</card:", "</CARD:")
        .replace(
            r#"xmlns:card="urn:ietf:params:xml:ns:carddav""#,
            r#"xmlns:CARD="urn:ietf:params:xml:ns:carddav""#,
        )
}

/// The same two cards again, this time with `DAV:` as the document's default namespace, so no
/// DAV element carries a prefix at all.
fn multistatus_body_with_default_namespace() -> String {
    multistatus_body()
        .replace("<d:", "<")
        .replace("</d:", "</")
        .replace(r#"xmlns:d="DAV:""#, r#"xmlns="DAV:""#)
}

/// Serve `body` from a mock CardDAV endpoint and run one real `REPORT` round trip against it.
async fn fetch_against_body(body: String, account_id: &str) -> Result<Vec<Contact>, CardDavError> {
    let mock_server = MockServer::start().await;
    Mock::given(method("REPORT"))
        .and(path("/addressbooks/contacts/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(body))
        .mount(&mock_server)
        .await;

    let config = CardDavAccountConfig {
        account_id: account_id.to_string(),
        carddav_url: format!("{}/addressbooks/contacts/", mock_server.uri()),
        username: "james.maes".to_string(),
        auth_token: "wiremock-app-token".to_string().into(),
    };
    CardDavClient::new(config)
        .fetch_remote_vcards(account_id)
        .await
}

#[tokio::test]
async fn wiremock_carddav_report_reads_the_same_document_under_any_namespace_prefix() {
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
        let contacts = fetch_against_body(body, "acct-wm-ns")
            .await
            .unwrap_or_else(|e| panic!("{label} fetches: {e}"));
        assert_eq!(contacts.len(), 2, "{label}");
        assert!(
            contacts.iter().any(|c| c.display_name == "James Maes"),
            "{label}"
        );
    }
}

#[tokio::test]
async fn wiremock_carddav_report_ignores_a_property_that_merely_contains_the_searched_name() {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <d:response>
        <d:href>/addressbooks/contacts/lookalike.vcf</d:href>
        <d:propstat>
            <d:prop>
                <card:address-data-summary>BEGIN:VCARD
VERSION:4.0
FN:Decoy Contact
END:VCARD</card:address-data-summary>
            </d:prop>
            <d:status>HTTP/1.1 200 OK</d:status>
        </d:propstat>
    </d:response>
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
            <d:status>HTTP/1.1 200 OK</d:status>
        </d:propstat>
    </d:response>
</d:multistatus>"#;

    let contacts = fetch_against_body(body.to_string(), "acct-wm-lookalike")
        .await
        .expect("fetch succeeds");

    assert_eq!(contacts.len(), 1, "the decoy element is not address-data");
    assert_eq!(contacts[0].display_name, "Alice Dev");
}

#[tokio::test]
async fn wiremock_carddav_report_treats_a_404_propstat_as_no_value() {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <d:response>
        <d:href>/addressbooks/contacts/gone.vcf</d:href>
        <d:propstat>
            <d:prop>
                <card:address-data>BEGIN:VCARD
VERSION:4.0
FN:Should Not Appear
END:VCARD</card:address-data>
            </d:prop>
            <d:status>HTTP/1.1 404 Not Found</d:status>
        </d:propstat>
    </d:response>
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
            <d:status>HTTP/1.1 200 OK</d:status>
        </d:propstat>
    </d:response>
</d:multistatus>"#;

    let contacts = fetch_against_body(body.to_string(), "acct-wm-404")
        .await
        .expect("fetch succeeds");

    assert_eq!(contacts.len(), 1, "a 404 propstat carries no value");
    assert_eq!(contacts[0].display_name, "Alice Dev");
}

#[tokio::test]
async fn wiremock_carddav_report_malformed_body_surfaces_as_an_error_not_an_empty_address_book() {
    // Well-formed HTTP, unreadable body: the `<d:response>` element is never closed.
    let truncated = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:"><d:response><d:href>/addressbooks/contacts/a.vcf</d:href>"#;

    let err = fetch_against_body(truncated.to_string(), "acct-wm-malformed")
        .await
        .expect_err("a body this client cannot read must not look like an empty address book");
    assert!(matches!(err, CardDavError::MalformedResponse(_)));

    // A 200 response that is not a multistatus at all (an intercepting captive portal, say).
    let not_dav = "<html><body>Sign in to continue</body></html>";
    let err = fetch_against_body(not_dav.to_string(), "acct-wm-html")
        .await
        .expect_err("a non-multistatus body must not look like an empty address book");
    assert!(matches!(err, CardDavError::MalformedResponse(_)));
}
