//! Layered `tracing` subscriber for `nunciod`.
//!
//! This replaces the previous stderr-only `tracing_subscriber::fmt::init()`
//! bootstrap, which pinned the level at INFO, ignored `RUST_LOG` (the
//! `env-filter` feature was not even enabled), had no file sink, no JSON
//! output, and no way to change levels at runtime -- leaving the daemon
//! effectively unfollowable and unauditable.
//!
//! The subscriber is a layered [`Registry`] with three layers:
//!
//! 1. A reloadable [`EnvFilter`] applied as the global filter, resolved from
//!    the environment (see [`resolve_filter_directive`]). It is wrapped in a
//!    [`reload::Layer`] so a future control RPC can change verbosity at
//!    runtime without a restart; the [`reload::Handle`] is returned from
//!    [`init`]. A malformed filter string never panics -- it degrades to the
//!    default and a `warn!` is emitted once the subscriber is live.
//! 2. A human-readable console layer to stderr (ANSI, target + span context).
//! 3. A rotating daily file layer under `<data_dir>/logs/`, written through a
//!    non-blocking background writer.
//!
//! ## Environment variables
//!
//! * `NUNCIO_LOG` -- filter directive; takes precedence over `RUST_LOG`.
//! * `RUST_LOG` -- filter directive; used when `NUNCIO_LOG` is unset/empty.
//! * (default) -- when neither is set, the filter is [`DEFAULT_FILTER`].
//! * `NUNCIO_LOG_FORMAT=json` -- emit the *file* sink as JSON. The console
//!   sink always stays human-readable (it is for an operator watching a
//!   terminal); the file sink is the one a machine parses, so JSON is applied
//!   there. Any other value (or unset) keeps the file sink human-readable.
//!
//! ## Worker guard lifetime (CRITICAL)
//!
//! The rotating file layer uses [`tracing_appender::non_blocking`], which
//! hands back a [`WorkerGuard`]. That guard MUST be held for the entire
//! process lifetime: when it drops, the background writer thread is joined and
//! any buffered-but-unflushed log lines are discarded. [`init`] therefore
//! returns it inside [`LoggingGuards`], and `main` binds that value (e.g.
//! `let _guards = ...;`) so it lives until the daemon exits. Dropping it early
//! silently loses file logs.
//!
//! ## Failure handling
//!
//! Logging must never crash the daemon. If the log directory cannot be
//! created, the file layer is skipped and the daemon logs to stderr only
//! (with a `warn!`). If a global subscriber is already installed (e.g. a test
//! harness set one), [`init`] leaves it in place and returns empty guards.

use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::{Layered, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{reload, EnvFilter, Layer, Registry};

/// Filter directive used when neither `NUNCIO_LOG` nor `RUST_LOG` is set.
pub const DEFAULT_FILTER: &str = "info";

/// Environment variable that overrides the log filter, taking precedence over
/// `RUST_LOG`.
pub const LOG_FILTER_ENV_VAR: &str = "NUNCIO_LOG";

/// Environment variable that, when equal to `"json"` (case-insensitive),
/// switches the file sink to JSON output.
pub const LOG_FORMAT_ENV_VAR: &str = "NUNCIO_LOG_FORMAT";

/// The output encoding of a log sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable, single-line-per-event text.
    Human,
    /// One JSON object per event, for machine ingestion.
    Json,
}

/// The base registry with the reloadable [`EnvFilter`] applied. The concrete
/// fmt layers are boxed against this type so the JSON and human variants (and
/// the presence/absence of the file layer) can be chosen at runtime without
/// leaking their differing types into the subscriber's type signature.
type FilteredRegistry = Layered<reload::Layer<EnvFilter, Registry>, Registry>;

/// A type-erased layer compatible with the filtered registry.
type DynLayer = Box<dyn Layer<FilteredRegistry> + Send + Sync>;

/// Handle that can swap the global [`EnvFilter`] at runtime. Exposed so a
/// future control RPC can raise or lower verbosity without restarting the
/// daemon.
pub type FilterReloadHandle = reload::Handle<EnvFilter, Registry>;

/// Guards and handles that must outlive the process.
///
/// Hold this for the whole daemon lifetime. See the module docs on the
/// [`WorkerGuard`] lifetime requirement.
pub struct LoggingGuards {
    reload_handle: Option<FilterReloadHandle>,
    // Kept alive purely for its `Drop`: dropping it flushes and shuts down the
    // non-blocking file writer. Never read directly.
    _file_guard: Option<WorkerGuard>,
}

impl LoggingGuards {
    /// Returns the runtime filter-reload handle, if the subscriber this crate
    /// installed is the active one. `None` means a subscriber was already set
    /// by someone else (e.g. a test harness) and this handle would not affect
    /// it.
    pub fn reload_handle(&self) -> Option<&FilterReloadHandle> {
        self.reload_handle.as_ref()
    }
}

/// Resolves the effective filter directive with precedence
/// `NUNCIO_LOG` > `RUST_LOG` > [`DEFAULT_FILTER`].
///
/// An env var set to an empty/whitespace-only string is treated as unset so a
/// stray `NUNCIO_LOG=` does not silence all output.
pub fn resolve_filter_directive() -> String {
    env_directive(LOG_FILTER_ENV_VAR)
        .or_else(|| env_directive("RUST_LOG"))
        .unwrap_or_else(|| DEFAULT_FILTER.to_string())
}

fn env_directive(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

/// Builds an [`EnvFilter`] from `directive`, degrading to [`DEFAULT_FILTER`]
/// on a parse error instead of panicking.
///
/// The second element is `Some(directive)` when the directive was malformed
/// and the default was substituted, so the caller can emit a `warn!` once the
/// subscriber is live (warning before init would go nowhere).
pub fn build_filter(directive: &str) -> (EnvFilter, Option<String>) {
    match EnvFilter::try_new(directive) {
        Ok(filter) => (filter, None),
        Err(_) => (EnvFilter::new(DEFAULT_FILTER), Some(directive.to_string())),
    }
}

/// Reads the desired file-sink format from [`LOG_FORMAT_ENV_VAR`].
pub fn log_format_from_env() -> LogFormat {
    match std::env::var(LOG_FORMAT_ENV_VAR) {
        Ok(value) if value.trim().eq_ignore_ascii_case("json") => LogFormat::Json,
        _ => LogFormat::Human,
    }
}

/// Derives the log directory (`<data_dir>/logs/`) from the database path,
/// whose parent is the daemon's data directory.
pub fn log_dir_for_db_path(db_path: &Path) -> PathBuf {
    match db_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("logs"),
        _ => PathBuf::from("logs"),
    }
}

/// Installs the global layered subscriber and returns the guards that must be
/// held for the process lifetime.
///
/// `db_path` is the resolved database path; logs are written to a sibling
/// `logs/` directory. This never panics and never returns an error: any
/// problem (unwritable log directory, an already-installed subscriber) is
/// degraded to a lesser-but-working configuration and reported via `warn!`.
pub fn init(db_path: &Path) -> LoggingGuards {
    let directive = resolve_filter_directive();
    let (filter, malformed) = build_filter(&directive);
    let (filter_layer, reload_handle) = reload::Layer::new(filter);

    let format = log_format_from_env();
    let log_dir = log_dir_for_db_path(db_path);

    // Try to stand up the rotating file sink. A failure to create the log
    // directory must not stop the daemon; it just falls back to stderr-only.
    let (file_layer, file_guard, file_error) = match std::fs::create_dir_all(&log_dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(&log_dir, "nunciod.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer: DynLayer = match format {
                LogFormat::Json => tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_ansi(false)
                    .boxed(),
                LogFormat::Human => tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .boxed(),
            };
            (Some(layer), Some(guard), None)
        }
        Err(err) => (None, None, Some(format!("{}: {err}", log_dir.display()))),
    };

    // Human-readable console sink for an operator watching the terminal. This
    // stays text even when the file sink is JSON.
    let console_layer: DynLayer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(true)
        .boxed();

    let mut layers: Vec<DynLayer> = vec![console_layer];
    if let Some(layer) = file_layer {
        layers.push(layer);
    }

    let subscriber = tracing_subscriber::registry()
        .with(filter_layer)
        .with(layers);

    if subscriber.try_init().is_err() {
        // A subscriber is already installed (commonly a test harness). Leave
        // it untouched and hand back empty guards; our reload handle would not
        // apply to it, so do not expose it.
        return LoggingGuards {
            reload_handle: None,
            _file_guard: None,
        };
    }

    if let Some(bad) = malformed {
        tracing::warn!(
            directive = %bad,
            default = %DEFAULT_FILTER,
            "ignoring malformed log filter directive; using default"
        );
    }
    if let Some(err) = file_error {
        tracing::warn!(error = %err, "could not open log directory; logging to stderr only");
    }

    LoggingGuards {
        reload_handle: Some(reload_handle),
        _file_guard: file_guard,
    }
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
        std::env::remove_var(LOG_FORMAT_ENV_VAR);
    }

    #[test]
    fn nuncio_log_beats_rust_log_beats_default() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();

        // Default when neither is set.
        assert_eq!(resolve_filter_directive(), DEFAULT_FILTER);

        // RUST_LOG wins over the default.
        std::env::set_var("RUST_LOG", "warn");
        assert_eq!(resolve_filter_directive(), "warn");

        // NUNCIO_LOG wins over RUST_LOG.
        std::env::set_var(LOG_FILTER_ENV_VAR, "debug");
        assert_eq!(resolve_filter_directive(), "debug");

        // An empty NUNCIO_LOG is treated as unset, so RUST_LOG applies again.
        std::env::set_var(LOG_FILTER_ENV_VAR, "   ");
        assert_eq!(resolve_filter_directive(), "warn");

        clear_env();
    }

    #[test]
    fn malformed_filter_falls_back_to_default_without_panicking() {
        // A valid directive parses and signals no fallback.
        let (_filter, malformed) = build_filter("info,nunciod=debug");
        assert!(malformed.is_none());

        // A directive with an invalid level degrades to the default and
        // reports the offending string, rather than panicking.
        let (_filter, malformed) = build_filter("nunciod=not_a_level");
        assert_eq!(malformed.as_deref(), Some("nunciod=not_a_level"));
    }

    #[test]
    fn json_format_is_selected_only_by_the_json_value() {
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();

        assert_eq!(log_format_from_env(), LogFormat::Human);

        std::env::set_var(LOG_FORMAT_ENV_VAR, "json");
        assert_eq!(log_format_from_env(), LogFormat::Json);

        // Case-insensitive.
        std::env::set_var(LOG_FORMAT_ENV_VAR, "JSON");
        assert_eq!(log_format_from_env(), LogFormat::Json);

        // Any other value stays human-readable.
        std::env::set_var(LOG_FORMAT_ENV_VAR, "text");
        assert_eq!(log_format_from_env(), LogFormat::Human);

        clear_env();
    }

    #[test]
    fn log_dir_is_a_logs_sibling_of_the_database_file() {
        let db = PathBuf::from("/var/lib/nuncio/nuncio.db");
        assert_eq!(
            log_dir_for_db_path(&db),
            PathBuf::from("/var/lib/nuncio/logs")
        );

        // A bare filename (no parent) still yields a usable relative dir.
        assert_eq!(
            log_dir_for_db_path(Path::new("nuncio.db")),
            PathBuf::from("logs")
        );
    }
}
