//! End-to-End multi-interface daemon integration test suite.

use nuncio_core::ipc::{IpcClient, IpcDaemonServer};
use nuncio_core::{CoreCommand, EventBus};
use std::sync::Arc;

#[tokio::test]
async fn e2e_multi_shell_daemon_concurrency_test() {
    let event_bus = Arc::new(EventBus::new());
    let addr = "127.0.0.1:19424";
    let server = IpcDaemonServer::new(event_bus.clone(), addr);

    tokio::spawn(async move {
        let _ = server.run_server().await;
    });

    // Simulate 3 concurrent shells connecting to nunciod daemon
    let client_cli = IpcClient::new(addr);
    let client_tui = IpcClient::new(addr);
    let client_mcp = IpcClient::new(addr);

    // Give server time to bind TCP socket
    let mut ping_res = false;
    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(res) = client_cli.ping().await {
            ping_res = res;
            if ping_res {
                break;
            }
        }
    }
    assert!(ping_res);
    assert!(client_tui.ping().await.expect("tui ping"));
    assert!(client_mcp.ping().await.expect("mcp ping"));

    // 2. MCP shell issues sync command
    let sync_res = client_mcp.send_command(CoreCommand::SyncAll).await.expect("mcp sync command");
    assert_eq!(sync_res["status"], "dispatched");

    // 3. CLI shell fetches state and verifies status is Syncing
    let state_res = client_cli.get_state().await.expect("cli get state");
    assert_eq!(state_res["status"].as_str().unwrap(), "Syncing");

    // 4. TUI shell marks message read
    let mark_res = client_tui.send_command(CoreCommand::MarkRead { message_id: "msg-1".to_string(), read: true }).await.expect("tui mark read");
    assert_eq!(mark_res["status"], "marked");
}
