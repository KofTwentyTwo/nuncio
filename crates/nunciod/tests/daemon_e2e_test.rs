//! End-to-End multi-shell daemon integration test suite.
//!
//! Exercises the `nunciod` daemon end-to-end over the real IPC socket using
//! `nuncio-core`'s `IpcClient`/`IpcDaemonServer`, and independently exercises
//! the in-workspace `nuncio-cli` presentation shell (`HeadlessRunner`) to
//! confirm a real reference client shell continues to function alongside
//! the daemon. This suite intentionally has no dependency on the archived
//! presentation shells that were moved to `_reference/` (nuncio-tui,
//! nuncio-gui, nuncio-mcp) as part of shrinking the workspace to the
//! engine + daemon + reference CLI (backlog 0.D.2 / GH-145).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_cli::{Commands, HeadlessRunner, MailSubcommand, SystemSubcommand};
use nuncio_core::ipc::{IpcClient, IpcDaemonServer};
use nuncio_core::{CoreCommand, EventBus};
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use std::sync::Arc;
use tokio::net::TcpListener;

/// Boots a real `nunciod` `IpcDaemonServer` and drives it concurrently from
/// three independent `IpcClient` socket connections (standing in for any
/// number of simultaneous thin presentation shells talking JSON-RPC over
/// TCP), then separately drives the real `nuncio-cli` `HeadlessRunner`
/// reference shell to confirm it still executes full noun+verb commands
/// correctly. This preserves the daemon-boot + real-client-round-trip
/// coverage the previous multi-shell test provided, without requiring the
/// archived `nuncio-mcp` crate.
#[tokio::test]
async fn e2e_multi_shell_daemon_concurrency_test() {
    let event_bus = Arc::new(EventBus::new());
    let addr = "127.0.0.1:19424";
    let server = IpcDaemonServer::new(event_bus.clone(), addr);

    tokio::spawn(async move {
        let _ = server.run_server().await;
    });

    // Simulate 3 concurrent thin-shell connections to the nunciod daemon
    // over the real IPC socket.
    let client_a = IpcClient::new(addr);
    let client_b = IpcClient::new(addr);
    let client_c = IpcClient::new(addr);

    // Give server time to bind TCP socket.
    let mut ping_res = false;
    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(res) = client_a.ping().await {
            ping_res = res;
            if ping_res {
                break;
            }
        }
    }
    assert!(ping_res);
    assert!(client_b.ping().await.expect("shell b ping"));
    assert!(client_c.ping().await.expect("shell c ping"));

    // 2. Shell B issues a sync command over its own concurrent connection.
    let sync_res = client_b
        .send_command(CoreCommand::SyncAll)
        .await
        .expect("shell b sync command");
    assert_eq!(sync_res["status"], "dispatched");

    // 3. Shell A fetches state and verifies status is Syncing.
    let state_res = client_a.get_state().await.expect("shell a get state");
    assert_eq!(state_res["status"].as_str().unwrap(), "Syncing");

    // 4. Shell C marks a message read over its own concurrent connection.
    let mark_res = client_c
        .send_command(CoreCommand::MarkRead {
            message_id: "msg-1".to_string(),
            read: true,
        })
        .await
        .expect("shell c mark read");
    assert_eq!(mark_res["status"], "marked");

    // 5. Independently, drive the real in-workspace `nuncio-cli` reference
    // shell (its own ephemeral local engine, per its documented headless
    // design) to confirm a genuine presentation-shell client still
    // round-trips full commands correctly end-to-end.
    let cli_runner = HeadlessRunner::ephemeral()
        .await
        .expect("cli headless runner initializes");

    let cli_sync = cli_runner
        .execute_command(
            &Commands::Mail {
                action: MailSubcommand::Sync,
            },
            true,
        )
        .await;
    assert!(cli_sync.contains(r#""status":"sync_started""#));

    // `system status` no longer reflects this `HeadlessRunner`'s own local
    // ephemeral state: it is a real gRPC client of the `nunciod` daemon
    // (backlog story 1.A.3 / GH-150). That round trip is exercised in full,
    // against a real `nunciod` gRPC server, by
    // `cli_system_status_round_trips_over_grpc_to_live_daemon` below.
}

/// Reference-client proof for backlog story 1.A.3 (GH-150): boots a real
/// `nunciod` `nuncio.v1.System` gRPC server (via
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
    let _server_handle = tokio::spawn(async move {
        let _ = nunciod::grpc::serve_on_listener(listener, server_event_bus, token).await;
    });

    // 4. Drive the real `nuncio-cli` `HeadlessRunner`'s gRPC `system status`
    // client path against the live daemon, reading the token back from the
    // shared mock `SecretManager` (proving the CLI's injected-vault design
    // works end-to-end, not just against a hand-rolled token string).
    let cli_runner = HeadlessRunner::ephemeral_with(secrets, addr.to_string())
        .await
        .expect("cli headless runner initializes");

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
