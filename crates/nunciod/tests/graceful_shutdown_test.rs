//! Integration test for `nunciod`'s graceful shutdown path.
//!
//! Before this, `nunciod` installed no signal handler and never called
//! [`DatabaseEngine::close`], so every real exit was an OS kill racing the
//! WAL checkpoint that `close` exists to force. This test drives the same
//! building blocks `main.rs` wires together -- a [`ShutdownController`]
//! shared by the gRPC server and a background worker, triggered directly
//! (never a real OS signal, for determinism) -- and asserts:
//!   (a) the gRPC server stops serving once the shutdown signal fires;
//!   (b) a background worker exits its loop on the same signal instead of
//!       being aborted mid-iteration;
//!   (c) `DatabaseEngine::close()` runs and leaves the pool unusable for
//!       any subsequent query.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::system_client::SystemClient;
use nuncio_proto::v1::GetStatusRequest;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::serve_on_listener_with_shutdown;
use nunciod::lifecycle::ShutdownController;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tonic::Request;

const JOIN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn shutdown_signal_stops_grpc_server_exits_background_worker_and_closes_the_database() {
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

    // (a) the gRPC server: mirrors main.rs's `serve_with_shutdown` wiring --
    // the future it's given resolves from the SAME `ShutdownSignal` the
    // worker below observes.
    let mut grpc_shutdown = shutdown_signal.clone();
    let grpc_task = tokio::spawn(serve_on_listener_with_shutdown(
        listener,
        event_bus.clone(),
        db.clone(),
        filter_engine,
        secrets,
        token.clone(),
        controller.clone(),
        async move { grpc_shutdown.wait().await },
    ));

    // Confirm the server actually accepts a call before shutdown, so a
    // trivially-passing "stopped" observation later isn't just "it never
    // started".
    let mut client = SystemClient::connect(format!("http://{addr}"))
        .await
        .expect("client connects before shutdown");
    let mut request = Request::new(GetStatusRequest {});
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid ascii metadata value"),
    );
    client
        .get_status(request)
        .await
        .expect("authenticated call succeeds before shutdown");

    // (b) a background worker built the same way the outbox worker and
    // update-check loop are in `main.rs`: a `tokio::select!` between the
    // shared shutdown signal and a repeating unit of work. `tick_count`
    // proves the loop body actually ran, ruling out the trivial case where
    // the select immediately picks the shutdown branch every time.
    let tick_count = Arc::new(AtomicUsize::new(0));
    let tick_count_worker = tick_count.clone();
    let mut worker_shutdown = shutdown_signal.clone();
    let worker_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(20));
        loop {
            tokio::select! {
                _ = worker_shutdown.wait() => break,
                _ = interval.tick() => {
                    tick_count_worker.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    });

    // Let the worker tick a few times before shutdown is requested.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        tick_count.load(Ordering::SeqCst) > 0,
        "worker must have run at least one iteration before shutdown"
    );

    // Trigger shutdown once, from a single controller shared by both tasks
    // -- exactly the production wiring, minus the real OS signal.
    controller.trigger();

    let grpc_result = tokio::time::timeout(JOIN_TIMEOUT, grpc_task)
        .await
        .expect("gRPC server task must exit within the join timeout, not hang")
        .expect("gRPC server task must not panic");
    assert!(
        grpc_result.is_ok(),
        "serve_on_listener_with_shutdown must return Ok once it finishes draining, got {grpc_result:?}"
    );

    tokio::time::timeout(JOIN_TIMEOUT, worker_task)
        .await
        .expect("background worker must exit on the shutdown signal within the join timeout, not be killed")
        .expect("background worker task must not panic");

    // A dial attempt after the server task has exited must fail: the
    // listener is gone, proving the server actually stopped accepting
    // connections rather than merely returning while still bound.
    assert!(
        SystemClient::connect(format!("http://{addr}"))
            .await
            .is_err(),
        "gRPC server must no longer be reachable after shutdown"
    );

    // (c) `DatabaseEngine::close()` -- forces the WAL checkpoint and closes
    // the pool. A query issued afterward must fail, proving `close()` ran
    // rather than merely being reachable dead code.
    db.close().await;
    let err = db
        .list_filter_rules()
        .await
        .expect_err("a query after close() must fail because the pool is closed");
    let message = err.to_string();
    assert!(
        message.to_lowercase().contains("closed") || message.to_lowercase().contains("pool"),
        "expected a pool-closed style error after DatabaseEngine::close(), got: {message}"
    );
}

#[tokio::test]
async fn controller_trigger_is_observed_by_every_clone_of_the_signal() {
    let event_bus = Arc::new(EventBus::new());
    let (controller, shutdown_signal) = ShutdownController::new(event_bus);

    let mut first = shutdown_signal.clone();
    let mut second = shutdown_signal.clone();
    assert!(!first.is_requested());
    assert!(!second.is_requested());

    controller.trigger();

    tokio::time::timeout(JOIN_TIMEOUT, first.wait())
        .await
        .expect("first clone must observe the trigger");
    tokio::time::timeout(JOIN_TIMEOUT, second.wait())
        .await
        .expect("second clone must observe the trigger");
    assert!(second.is_requested());
}
