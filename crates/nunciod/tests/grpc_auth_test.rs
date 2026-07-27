//! Integration test for the `nuncio.v1.System` gRPC server wired into
//! `nunciod` (backlog story 1.A.2 / GH-149).
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
use nuncio_proto::v1::system_client::SystemClient;
use nuncio_proto::v1::GetStatusRequest;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::serve_on_listener;
use std::sync::Arc;
use tokio::net::TcpListener;
use tonic::{Code, Request};

/// Binds an ephemeral loopback port and spawns the real gRPC server on it.
/// Because `TcpListener::bind` has already succeeded (the OS is accepting
/// connections on the returned port) before this function returns, callers
/// can dial `addr` immediately with no fixed sleep.
async fn start_server(event_bus: Arc<EventBus>, token: String) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");
    tokio::spawn(async move {
        let _ = serve_on_listener(listener, event_bus, token).await;
    });
    addr
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

    let addr = start_server(event_bus, token).await;
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

    let addr = start_server(event_bus, token).await;
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

    let addr = start_server(event_bus.clone(), token.clone()).await;
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
