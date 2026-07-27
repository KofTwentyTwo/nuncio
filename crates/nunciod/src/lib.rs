//! Centralized Standalone Background Daemon Server Crate (`nunciod`).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod grpc;
pub mod orchestrator;
pub mod send;
pub mod sync;
pub use orchestrator::SelfHealingSyncOrchestrator;

/// Default PERSISTENT database path for `nunciod`: `~/.nuncio/nuncio.db`
/// (or `%USERPROFILE%\.nuncio\nuncio.db` on Windows, since `USERPROFILE` is
/// checked first). This is deliberately NOT a temp/ephemeral path -- it is
/// what makes accounts and every other piece of daemon state survive a
/// daemon restart.
///
/// Mirrors `nuncio_store::recovery::CorruptedBackupManager::default_backup_dir`'s
/// existing `~/.nuncio/...` convention so all of `nunciod`'s on-disk state
/// lives under the same root directory.
///
/// Callers that need a different (e.g. isolated, ephemeral) path -- tests,
/// CI, multiple daemon instances on one machine -- MUST set the
/// `NUNCIO_DB_PATH` environment variable instead of calling this directly;
/// see `main`'s boot sequence.
pub fn default_db_path() -> std::path::PathBuf {
    if let Ok(home_str) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        std::path::PathBuf::from(home_str)
            .join(".nuncio")
            .join("nuncio.db")
    } else {
        std::path::PathBuf::from(".nuncio").join("nuncio.db")
    }
}

/// Environment variable that opts `nunciod` in to its autonomous
/// background auto-update-check loop.
///
/// # Security
///
/// `nuncio_core::UpdateEngine`'s checksum verification is currently
/// **fail-open**: if the `SHA256SUMS.txt` release asset is missing, an
/// update install proceeds without checksum verification. Until that
/// checksum-verification gap is closed, `nunciod` MUST NOT
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
            "autonomous auto-update loop must default to disabled"
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

    #[test]
    fn default_db_path_is_persistent_under_dot_nuncio_directory() {
        // Deliberately does not mutate `USERPROFILE`/`HOME` (real env vars
        // many other things -- `tempfile`, `keyring`, etc. -- may also
        // read); instead this asserts the path SHAPE holds regardless of
        // which of the two branches the current process environment takes,
        // proving the default is a stable `.nuncio/nuncio.db` layout and
        // never a bare OS temp directory.
        let path = default_db_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("nuncio.db"),
            "default db path must end in nuncio.db, got {path:?}"
        );
        assert_eq!(
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some(".nuncio"),
            "default db path must live under a .nuncio directory, got {path:?}"
        );
        assert!(
            !path.starts_with(std::env::temp_dir()),
            "default db path must never be an ephemeral temp path, got {path:?}"
        );
    }
}
