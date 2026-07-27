//! Centralized Standalone Background Daemon Server Crate (`nunciod`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod orchestrator;
pub use orchestrator::SelfHealingSyncOrchestrator;
