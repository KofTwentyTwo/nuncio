//! Nuncio GUI Desktop Library.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod ipc;
pub mod sandbox;
pub mod view;

pub use ipc::{IpcBridge, IpcCommandPayload};
pub use sandbox::HtmlSanitizer;
pub use view::GuiViewState;
