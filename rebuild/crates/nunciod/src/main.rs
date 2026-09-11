mod config;
use clap::Parser;
use nuncio_engine::{
    engine::{Engine, EngineConfig},
    secrets::{keyring::OsKeyring, SecretStore},
};
use std::{io::Write, sync::Arc};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = config::Config::parse();
    match run(config).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(std::io::stderr().lock(), "{message}");
            std::process::ExitCode::from(1)
        }
    }
}

async fn run(config: config::Config) -> Result<(), String> {
    if std::env::vars_os().any(|(name, _)| name.to_string_lossy().starts_with("NUNCIO_TEST_")) {
        return Err("Test environment controls are unsupported".into());
    }
    let directory = config
        .directory()?
        .canonicalize()
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                config.directory().map_err(std::io::Error::other)
            } else {
                Err(error)
            }
        })
        .map_err(|_| "Data directory is unavailable")?;
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|_| "Daemon address is unavailable")?;
    let secrets: Arc<dyn SecretStore> = Arc::new(OsKeyring);
    #[cfg(feature = "test-harness")]
    let secrets: Arc<dyn SecretStore> = match config.test_secrets_file {
        Some(path) => Arc::new(nuncio_engine::secrets::test_store::FileTestStore::new(path)),
        None => secrets,
    };
    let engine_config = EngineConfig { directory, secrets };
    #[cfg(feature = "test-harness")]
    let engine = match config.test_config {
        Some(path) => {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .map_err(|_| "Test configuration is unavailable")?
                .take(65537)
                .read_to_end(&mut bytes)
                .map_err(|_| "Test configuration is unavailable")?;
            if bytes.len() > 65536 {
                return Err("Test configuration is too large".into());
            }
            let settings =
                serde_json::from_slice(&bytes).map_err(|_| "Test configuration is invalid")?;
            Engine::open_for_test(engine_config, settings).await
        }
        None => Engine::open(engine_config).await,
    };
    #[cfg(not(feature = "test-harness"))]
    let engine = Engine::open(engine_config).await;
    let engine = engine.map_err(|error| error.to_string())?;
    let status = engine.status().await.map_err(|error| error.to_string())?;
    let address = listener
        .local_addr()
        .map_err(|_| "Bound address is unavailable")?;
    let ready = serde_json::json!({"event":"ready", "endpoint":format!("http://{address}"), "profile_id":status.profile_id, "api_version":status.api_version,"pid":std::process::id()});
    if let Some(path) = config.ready_file {
        if publish_readiness(&path, &ready).is_err() {
            let _ = engine.shutdown().await;
            return Err(
                "Readiness file could not be created; existing paths were preserved".into(),
            );
        }
    }
    writeln!(std::io::stdout().lock(), "{ready}").map_err(|_| "Readiness output failed")?;
    nunciod::serve(engine, listener)
        .await
        .map_err(|error| error.to_string())
}

fn publish_readiness(
    path: &std::path::Path,
    ready: &serde_json::Value,
) -> Result<(), std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, ready)?;
    file.write_all(b"\n")?;
    file.sync_all()
}
