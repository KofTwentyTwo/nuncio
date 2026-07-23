//! Tauri v2 desktop application shell bindings for Nuncio GUI.

use nuncio_core::EventBus;
use nuncio_gui::{IpcBridge, IpcCommandPayload};
use std::sync::Mutex;
use tauri::State;

/// Managed application state holding the `EventBus`.
pub struct GuiState {
    /// Event bus instance for IPC dispatch.
    pub bus: Mutex<EventBus>,
}

impl Default for GuiState {
    fn default() -> Self {
        Self {
            bus: Mutex::new(EventBus::new()),
        }
    }
}

/// Tauri command bindings module.
pub mod commands {
    use super::*;

    /// Tauri command to retrieve current application state snapshot.
    #[tauri::command]
    pub fn get_app_state(state: State<'_, GuiState>) -> Result<serde_json::Value, String> {
        let bus = state
            .bus
            .lock()
            .map_err(|e| format!("Failed to lock EventBus state: {e}"))?;
        let app_state = IpcBridge::get_app_state(&bus);
        Ok(serde_json::json!({
            "status": format!("{:?}", app_state.status),
            "accounts_loaded": app_state.accounts_loaded,
            "unread_count": app_state.unread_count,
            "last_error": app_state.last_error,
        }))
    }

    /// Tauri command to dispatch IPC payload command to `IpcBridge`.
    #[tauri::command]
    pub fn dispatch_ipc_command(
        state: State<'_, GuiState>,
        payload: IpcCommandPayload,
    ) -> Result<(), String> {
        let bus = state
            .bus
            .lock()
            .map_err(|e| format!("Failed to lock EventBus state: {e}"))?;
        IpcBridge::handle_ipc_command(&bus, payload);
        Ok(())
    }
}

/// Run Tauri v2 desktop shell application.
pub fn run() {
    let result = tauri::Builder::default()
        .manage(GuiState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_app_state,
            commands::dispatch_ipc_command
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        eprintln!("Error running Tauri application: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gui_state_default() {
        let state = GuiState::default();
        let bus = state.bus.lock();
        assert!(bus.is_ok());
    }
}
