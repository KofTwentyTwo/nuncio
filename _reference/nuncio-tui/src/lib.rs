//! Nuncio Terminal UI library.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod app;
pub mod html;
pub mod keybindings;
pub mod layout;

pub use app::{AppMode, TuiApp};
pub use keybindings::{KeybindingEngine, UserAction};
pub use layout::{ActivePane, AppLayout};
