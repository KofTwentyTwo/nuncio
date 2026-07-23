//! System integration test suite for nuncio-core IPC client-server framing & JSON-RPC protocol.

use nuncio_core::ipc::{IpcClient, IpcDaemonServer};
use nuncio_core::{CoreCommand, EventBus};
use std::sync::Arc;

#[tokio::test]
async fn system_test_ipc_daemon_ping_state_and_commands() {
    let event_bus = Arc::new(EventBus::new());
    let addr = "127.0.0.1:19423";
    let server = IpcDaemonServer::new(event_bus.clone(), addr);

    tokio::spawn(async move {
        let _ = server.run_server().await;
    });

    let client = IpcClient::new(addr);

    // Give server time to bind TCP socket
    let mut ping_res = false;
    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(res) = client.ping().await {
            ping_res = res;
            if ping_res {
                break;
            }
        }
    }
    assert!(ping_res);

    // 2. Fetch State
    let state_res = client.get_state().await.expect("get state success");
    assert_eq!(state_res["status"].as_str().unwrap(), "Idle");

    // 3. Dispatch CoreCommand over IPC
    let sync_res = client.send_command(CoreCommand::SyncAll).await.expect("send command success");
    assert_eq!(sync_res["status"], "dispatched");
    assert_eq!(event_bus.current_state().status, nuncio_core::EngineStatus::Syncing);
}
