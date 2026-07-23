//! Nuncio GUI Desktop Library.

pub mod ipc;
pub mod sandbox;
pub mod view;

pub use ipc::{IpcBridge, IpcCommandPayload};
pub use sandbox::HtmlSanitizer;
pub use view::GuiViewState;
