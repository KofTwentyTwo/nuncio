//! Tauri IPC state bridge and command dispatch handlers.

use nuncio_core::{AppState, CoreCommand, EventBus};
use serde::{Deserialize, Serialize};

/// Tauri IPC command payload wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcCommandPayload {
    /// Action type name.
    pub action: String,
    /// Message ID if applicable.
    pub message_id: Option<String>,
    /// Read flag if applicable.
    pub read: Option<bool>,
}

/// Tauri IPC bridge orchestrating communications between WebVIEW frontend and `EventBus`.
pub struct IpcBridge;

impl IpcBridge {
    /// Retrieve current application state snapshot for WebVIEW initialization.
    pub fn get_app_state(bus: &EventBus) -> AppState {
        bus.current_state()
    }

    /// Process incoming IPC payload command.
    pub fn handle_ipc_command(bus: &EventBus, payload: IpcCommandPayload) {
        match payload.action.as_str() {
            "sync" => bus.process_command(CoreCommand::SyncAll),
            "mark_read" => {
                if let Some(msg_id) = payload.message_id {
                    let read_flag = payload.read.unwrap_or(true);
                    bus.process_command(CoreCommand::MarkRead {
                        message_id: msg_id,
                        read: read_flag,
                    });
                }
            }
            "shutdown" => bus.process_command(CoreCommand::Shutdown),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::EngineStatus;

    #[test]
    fn ipc_handle_sync_command() {
        let bus = EventBus::new();
        let payload = IpcCommandPayload {
            action: "sync".to_string(),
            message_id: None,
            read: None,
        };

        IpcBridge::handle_ipc_command(&bus, payload);
        assert_eq!(IpcBridge::get_app_state(&bus).status, EngineStatus::Syncing);
    }

    #[test]
    fn ipc_handle_mark_read_command() {
        let bus = EventBus::with_initial_state(AppState {
            unread_count: 5,
            ..AppState::default()
        });

        let payload = IpcCommandPayload {
            action: "mark_read".to_string(),
            message_id: Some("msg-1".to_string()),
            read: Some(true),
        };

        IpcBridge::handle_ipc_command(&bus, payload);
        assert_eq!(IpcBridge::get_app_state(&bus).unread_count, 4);
    }

    #[test]
    fn ipc_handle_shutdown_and_unknown() {
        let bus = EventBus::new();

        let payload = IpcCommandPayload {
            action: "shutdown".to_string(),
            message_id: None,
            read: None,
        };
        IpcBridge::handle_ipc_command(&bus, payload);
        assert_eq!(IpcBridge::get_app_state(&bus).status, EngineStatus::ShuttingDown);

        // Unknown action does not panic
        let unknown = IpcCommandPayload {
            action: "unknown".to_string(),
            message_id: None,
            read: None,
        };
        IpcBridge::handle_ipc_command(&bus, unknown);
    }
}
