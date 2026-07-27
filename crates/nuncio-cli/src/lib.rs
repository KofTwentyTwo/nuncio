//! Nuncio CLI library.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod args;
pub mod output;
pub mod runner;

pub use args::{
    AccountSubcommand, CalSubcommand, Commands, FolderSubcommand, MailSubcommand, SystemSubcommand,
};
pub use output::JsonResponse;
pub use runner::HeadlessRunner;
