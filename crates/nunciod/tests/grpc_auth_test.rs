//! Integration test for the `nuncio.v1.System` gRPC server wired into
//! `nunciod`.
//!
//! Starts a real `nunciod::grpc` server on an ephemeral loopback port and
//! drives it with the generated `nuncio-proto` tonic client to prove:
//!   (a) calls with no/invalid bearer token are rejected with `Unauthenticated`;
//!   (b) calls with the correct bearer token succeed and return the daemon's
//!       real, live engine status (not a stub).
//!
//! The bearer token is minted through `SecretManager::mock()` (never the
//! real OS keyring), exactly the way `nunciod`'s boot sequence mints it
//! through `SecretManager::production()` in `main.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::{CoreCommand, EngineStatus, EventBus};
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::system_client::SystemClient;
use nuncio_proto::v1::GetStatusRequest;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::serve_on_listener;
use std::sync::Arc;
use tokio::net::TcpListener;
use tonic::{Code, Request};

/// Binds an ephemeral loopback port and spawns the real gRPC server on it.
/// Because `TcpListener::bind` has already succeeded (the OS is accepting
/// connections on the returned port) before this function returns, callers
/// can dial `addr` immediately with no fixed sleep.
///
/// Backs the mounted `nuncio.v1.Accounts` service with its own fresh
/// ephemeral database + mock keyring, since this test file only exercises
/// `System` and doesn't care about account/keyring state.
///
/// Also returns the ephemeral database's backing `TempDir` guard: the
/// spawned server task keeps using `db` long after this function returns,
/// and on Linux dropping the guard here would unlink the directory out from
/// under it, so any connection the pool opens afterward fails with "unable
/// to open database file". Callers must hold the guard for as long as they
/// keep talking to the server.
async fn start_server(
    event_bus: Arc<EventBus>,
    token: String,
) -> (std::net::SocketAddr, tempfile::TempDir) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");
    let (db, dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let secrets = Arc::new(SecretManager::mock());
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    tokio::spawn(async move {
        let _ = serve_on_listener(
            listener,
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            token,
        )
        .await;
    });
    (addr, dir)
}

#[tokio::test]
async fn rejects_calls_without_a_bearer_token() {
    let event_bus = Arc::new(EventBus::new());
    let secrets = SecretManager::mock();
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );

    let (addr, _dir) = start_server(event_bus, token).await;
    let mut client = SystemClient::connect(format!("http://{addr}"))
        .await
        .expect("client connects");

    let err = client
        .get_status(GetStatusRequest {})
        .await
        .expect_err("call without any authorization header must be rejected");
    assert_eq!(err.code(), Code::Unauthenticated);
}

#[tokio::test]
async fn rejects_calls_with_an_invalid_bearer_token() {
    let event_bus = Arc::new(EventBus::new());
    let secrets = SecretManager::mock();
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );

    let (addr, _dir) = start_server(event_bus, token).await;
    let mut client = SystemClient::connect(format!("http://{addr}"))
        .await
        .expect("client connects");

    let mut request = Request::new(GetStatusRequest {});
    request.metadata_mut().insert(
        "authorization",
        "Bearer definitely-the-wrong-token"
            .parse()
            .expect("valid ascii metadata value"),
    );
    let err = client
        .get_status(request)
        .await
        .expect_err("call with an invalid bearer token must be rejected");
    assert_eq!(err.code(), Code::Unauthenticated);
}

#[tokio::test]
async fn accepts_correct_bearer_token_and_returns_real_engine_status() {
    let event_bus = Arc::new(EventBus::new());

    // Drive real daemon state via the same `EventBus` the gRPC server reads
    // from, proving `GetStatus` reflects live state rather than a stub.
    event_bus.process_command(CoreCommand::SyncAll);
    assert_eq!(event_bus.current_state().status, EngineStatus::Syncing);

    let secrets = SecretManager::mock();
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );

    let (addr, _dir) = start_server(event_bus.clone(), token.clone()).await;
    let mut client = SystemClient::connect(format!("http://{addr}"))
        .await
        .expect("client connects");

    let mut request = Request::new(GetStatusRequest {});
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid ascii metadata value"),
    );
    let response = client
        .get_status(request)
        .await
        .expect("call with the correct bearer token must succeed")
        .into_inner();

    assert_eq!(response.engine_status, "Syncing");
    assert_eq!(response.version, env!("CARGO_PKG_VERSION"));
}

/// Confirms `Filters` newly added management RPCs are mounted behind the
/// SAME `BearerAuthInterceptor` as every other service -- an unauthenticated
/// call to `UpdateRule` must be rejected exactly like an unauthenticated
/// call to `System/GetStatus` above, with no carve-out for "just one more"
/// RPC.
#[tokio::test]
async fn rejects_calls_to_new_filters_rpcs_without_a_bearer_token() {
    let event_bus = Arc::new(EventBus::new());
    let secrets = SecretManager::mock();
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );

    let (addr, _dir) = start_server(event_bus, token).await;
    let mut client =
        nuncio_proto::v1::filters_client::FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

    let err = client
        .update_rule(nuncio_proto::v1::UpdateRuleRequest {
            id: "rule-1".to_string(),
            name: None,
            nsql: None,
            priority: None,
        })
        .await
        .expect_err("call without any authorization header must be rejected");
    assert_eq!(err.code(), Code::Unauthenticated);
}

/// Confirms the newly added `Accounts` lifecycle RPCs
/// (`UpdateAccount`/`RemoveAccount`/`TestAccountConnection`) are each mounted
/// behind the SAME `BearerAuthInterceptor` as every other service -- an
/// unauthenticated call to any of them must be rejected exactly like an
/// unauthenticated `System/GetStatus`, with no carve-out for "just one more"
/// RPC.
#[tokio::test]
async fn rejects_calls_to_new_accounts_rpcs_without_a_bearer_token() {
    let event_bus = Arc::new(EventBus::new());
    let secrets = SecretManager::mock();
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );

    let (addr, _dir) = start_server(event_bus, token).await;
    let mut client =
        nuncio_proto::v1::accounts_client::AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

    let update_err = client
        .update_account(nuncio_proto::v1::UpdateAccountRequest {
            config: None,
            password: None,
        })
        .await
        .expect_err("unauthenticated update_account must be rejected");
    assert_eq!(update_err.code(), Code::Unauthenticated);

    let remove_err = client
        .remove_account(nuncio_proto::v1::RemoveAccountRequest {
            id: "acct-1".to_string(),
        })
        .await
        .expect_err("unauthenticated remove_account must be rejected");
    assert_eq!(remove_err.code(), Code::Unauthenticated);

    let test_err = client
        .test_account_connection(nuncio_proto::v1::TestAccountConnectionRequest {
            id: "acct-1".to_string(),
        })
        .await
        .expect_err("unauthenticated test_account_connection must be rejected");
    assert_eq!(test_err.code(), Code::Unauthenticated);
}

/// Confirms the streaming `Filters.Triage` RPC is mounted behind the SAME
/// `BearerAuthInterceptor` as every other `Filters` RPC -- an unauthenticated
/// call must be rejected before the stream is ever established, exactly
/// like an unauthenticated unary call.
#[tokio::test]
async fn rejects_triage_calls_without_a_bearer_token() {
    let event_bus = Arc::new(EventBus::new());
    let secrets = SecretManager::mock();
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );

    let (addr, _dir) = start_server(event_bus, token).await;
    let mut client =
        nuncio_proto::v1::filters_client::FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

    let err = client
        .triage(nuncio_proto::v1::TriageRequest {
            rule_id: None,
            chunk_size: 0,
        })
        .await
        .expect_err("call without any authorization header must be rejected");
    assert_eq!(err.code(), Code::Unauthenticated);
}
