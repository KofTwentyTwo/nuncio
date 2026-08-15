//! Full-daemon offline end-to-end test for the PRODUCTION CardDAV sync path.
//!
//! Unlike `contacts_e2e_test.rs`, which injects a `MockContactsBackend` through
//! the [`nunciod::grpc::ContactsEngineOverrides`] seam, this test drives the
//! real production path with NO injected backend: it configures a first-class
//! CardDAV account over the authenticated `Accounts/AddAccount` RPC (protocol
//! CARDDAV + a `collection_url` pointing at a WireMock server + a keyring
//! credential), then calls `Contacts/Sync`. The daemon resolves the configured
//! CardDAV account, reads its credential from the (mock) vault, builds a REAL
//! [`nuncio_contacts::CardDavClient`] from the persisted collection URL, issues
//! a genuine CardDAV `REPORT` against WireMock, and persists the fetched
//! contacts.
//!
//! This proves the production wiring end to end while remaining fully offline:
//! WireMock stands in for the remote CardDAV server, and a
//! [`SecretManager::mock`] vault stands in for the OS keyring -- no real
//! network and no real keyring are ever touched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::{
    AccountConfig as AccountConfigProto, AddAccountRequest, ContactsSyncRequest,
    ListContactsRequest,
};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::{
    serve_on_listener_with_overrides, AccountsEngineOverrides, CalendarEngineOverrides,
    ContactsEngineOverrides, MailEngineOverrides,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tonic::Code;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCOUNT_ID: &str = "acct-carddav-e2e-1";
const ACCOUNT_EMAIL: &str = "james.maes@nuncio.mx";
const KEYRING_KEY: &str = "nuncio/acct-carddav-e2e-1";
const CARDDAV_PASSWORD: &str = "wiremock-app-token";

/// A mail account added alongside the CardDAV one, to prove `Contacts/Sync`
/// refuses an account that is not a CardDAV account rather than silently
/// syncing nothing.
const MAIL_ACCOUNT_ID: &str = "acct-carddav-e2e-mail";

/// A canned CardDAV `multistatus` response with two vCards, mirroring the
/// transport-level fixture used by `nuncio-contacts`'s own WireMock test.
fn multistatus_body() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <d:response>
        <d:href>/addressbooks/contacts/james.vcf</d:href>
        <d:propstat>
            <d:prop>
                <card:address-data>BEGIN:VCARD
VERSION:4.0
FN:Production CardDAV Maes
N:Maes;James;;;
ORG:KofTwentyTwo
TITLE:Founder
EMAIL;TYPE=WORK:james.maes@kof22.com
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
FN:Production CardDAV Dev
EMAIL:alice@nuncio.mx
END:VCARD</card:address-data>
            </d:prop>
        </d:propstat>
    </d:response>
</d:multistatus>"#
        .to_string()
}

/// Boots the real daemon gRPC server with DEFAULT overrides (no injected
/// contacts backend), backed by a real temp file-backed [`DatabaseEngine`] and
/// the given mock vault. Returns the server address and the minted bearer
/// token; the `TempDir` guard is returned so the caller controls its lifetime.
async fn boot_daemon(
    secrets: Arc<SecretManager>,
) -> (std::net::SocketAddr, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join("carddav_prod_e2e.db");
    let db = Arc::new(
        DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .expect("open temp file-backed database"),
    );

    let token_bytes = secrets
        .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
        .expect("mint gRPC bearer token from mock vault");
    let token = hex::encode(token_bytes);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    let event_bus = Arc::new(EventBus::new());
    let server_secrets = secrets.clone();
    let server_token = token.clone();
    let server_filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    tokio::spawn(async move {
        let _ = serve_on_listener_with_overrides(
            listener,
            event_bus,
            db,
            server_filter_engine,
            server_secrets,
            server_token,
            MailEngineOverrides::default(),
            CalendarEngineOverrides::default(),
            // No injected contacts backend: exercise the real production path.
            ContactsEngineOverrides::default(),
            AccountsEngineOverrides::default(),
        )
        .await;
    });

    (addr, token, dir)
}

/// The full offline proof of the production CardDAV sync path: configure a
/// CardDAV account over the real `Accounts` API, then `Contacts/Sync` builds a
/// real `CardDavClient` and syncs contacts fetched from WireMock into the
/// store, immediately visible to `ListContacts` -- all over the authenticated
/// gRPC API.
#[tokio::test]
async fn carddav_production_sync_builds_real_client_and_persists_contacts() {
    let mock_server = MockServer::start().await;
    Mock::given(method("REPORT"))
        .and(path("/addressbooks/contacts/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(multistatus_body()))
        .mount(&mock_server)
        .await;
    let collection_url = format!("{}/addressbooks/contacts/", mock_server.uri());

    let secrets = Arc::new(SecretManager::mock());
    let (addr, token, _db_dir) = boot_daemon(secrets).await;
    let addr_str = addr.to_string();

    // ---- Configure a first-class CardDAV account over the Accounts API ----
    // The password travels to the daemon ONLY on this RPC and is written only
    // to the mock vault; the sync path below reads it back from there.
    let mut accounts_client = nuncio_proto::client::connect_accounts(&addr_str, &token)
        .await
        .expect("accounts client connects");
    accounts_client
        .add_account(AddAccountRequest {
            config: Some(AccountConfigProto {
                id: ACCOUNT_ID.to_string(),
                name: "Work Address Book".to_string(),
                email_address: ACCOUNT_EMAIL.to_string(),
                keyring_secret_key: KEYRING_KEY.to_string(),
                sync_interval: Some(nuncio_proto::time::duration_from_secs(300)),
                filters_enabled: false,
                transport: Some(nuncio_proto::v1::account_config::Transport::Carddav(
                    nuncio_proto::v1::DavTransport {
                        collection_url: collection_url.clone(),
                    },
                )),
            }),
            password: CARDDAV_PASSWORD.to_string(),
        })
        .await
        .expect("CardDAV account is accepted and persisted");

    // A non-CardDAV account, so the "not a CardDAV account" assertion below
    // exercises a real, configured account rather than a missing one.
    accounts_client
        .add_account(AddAccountRequest {
            config: Some(AccountConfigProto {
                id: MAIL_ACCOUNT_ID.to_string(),
                name: "Work Mail".to_string(),
                email_address: ACCOUNT_EMAIL.to_string(),
                keyring_secret_key: "nuncio/acct-carddav-e2e-mail".to_string(),
                sync_interval: Some(nuncio_proto::time::duration_from_secs(300)),
                filters_enabled: false,
                transport: Some(nuncio_proto::v1::account_config::Transport::ImapSmtp(
                    nuncio_proto::v1::ImapSmtpTransport {
                        imap_host: "127.0.0.1".to_string(),
                        imap_port: 1143,
                        imap_tls_mode: nuncio_proto::v1::TlsMode::Plain.into(),
                        smtp_host: "127.0.0.1".to_string(),
                        smtp_port: 1025,
                        smtp_tls_mode: nuncio_proto::v1::TlsMode::Plain.into(),
                    },
                )),
            }),
            password: "mail-password".to_string(),
        })
        .await
        .expect("mail account is accepted and persisted");

    // ---- Contacts/Sync runs the PRODUCTION path (no injected backend) ----
    let mut contacts_client = nuncio_proto::client::connect_contacts(&addr_str, &token)
        .await
        .expect("contacts client connects");
    let sync_response = contacts_client
        .sync(ContactsSyncRequest {
            account_id: ACCOUNT_ID.to_string(),
        })
        .await
        .expect("production CardDAV sync succeeds")
        .into_inner();
    assert_eq!(sync_response.synced_count, 2);

    // Immediately visible: `Sync` awaits full fetch-and-persist completion.
    let list_response = contacts_client
        .list_contacts(ListContactsRequest {
            account_id: ACCOUNT_ID.to_string(),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect("list_contacts succeeds")
        .into_inner();
    assert_eq!(list_response.contacts.len(), 2);
    let names: Vec<&str> = list_response
        .contacts
        .iter()
        .map(|c| c.display_name.as_str())
        .collect();
    assert!(names.contains(&"Production CardDAV Maes"));
    assert!(names.contains(&"Production CardDAV Dev"));

    // ---- A configured account that is NOT CardDAV fails honestly ----
    let not_carddav = contacts_client
        .sync(ContactsSyncRequest {
            account_id: MAIL_ACCOUNT_ID.to_string(),
        })
        .await
        .expect_err("a sync targeting a non-CardDAV account must fail honestly");
    assert_eq!(not_carddav.code(), Code::FailedPrecondition);
    assert!(not_carddav.message().contains(MAIL_ACCOUNT_ID));

    // ---- An account with no CardDAV configuration at all fails honestly ----
    let missing = contacts_client
        .sync(ContactsSyncRequest {
            account_id: "acct-does-not-exist".to_string(),
        })
        .await
        .expect_err("a sync targeting an unconfigured account must fail honestly");
    assert_eq!(missing.code(), Code::FailedPrecondition);

    // ---- An unauthenticated call to Contacts is rejected ----
    let mut unauthenticated_client =
        nuncio_proto::v1::contacts_client::ContactsClient::connect(format!("http://{addr_str}"))
            .await
            .expect("client connects at the transport level without any auth");
    let err = unauthenticated_client
        .list_contacts(ListContactsRequest {
            account_id: ACCOUNT_ID.to_string(),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect_err("missing bearer token must be rejected");
    assert_eq!(err.code(), Code::Unauthenticated);
}
