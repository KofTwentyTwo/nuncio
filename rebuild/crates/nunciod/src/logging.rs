use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
impl LogLevel {
    fn filter(self) -> LevelFilter {
        match self {
            Self::Off => LevelFilter::OFF,
            Self::Error => LevelFilter::ERROR,
            Self::Warn => LevelFilter::WARN,
            Self::Info => LevelFilter::INFO,
            Self::Debug => LevelFilter::DEBUG,
            Self::Trace => LevelFilter::TRACE,
        }
    }
}

fn subscriber<W>(level: LogLevel, writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    // Only audited application events. Do not enable dependency wire tracing,
    // install a log bridge, or accept RUST_LOG overrides containing credentials.
    let filter = tracing_subscriber::filter::Targets::new()
        .with_target("nunciod", level.filter())
        .with_target("nuncio_engine", level.filter());
    tracing_subscriber::registry().with(filter).with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer),
    )
}

pub fn init(level: LogLevel) -> Result<(), &'static str> {
    subscriber(level, std::io::stderr)
        .try_init()
        .map_err(|_| "Application logging could not be initialized")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[allow(clippy::unwrap_used)]
    fn app_levels_cannot_enable_third_party_wire_logs() {
        for (level, debug, trace, warning) in [
            (LogLevel::Info, false, false, true),
            (LogLevel::Debug, true, false, true),
            (LogLevel::Trace, true, true, true),
            (LogLevel::Warn, false, false, true),
            (LogLevel::Error, false, false, false),
            (LogLevel::Off, false, false, false),
        ] {
            let file = tempfile::NamedTempFile::new().unwrap();
            let writer = file.reopen().unwrap();
            tracing::subscriber::with_default(subscriber(level, writer), || {
                tracing::error!(target: "nunciod", "app-error");
                tracing::warn!(target: "nunciod", "app-warning");
                tracing::debug!(target: "nuncio_engine::sync", "app-debug");
                tracing::trace!(target: "nuncio_engine::sync", "app-trace");
                tracing::error!(target: "reqwest", "wire-secret-canary");
                tracing::trace!(target: "async_imap", "wire-secret-canary");
                tracing::debug!(target: "lettre", "wire-secret-canary");
                tracing::info!(target: "hyper", "wire-secret-canary");
            });
            let logs = std::fs::read_to_string(file.path()).unwrap();
            assert_eq!(logs.contains("app-error"), !matches!(level, LogLevel::Off));
            assert_eq!(logs.contains("app-warning"), warning);
            assert_eq!(logs.contains("app-debug"), debug);
            assert_eq!(logs.contains("app-trace"), trace);
            assert!(!logs.contains("wire-secret-canary"));
        }
    }
}
