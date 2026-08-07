//! Proves an authenticated `System.Shutdown` actually stops the daemon, using
//! the same wiring `main.rs` assembles: one `ShutdownController` shared by
//! the gRPC server and the lifecycle path. Never opens a socket off
//! 127.0.0.1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::system_client::SystemClient;
use nuncio_proto::v1::ShutdownRequest;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::serve_on_listener_with_shutdown;
use nunciod::lifecycle::ShutdownController;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tonic::metadata::MetadataValue;
use tonic::Request;

const JOIN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn authenticated_shutdown_stops_the_serving_task() {
    let event_bus = Arc::new(EventBus::new());
    let (controller, shutdown_signal) = ShutdownController::new(event_bus.clone());
    let controller = Arc::new(controller);

    let secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    let (db, _dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let db = Arc::new(db);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    let serve = tokio::spawn(serve_on_listener_with_shutdown(
        listener,
        event_bus,
        db,
        filter_engine,
        secrets,
        token.clone(),
        Arc::clone(&controller),
        async move {
            let mut sig = shutdown_signal;
            sig.wait().await;
        },
    ));

    let mut client = SystemClient::connect(format!("http://{addr}"))
        .await
        .expect("connect");

    let mut request = Request::new(ShutdownRequest {});
    request.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {token}")).expect("valid metadata"),
    );

    client.shutdown(request).await.expect("shutdown accepted");

    // The RPC returns once shutdown is REQUESTED; the serve task must then
    // finish on its own. A timeout here means the trigger never reached the
    // lifecycle signal.
    let joined = tokio::time::timeout(JOIN_TIMEOUT, serve).await;
    assert!(
        joined.is_ok(),
        "gRPC serve task did not stop within {JOIN_TIMEOUT:?} of Shutdown"
    );
}
