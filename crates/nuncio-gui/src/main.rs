use nuncio_core::EventBus;
use nuncio_gui::{GuiViewState, IpcBridge};

fn main() {
    let bus = EventBus::new();
    let state = IpcBridge::get_app_state(&bus);
    let view_state = GuiViewState::default();

    println!(
        "Nuncio GUI Desktop Shell Initialized: status={:?}, unread={}, active_folder={}",
        state.status, state.unread_count, view_state.active_folder_id
    );
}
