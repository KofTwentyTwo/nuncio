//! End-to-End daemon integration test suite.
//!
//! Boots a real `nunciod` gRPC server and drives it via the in-workspace
//! `nuncio-cli` presentation shell (`HeadlessRunner`) to confirm a genuine
//! reference client round-trips full commands against a live daemon. This
//! suite intentionally has no dependency on the archived presentation
//! shells that were moved to `_reference/` (nuncio-tui, nuncio-gui,
//! nuncio-mcp) as part of shrinking the workspace to the engine + daemon +
//! reference CLI.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_cli::{Commands, HeadlessRunner, SystemSubcommand};
use nuncio_core::{CoreCommand, EventBus};
use nuncio_filter::FilterEngine;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use std::sync::Arc;
use tokio::net::TcpListener;

/// Reference-client proof that boots a real `nunciod` `nuncio.v1.System`
/// gRPC server (via
/// [`nunciod::grpc::serve_on_listener`]) on an ephemeral loopback port,
/// authenticated by a bearer token minted from a [`SecretManager::mock`]
/// (never the real OS keyring), then drives the real in-workspace
/// `nuncio-cli` `HeadlessRunner`'s `system status` gRPC client path against
/// it — sharing the *same* mock `SecretManager` instance so the CLI reads
/// back the identical previously-minted token rather than needing any
/// out-of-band secret transfer. Asserts the CLI reports the daemon's real,
/// live engine state (not a fabricated or default one) over the wire.
#[tokio::test]
async fn cli_system_status_round_trips_over_grpc_to_live_daemon() {
    // 1. Bring the daemon's live engine state to a known, non-default
    // status so this test cannot pass by accident against a hardcoded
    // fallback value anywhere in the round trip.
    let event_bus = Arc::new(EventBus::new());
    event_bus.process_command(CoreCommand::SyncAll);
    assert_eq!(
        event_bus.current_state().status,
        nuncio_core::EngineStatus::Syncing
    );

    // 2. Mint the gRPC bearer token from a mock vault (never the real OS
    // keyring) and share the *same* `SecretManager` instance between the
    // server-minting step and the CLI client below.
    let secrets = Arc::new(SecretManager::mock());
    let token_bytes = secrets
        .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
        .expect("mock vault mints gRPC bearer token");
    let token = hex::encode(&token_bytes);

    // 3. Boot the real `nunciod` gRPC server on an ephemeral loopback port.
    // Binding synchronously (before spawning) means the OS is already
    // accepting connections on `addr` once this call returns, so the CLI
    // client below can dial it immediately with no fixed sleep.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");
    let server_event_bus = event_bus.clone();
    let (server_db, _server_db_dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let server_secrets = secrets.clone();
    let server_filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    let _server_handle = tokio::spawn(async move {
        let _ = nunciod::grpc::serve_on_listener(
            listener,
            server_event_bus,
            Arc::new(server_db),
            server_filter_engine,
            server_secrets,
            token,
        )
        .await;
    });

    // 4. Drive the real `nuncio-cli` `HeadlessRunner`'s gRPC `system status`
    // client path against the live daemon, reading the token back from the
    // shared mock `SecretManager` (proving the CLI's injected-vault design
    // works end-to-end, not just against a hand-rolled token string).
    let cli_runner = HeadlessRunner::connect_with(secrets, addr.to_string());

    let cli_status = cli_runner
        .execute_command(
            &Commands::System {
                action: SystemSubcommand::Status,
            },
            true,
        )
        .await;
    assert!(
        cli_status.contains(r#""status":"ok""#),
        "expected a successful status envelope, got: {cli_status}"
    );
    assert!(cli_status.contains(r#""engine_status":"Syncing""#));
    assert!(cli_status.contains(env!("CARGO_PKG_VERSION")));
}
