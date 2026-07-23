//! E2E System Test Suite for nuncio-gui.

use nuncio_core::{EngineStatus, EventBus};
use nuncio_gui::{GuiViewState, HtmlSanitizer, IpcBridge, IpcCommandPayload};

#[tokio::test]
async fn system_test_gui_ipc_sandbox_and_view_matrix() {
    let bus = EventBus::new();
    let mut state_rx = bus.subscribe_state();

    // 1. App State Query via IPC
    let initial_state = IpcBridge::get_app_state(&bus);
    assert_eq!(initial_state.status, EngineStatus::Idle);

    // 2. Command Dispatch: Sync
    IpcBridge::handle_ipc_command(
        &bus,
        IpcCommandPayload {
            action: "sync".to_string(),
            message_id: None,
            read: None,
            account_id: None,
            email: None,
            imap_host: None,
            imap_port: None,
            smtp_host: None,
            smtp_port: None,
        },
    );
    assert_eq!(state_rx.borrow_and_update().status, EngineStatus::Syncing);

    // 3. Command Dispatch: MarkRead
    IpcBridge::handle_ipc_command(
        &bus,
        IpcCommandPayload {
            action: "mark_read".to_string(),
            message_id: Some("msg-100".to_string()),
            read: Some(true),
            account_id: None,
            email: None,
            imap_host: None,
            imap_port: None,
            smtp_host: None,
            smtp_port: None,
        },
    );

    // 4. Command Dispatch: Shutdown
    IpcBridge::handle_ipc_command(
        &bus,
        IpcCommandPayload {
            action: "shutdown".to_string(),
            message_id: None,
            read: None,
            account_id: None,
            email: None,
            imap_host: None,
            imap_port: None,
            smtp_host: None,
            smtp_port: None,
        },
    );

    // 5. HTML Sandboxing & CSP Output Verification
    let raw_html = "<p>Welcome <script>alert('xss')</script></p>";
    let iframe = HtmlSanitizer::build_sandboxed_iframe(raw_html);
    assert!(iframe.contains(r#"sandbox=""#));
    assert!(iframe.contains("default-src 'none'"));
    assert!(iframe.contains("<!-- <script"));

    // 6. View State Navigation
    let mut view = GuiViewState::default();
    assert_eq!(view.active_folder_id, "inbox");

    view.select_folder("Sent");
    assert_eq!(view.active_folder_id, "Sent");

    view.select_message("msg-500");
    assert_eq!(view.selected_message_id, Some("msg-500".to_string()));
}
