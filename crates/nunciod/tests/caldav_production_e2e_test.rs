//! Full-daemon offline end-to-end test for the PRODUCTION CalDAV sync path.
//!
//! Unlike `calendar_e2e_test.rs`, which injects a `MockCalendarBackend` through
//! the [`nunciod::grpc::CalendarEngineOverrides`] seam, this test drives the
//! real production path with NO injected backend: it configures a first-class
//! CalDAV account over the authenticated `Accounts/AddAccount` RPC (protocol
//! CALDAV + a `collection_url` pointing at a WireMock server + a keyring
//! credential), then calls `Calendar/Sync`. The daemon resolves the configured
//! CalDAV account, reads its credential from the (mock) vault, builds a REAL
//! [`nuncio_cal::CalDavClient`] from the persisted collection URL, issues a
//! genuine CalDAV `REPORT` against WireMock, and persists the fetched events.
//!
//! This proves the production wiring end to end while remaining fully offline:
//! WireMock stands in for the remote CalDAV server, and a
//! [`SecretManager::mock`] vault stands in for the OS keyring -- no real
//! network and no real keyring are ever touched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::{
    AccountConfig as AccountConfigProto, AccountProtocol as AccountProtocolProto,
    AddAccountRequest, CalendarSyncRequest, ListEventsRequest, TlsMode as TlsModeProto,
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

const ACCOUNT_ID: &str = "acct-caldav-e2e-1";
const ACCOUNT_EMAIL: &str = "james.maes@nuncio.mx";
const KEYRING_KEY: &str = "nuncio/acct-caldav-e2e-1";
const CALDAV_PASSWORD: &str = "wiremock-app-token";
const CALENDAR_ID: &str = "cal-work";

/// A canned CalDAV `multistatus` response with two VEVENTs, mirroring the
/// transport-level fixture used by `nuncio-cal`'s own WireMock test.
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
SUMMARY:Production CalDAV Standup
LOCATION:Conference Room B
DTSTART:20240715T130000Z
DTEND:20240715T140000Z
END:VEVENT
END:VCALENDAR</c:calendar-data>
            </d:prop>
        </d:propstat>
    </d:response>
    <d:response>
        <d:href>/calendars/work/review.ics</d:href>
        <d:propstat>
            <d:prop>
                <c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:Production UTC Review
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

/// Boots the real daemon gRPC server with DEFAULT overrides (no injected
/// calendar backend), backed by a real temp file-backed [`DatabaseEngine`] and
/// the given mock vault. Returns the server address and the minted bearer
/// token; the `TempDir` guard is returned so the caller controls its lifetime.
async fn boot_daemon(
    secrets: Arc<SecretManager>,
) -> (std::net::SocketAddr, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join("caldav_prod_e2e.db");
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
            // No injected calendar backend: exercise the real production path.
            CalendarEngineOverrides::default(),
            ContactsEngineOverrides::default(),
            AccountsEngineOverrides::default(),
        )
        .await;
    });

    (addr, token, dir)
}

/// The full offline proof of the production CalDAV sync path: configure a
/// CalDAV account over the real `Accounts` API, then `Calendar/Sync` builds a
/// real `CalDavClient` and syncs events fetched from WireMock into the store,
/// immediately visible to `ListEvents` -- all over the authenticated gRPC API.
#[tokio::test]
async fn caldav_production_sync_builds_real_client_and_persists_events() {
    let mock_server = MockServer::start().await;
    Mock::given(method("REPORT"))
        .and(path("/calendars/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(multistatus_body()))
        .mount(&mock_server)
        .await;
    let collection_url = format!("{}/calendars/work/", mock_server.uri());

    let secrets = Arc::new(SecretManager::mock());
    let (addr, token, _db_dir) = boot_daemon(secrets).await;
    let addr_str = addr.to_string();

    // ---- Configure a first-class CalDAV account over the Accounts API ----
    let mut accounts_client = nuncio_proto::client::connect_accounts(&addr_str, &token)
        .await
        .expect("accounts client connects");
    accounts_client
        .add_account(AddAccountRequest {
            config: Some(AccountConfigProto {
                id: ACCOUNT_ID.to_string(),
                name: "Work Calendar".to_string(),
                email_address: ACCOUNT_EMAIL.to_string(),
                protocol: AccountProtocolProto::Caldav.into(),
                server_host: String::new(),
                server_port: 0,
                imap_tls_mode: TlsModeProto::ImplicitTls.into(),
                smtp_tls_mode: TlsModeProto::ImplicitTls.into(),
                keyring_secret_key: KEYRING_KEY.to_string(),
                sync_interval: Some(nuncio_proto::time::duration_from_secs(300)),
                smtp_host: String::new(),
                smtp_port: 0,
                collection_url: collection_url.clone(),
            }),
            password: CALDAV_PASSWORD.to_string(),
        })
        .await
        .expect("CalDAV account is accepted and persisted");

    // ---- Calendar/Sync runs the PRODUCTION path (no injected backend) ----
    let mut calendar_client = nuncio_proto::client::connect_calendar(&addr_str, &token)
        .await
        .expect("calendar client connects");
    let sync_response = calendar_client
        .sync(CalendarSyncRequest {
            account_id: ACCOUNT_ID.to_string(),
            calendar_id: CALENDAR_ID.to_string(),
            start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_700_000_000)),
            end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_800_000_000)),
        })
        .await
        .expect("production CalDAV sync succeeds")
        .into_inner();
    assert_eq!(sync_response.synced_count, 2);

    // Immediately visible: `Sync` awaits full fetch-and-persist completion.
    let list_response = calendar_client
        .list_events(ListEventsRequest {
            account_id: ACCOUNT_ID.to_string(),
            calendar_id: CALENDAR_ID.to_string(),
            start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_700_000_000)),
            end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_800_000_000)),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect("list_events succeeds")
        .into_inner();
    assert_eq!(list_response.events.len(), 2);
    let summaries: Vec<&str> = list_response
        .events
        .iter()
        .map(|e| e.summary.as_str())
        .collect();
    assert!(summaries.contains(&"Production CalDAV Standup"));
    assert!(summaries.contains(&"Production UTC Review"));

    // ---- With no CalDAV account matching, the honest error still fires ----
    let missing = calendar_client
        .sync(CalendarSyncRequest {
            account_id: "acct-does-not-exist".to_string(),
            calendar_id: CALENDAR_ID.to_string(),
            start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_700_000_000)),
            end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_800_000_000)),
        })
        .await
        .expect_err("a sync targeting an unconfigured account must fail honestly");
    assert_eq!(missing.code(), Code::FailedPrecondition);

    // ---- An unauthenticated call to Calendar is rejected ----
    let mut unauthenticated_client =
        nuncio_proto::v1::calendar_client::CalendarClient::connect(format!("http://{addr_str}"))
            .await
            .expect("client connects at the transport level without any auth");
    let err = unauthenticated_client
        .list_events(ListEventsRequest {
            account_id: ACCOUNT_ID.to_string(),
            calendar_id: CALENDAR_ID.to_string(),
            start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(0)),
            end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(i64::MAX)),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect_err("missing bearer token must be rejected");
    assert_eq!(err.code(), Code::Unauthenticated);
}
