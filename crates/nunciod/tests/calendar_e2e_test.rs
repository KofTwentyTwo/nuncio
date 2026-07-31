//! Full-daemon offline end-to-end test for the calendar vertical.
//!
//! Boots a real `nunciod` daemon gRPC server
//! (`nunciod::grpc::serve_on_listener_with_overrides`) on an ephemeral
//! loopback port, backed by a real temp file-backed [`DatabaseEngine`] and a
//! [`SecretManager::mock`] vault (never the real OS keyring), with an
//! injected [`nuncio_cal::MockCalendarBackend`] standing in for real network
//! I/O (via the [`nunciod::grpc::CalendarEngineOverrides`] injection seam).
//!
//! Driving ONLY the authenticated `nuncio-proto` gRPC client
//! (`connect_calendar`) with the shared bearer token -- never reaching into
//! daemon internals -- this proves:
//!   a. `Calendar/Sync` fetches from the injected mock backend and persists
//!      the result, awaiting full completion before returning, so the
//!      synced events are immediately visible to `ListEvents`/`GetEvent`.
//!   b. An unauthenticated call to `Calendar` is rejected with
//!      `Code::Unauthenticated`, proving it is mounted behind its own
//!      `BearerAuthInterceptor` exactly like every other service.
//!
//! Deterministic and fully offline: no fixed sleeps, no real network, no
//! real OS keyring.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_cal::MockCalendarBackend;
use nuncio_core::model::CalendarEvent;
use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::{CalendarSyncRequest, GetEventRequest, ListEventsRequest};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::{
    serve_on_listener_with_overrides, AccountsEngineOverrides, CalendarEngineOverrides,
    ContactsEngineOverrides, MailEngineOverrides,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tonic::Code;

const ACCOUNT_ID: &str = "acct-cal-e2e-1";
const CALENDAR_ID: &str = "cal-e2e-work";

fn mock_event(id: &str, start: i64, end: i64) -> CalendarEvent {
    CalendarEvent {
        id: id.to_string(),
        account_id: ACCOUNT_ID.to_string(),
        calendar_id: CALENDAR_ID.to_string(),
        summary: format!("E2E Event {id}"),
        start_time: start,
        end_time: end,
        rrule: None,
        location: Some("Conference Room".to_string()),
    }
}

/// Boots the real daemon gRPC server, injecting `calendar_overrides`, on an
/// ephemeral loopback port backed by a real temp file-backed
/// [`DatabaseEngine`]. Returns the server address and the minted bearer
/// token; the `TempDir` guard is returned too so the caller controls its
/// lifetime for the whole test.
async fn boot_daemon(
    secrets: Arc<SecretManager>,
    calendar_overrides: CalendarEngineOverrides,
) -> (std::net::SocketAddr, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join("calendar_e2e.db");
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
            calendar_overrides,
            ContactsEngineOverrides::default(),
            AccountsEngineOverrides::default(),
        )
        .await;
    });

    (addr, token, dir)
}

/// The full offline, deterministic end-to-end proof of the calendar vertical:
/// `Sync` against an injected mock backend, then `ListEvents`/`GetEvent`
/// immediately reflect the persisted result -- ALL over the authenticated
/// `nuncio.v1.Calendar` gRPC API against a live in-process daemon, with
/// protocol I/O fully mocked.
#[tokio::test]
async fn calendar_sync_then_list_and_get_round_trip_over_authenticated_grpc() {
    let secrets = Arc::new(SecretManager::mock());

    let mock_backend = MockCalendarBackend::new();
    mock_backend.add_event(mock_event("evt-e2e-1", 1_700_000_000, 1_700_003_600));
    mock_backend.add_event(mock_event("evt-e2e-2", 1_700_010_000, 1_700_013_600));
    // Outside the sync window this test requests -- must not be synced.
    mock_backend.add_event(mock_event(
        "evt-e2e-out-of-window",
        1_900_000_000,
        1_900_003_600,
    ));

    let calendar_overrides = CalendarEngineOverrides {
        calendar_backend: Some(Arc::new(mock_backend)),
    };

    let (addr, token, _db_dir) = boot_daemon(secrets, calendar_overrides).await;
    let addr_str = addr.to_string();

    let mut calendar_client = nuncio_proto::client::connect_calendar(&addr_str, &token)
        .await
        .expect("calendar client connects");

    // ---- (a) Sync fetches from the injected mock backend and persists ----
    let sync_response = calendar_client
        .sync(CalendarSyncRequest {
            account_id: ACCOUNT_ID.to_string(),
            calendar_id: CALENDAR_ID.to_string(),
            start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_699_999_000)),
            end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_700_020_000)),
        })
        .await
        .expect("sync succeeds")
        .into_inner();
    assert_eq!(sync_response.synced_count, 2);

    // Immediately visible, no fixed sleep: `Sync` only returns once the
    // fetch-and-persist work has fully completed.
    let list_response = calendar_client
        .list_events(ListEventsRequest {
            account_id: ACCOUNT_ID.to_string(),
            calendar_id: CALENDAR_ID.to_string(),
            start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_699_999_000)),
            end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(1_700_020_000)),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect("list_events succeeds")
        .into_inner();
    assert_eq!(list_response.events.len(), 2);
    let synced_ids: Vec<&str> = list_response.events.iter().map(|e| e.id.as_str()).collect();
    assert!(synced_ids.contains(&"evt-e2e-1"));
    assert!(synced_ids.contains(&"evt-e2e-2"));
    assert!(!synced_ids.contains(&"evt-e2e-out-of-window"));

    let get_response = calendar_client
        .get_event(GetEventRequest {
            event_id: "evt-e2e-1".to_string(),
        })
        .await
        .expect("get_event succeeds")
        .into_inner();
    let event = get_response.event.expect("event present in response");
    assert_eq!(event.summary, "E2E Event evt-e2e-1");
    assert_eq!(event.location.as_deref(), Some("Conference Room"));

    // ---- (b) An unauthenticated call is rejected ----
    // Uses the raw generated client (bypassing `connect_calendar`'s
    // bearer-token interceptor) to prove `Calendar` is mounted behind its
    // own `BearerAuthInterceptor`, exactly like every other service.
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
