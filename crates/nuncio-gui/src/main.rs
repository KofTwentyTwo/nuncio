//! Nuncio GUI application entry point.

mod ipc;
mod sandbox;
mod view;

use ipc::IpcBridge;
use nuncio_core::EventBus;
use view::GuiViewState;

fn main() {
    let bus = EventBus::new();
    let state = IpcBridge::get_app_state(&bus);
    let view_state = GuiViewState::default();

    println!(
        "Nuncio GUI Desktop Shell Initialized: status={:?}, unread={}, active_folder={}",
        state.status, state.unread_count, view_state.active_folder_id
    );
}
