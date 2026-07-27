//! Centralized Standalone Background Daemon Server Crate (`nunciod`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod grpc;
pub mod orchestrator;
pub use orchestrator::SelfHealingSyncOrchestrator;

/// Environment variable that opts `nunciod` in to its autonomous
/// background auto-update-check loop.
///
/// # Security (GH #140 / backlog story 0.B.2)
///
/// `nuncio_core::UpdateEngine`'s checksum verification is currently
/// **fail-open**: if the `SHA256SUMS.txt` release asset is missing, an
/// update install proceeds without checksum verification. That will be
/// fixed properly in Phase 4. Until then, `nunciod` MUST NOT
/// autonomously check for or install updates, so the background loop
/// defaults to disabled and is only spawned when an operator explicitly
/// opts in via this environment variable.
pub const AUTO_UPDATE_ENV_VAR: &str = "NUNCIO_AUTO_UPDATE_ENABLED";

/// Returns `true` only if the operator has explicitly opted in to the
/// autonomous background auto-update-check loop by setting
/// [`AUTO_UPDATE_ENV_VAR`] to `"1"` or `"true"` (case-insensitive).
/// Defaults to `false` (disabled).
///
/// See the module-level documentation on [`AUTO_UPDATE_ENV_VAR`] for why
/// this defaults to disabled.
pub fn auto_update_task_enabled() -> bool {
    match std::env::var(AUTO_UPDATE_ENV_VAR) {
        Ok(val) => val.eq_ignore_ascii_case("1") || val.eq_ignore_ascii_case("true"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize access to the shared process environment variable so this
    // test cannot interleave with any other test that might touch it.
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn auto_update_task_disabled_by_default() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(AUTO_UPDATE_ENV_VAR);
        assert!(
            !auto_update_task_enabled(),
            "autonomous auto-update loop must default to disabled (GH #140)"
        );
    }

    #[test]
    fn auto_update_task_enabled_requires_explicit_opt_in() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(AUTO_UPDATE_ENV_VAR, "true");
        assert!(auto_update_task_enabled());
        std::env::set_var(AUTO_UPDATE_ENV_VAR, "0");
        assert!(!auto_update_task_enabled());
        std::env::remove_var(AUTO_UPDATE_ENV_VAR);
        assert!(!auto_update_task_enabled());
    }
}
