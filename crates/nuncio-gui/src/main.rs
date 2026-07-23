//! Nuncio GUI application entry point.

mod ipc;

use ipc::IpcBridge;
use nuncio_core::EventBus;

fn main() {
    let bus = EventBus::new();
    let state = IpcBridge::get_app_state(&bus);

    println!(
        "Nuncio GUI Desktop Shell Initialized: status={:?}, unread={}",
        state.status, state.unread_count
    );
}
