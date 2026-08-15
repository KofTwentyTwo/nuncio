//! Headless bearer-token resolution for the reference CLI.
//!
//! `nunciod` mints the gRPC bearer token into the OS keyring, so the keyring
//! item's access control list trusts the *daemon* binary. Any other binary
//! reading that item -- the CLI -- is a different program, and platform
//! keychains answer that by asking the logged-in user for consent. Nothing can
//! answer that prompt from CI, an ssh session, or a background shell, so the
//! read blocks forever. `NUNCIO_GRPC_TOKEN` is the explicit, opt-in escape
//! hatch; the keyring stays the default.
//!
//! These tests boot the real daemon gRPC server on an ephemeral loopback port
//! (mock vault, no real keychain, no live network) and drive the real
//! `HeadlessRunner` against it to prove:
//!
//!   - an override authenticates and the vault is never consulted for it;
//!   - with no override the keyring path is unchanged and still authenticates;
//!   - a wrong override fails honestly and the token never reaches the output
//!     or the telemetry.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex, OnceLock};

use nuncio_cli::{Commands, HeadlessRunner, SystemSubcommand};
use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use tokio::net::TcpListener;
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

/// A bearer token that is unmistakable if it ever leaks into output or logs.
const SENTINEL_TOKEN: &str = "SENTINELheadlessCliBearerTokenVvWw001";

/// Boots the real daemon gRPC server on an ephemeral loopback port,
/// authenticated by `token`. Returns the address; the temp dir is kept alive
/// by the returned guard.
async fn boot_daemon(secrets: Arc<SecretManager>, token: String) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");
    let (db, db_dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    tokio::spawn(async move {
        // The temp dir must outlive the server, so it is moved into the task.
        let _db_dir = db_dir;
        let _ = nunciod::grpc::serve_on_listener(
            listener,
            Arc::new(EventBus::new()),
            Arc::new(db),
            filter_engine,
            secrets,
            token,
        )
        .await;
    });
    addr
}

/// Runs `system status` through the reference CLI against `addr`.
async fn run_status(runner: &HeadlessRunner) -> String {
    runner
        .execute_command(
            &Commands::System {
                action: SystemSubcommand::Status,
            },
            true,
        )
        .await
}

/// An explicitly supplied token must authenticate on its own, with the runner's
/// vault holding a *different* token -- which is only possible if the override
/// short-circuits the keyring entirely.
#[tokio::test]
async fn env_override_authenticates_without_consulting_the_vault() {
    let daemon_secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        daemon_secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("mock vault mints gRPC bearer token"),
    );
    let addr = boot_daemon(Arc::clone(&daemon_secrets), token.clone()).await;

    // A vault the CLI could only fail with: an independent mock mints an
    // unrelated token.
    let unrelated_vault = Arc::new(SecretManager::mock());
    let unrelated_token = hex::encode(
        unrelated_vault
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("mock vault mints gRPC bearer token"),
    );
    assert_ne!(unrelated_token, token);

    let runner =
        HeadlessRunner::connect_with_token_override(unrelated_vault, addr.to_string(), Some(token));

    let out = run_status(&runner).await;
    assert!(
        out.contains(r#""status":"ok""#),
        "override should authenticate, got: {out}"
    );
}

/// With no override the CLI must behave exactly as before: read the token from
/// the injected vault and authenticate with it.
#[tokio::test]
async fn keyring_path_is_unchanged_when_no_override_is_set() {
    let secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("mock vault mints gRPC bearer token"),
    );
    let addr = boot_daemon(Arc::clone(&secrets), token).await;

    let runner = HeadlessRunner::connect_with(secrets, addr.to_string());

    let out = run_status(&runner).await;
    assert!(
        out.contains(r#""status":"ok""#),
        "vault-resolved token should authenticate, got: {out}"
    );
}

// ---- Telemetry capture: the token may never appear in logs. ----

#[derive(Default)]
struct Sink(Mutex<Vec<String>>);

impl Sink {
    fn push_all(&self, values: Vec<String>) {
        if let Ok(mut guard) = self.0.lock() {
            guard.extend(values);
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.0.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

struct FieldVisitor(Vec<String>);

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push(format!("{}={}", field.name(), value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push(format!("{}={:?}", field.name(), value));
    }
}

struct CaptureLayer(Arc<Sink>);

impl<S> tracing_subscriber::Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor(vec![format!("span={}", attrs.metadata().name())]);
        attrs.record(&mut visitor);
        self.0.push_all(visitor.0);
    }

    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor(vec![format!("event={}", event.metadata().target())]);
        event.record(&mut visitor);
        self.0.push_all(visitor.0);
    }
}

/// One process-wide capturing subscriber, installed once: a global subscriber
/// can only be set once per test binary and every test here shares it.
fn capture_sink() -> Arc<Sink> {
    static SINK: OnceLock<Arc<Sink>> = OnceLock::new();
    Arc::clone(SINK.get_or_init(|| {
        let sink = Arc::new(Sink::default());
        let subscriber = tracing_subscriber::registry().with(CaptureLayer(Arc::clone(&sink)));
        let _ = tracing::subscriber::set_global_default(subscriber);
        sink
    }))
}

/// A wrong override must fail honestly rather than hang or fabricate a status,
/// and the token must never reach the command output or the telemetry.
#[tokio::test]
async fn rejected_override_never_echoes_the_token_to_output_or_logs() {
    let sink = capture_sink();

    let daemon_secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        daemon_secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("mock vault mints gRPC bearer token"),
    );
    let addr = boot_daemon(Arc::clone(&daemon_secrets), token).await;

    let runner = HeadlessRunner::connect_with_token_override(
        Arc::new(SecretManager::mock()),
        addr.to_string(),
        Some(SENTINEL_TOKEN.to_string()),
    );

    let out = run_status(&runner).await;
    assert!(
        out.contains(r#""status":"error""#),
        "a wrong token must be rejected, got: {out}"
    );
    assert!(
        !out.contains(SENTINEL_TOKEN),
        "the bearer token must never be echoed in command output: {out}"
    );

    // Proves the capture is live, so the absence assertion below cannot pass
    // by capturing nothing at all.
    tracing::info!(marker = "headless-cli-token-capture-live", "capture probe");

    let captured = sink.snapshot();
    assert!(
        captured
            .iter()
            .any(|line| line.contains("headless-cli-token-capture-live")),
        "the capturing subscriber must be recording telemetry"
    );
    for line in &captured {
        assert!(
            !line.contains(SENTINEL_TOKEN),
            "the bearer token leaked into telemetry: {line}"
        );
    }
}
