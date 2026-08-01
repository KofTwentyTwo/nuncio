//! WireMock integration test suite for the real CardDAV `REPORT` transport.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_contacts::{CardDavAccountConfig, CardDavClient, CardDavError, ContactsBackend};
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
