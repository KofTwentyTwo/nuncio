//! CLI-local `tracing` subscriber.
//!
//! This controls `nuncio-cli`'s OWN log output -- it is entirely separate
//! from `nunciod`'s own layered subscriber (`nunciod::logging`), which
//! governs the daemon process the CLI merely talks to over gRPC. The CLI
//! prints its normal command output on stdout regardless of this
//! configuration; this subscriber only ever writes to stderr, so scripts that
//! parse stdout (especially `--json` mode) are never polluted by log lines.
//!
//! ## Verbosity mapping
//!
//! `-v`/`--verbose` is a repeatable flag (`clap`'s `ArgAction::Count`), not a
//! boolean: `nuncio <cmd>` with none is quiet (`warn`), `-v` is `info`, `-vv`
//! is `debug`, and `-vvv` or more is `trace`. [`verbosity_directive`] is the
//! pure mapping and is unit-tested directly, without touching the process's
//! single global subscriber slot.
//!
//! ## Environment override
//!
//! `NUNCIO_LOG` (if set and non-blank) wins over `RUST_LOG`, which wins over
//! the `-v` count -- mirroring `nunciod`'s own precedence so the two
//! processes behave consistently for an operator who sets `NUNCIO_LOG`
//! globally.

use tracing_subscriber::EnvFilter;

/// Environment variable that overrides the log filter, taking precedence over
/// `RUST_LOG` and the `-v` count.
pub const LOG_FILTER_ENV_VAR: &str = "NUNCIO_LOG";

/// Maps a `-v` occurrence count to a filter directive. `0` is the quiet
/// default (`warn`); each additional `-v` steps up one level, capping at
/// `trace`.
#[must_use]
pub fn verbosity_directive(verbosity: u8) -> &'static str {
    match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    }
}

/// Resolves the effective filter directive with precedence `NUNCIO_LOG` >
/// `RUST_LOG` > the `-v` count (via [`verbosity_directive`]).
///
/// An env var set to an empty/whitespace-only string is treated as unset so a
/// stray `NUNCIO_LOG=` does not silently override an explicit `-v`.
#[must_use]
pub fn resolve_filter_directive(verbosity: u8) -> String {
    env_directive(LOG_FILTER_ENV_VAR)
        .or_else(|| env_directive("RUST_LOG"))
        .unwrap_or_else(|| verbosity_directive(verbosity).to_string())
}

fn env_directive(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

/// Installs a stderr-only `tracing` subscriber for the CLI process, driven by
/// [`resolve_filter_directive`].
///
/// Never panics: a malformed filter directive (from a hand-edited
/// `NUNCIO_LOG`/`RUST_LOG`) degrades to the `-v`-derived default rather than
/// crashing the CLI before it can even report an error. Idempotent -- if a
/// subscriber is already installed (e.g. a test harness), this is a silent
/// no-op rather than a panic, since `nuncio-cli`'s own tests run many
/// commands in the same process.
pub fn init(verbosity: u8) {
    let directive = resolve_filter_directive(verbosity);
    let filter = EnvFilter::try_new(&directive)
        .unwrap_or_else(|_| EnvFilter::new(verbosity_directive(verbosity)));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    // The process environment is global mutable state; serialize every test
    // that reads or writes the log env vars so they cannot interleave.
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_env() {
        std::env::remove_var(LOG_FILTER_ENV_VAR);
        std::env::remove_var("RUST_LOG");
    }

    #[test]
    fn verbosity_count_maps_to_expected_level() {
        assert_eq!(verbosity_directive(0), "warn");
        assert_eq!(verbosity_directive(1), "info");
        assert_eq!(verbosity_directive(2), "debug");
        assert_eq!(verbosity_directive(3), "trace");
        // Anything beyond -vvv still caps at trace rather than erroring.
        assert_eq!(verbosity_directive(255), "trace");
    }

    #[test]
    fn nuncio_log_beats_rust_log_beats_verbosity_count() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();

        // No env override: falls back to the -v count mapping.
        assert_eq!(resolve_filter_directive(0), "warn");
        assert_eq!(resolve_filter_directive(2), "debug");

        // RUST_LOG wins over the -v count.
        std::env::set_var("RUST_LOG", "nuncio_cli=warn");
        assert_eq!(resolve_filter_directive(2), "nuncio_cli=warn");

        // NUNCIO_LOG wins over RUST_LOG.
        std::env::set_var(LOG_FILTER_ENV_VAR, "nuncio_cli=trace");
        assert_eq!(resolve_filter_directive(0), "nuncio_cli=trace");

        // An empty NUNCIO_LOG is treated as unset, so RUST_LOG applies again.
        std::env::set_var(LOG_FILTER_ENV_VAR, "   ");
        assert_eq!(resolve_filter_directive(0), "nuncio_cli=warn");

        clear_env();
    }

    #[test]
    fn init_never_panics_even_when_a_subscriber_is_already_installed() {
        // Calling init() twice must not panic: the second call finds the
        // global subscriber slot occupied and silently no-ops.
        init(1);
        init(1);
    }
}
