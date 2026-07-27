//! gRPC server wiring for `nunciod`: serves the versioned `nuncio.v1.System`
//! contract (see `nuncio-proto`, backlog story 1.A.1 / GH-148) over loopback,
//! authenticated by a bearer token sourced from the OS keyring vault
//! (`nuncio_store::vault::GRPC_TOKEN_ACCOUNT`, backlog story 1.A.2 / GH-149).
//!
//! This runs ALONGSIDE the existing hand-rolled JSON-RPC IPC server
//! (`nuncio_core::ipc::IpcDaemonServer`); migrating callers off the JSON-RPC
//! transport is out of scope here and lands in a later story.

use nuncio_core::{CoreEvent, EventBus};
use nuncio_proto::v1::accounts_server::{Accounts, AccountsServer};
use nuncio_proto::v1::event::Kind;
use nuncio_proto::v1::system_server::{System, SystemServer};
use nuncio_proto::v1::{
    AccountConfig as AccountConfigProto, AccountProtocol as AccountProtocolProto,
    AddAccountRequest, AddAccountResponse, BatchFilterProgress, DatabaseRecovered, Event,
    EventError, FilterExecuted, GetStatusRequest, GetStatusResponse, ListAccountsRequest,
    ListAccountsResponse, MessageFlagsChanged, ShuttingDown, SubscribeRequest, SyncCompleted,
    SyncStarted, TlsMode as TlsModeProto, UpdateAvailable,
};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::SecretManager;
use std::pin::Pin;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, TcpListenerStream};
use tokio_stream::{Stream, StreamExt};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

// The gRPC loopback address defaults/resolver live in `nuncio-proto` (see
// `nuncio_proto::addr`) so that thin presentation-shell clients (e.g.
// `nuncio-cli`) agree on the same default address without depending on this
// `nunciod` binary crate. Re-exported here so existing callers of
// `nunciod::grpc::{DEFAULT_GRPC_ADDR, GRPC_ADDR_ENV_VAR, grpc_addr_from_env}`
// keep working unchanged (backlog story 1.A.3 / GH-150).
pub use nuncio_proto::{grpc_addr_from_env, DEFAULT_GRPC_ADDR, GRPC_ADDR_ENV_VAR};

/// Errors that can occur while binding or running the `nuncio.v1` gRPC server.
#[derive(Debug, Error)]
pub enum GrpcServeError {
    /// Failed to bind the loopback TCP listener.
    #[error("failed to bind gRPC listener on {addr}: {source}")]
    Bind {
        /// The address that failed to bind.
        addr: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// The tonic transport server failed while serving requests.
    #[error("gRPC transport server failed: {0}")]
    Transport(#[from] tonic::transport::Error),
}

/// Maps a `nuncio_core::CoreEvent` domain event onto its wire-format
/// `nuncio.v1.Event` representation, faithfully carrying every variant's
/// fields into the matching `oneof` case (see `proto/nuncio/v1/nuncio.proto`
/// for the wire contract, backlog story 1.A.4 / GH-151).
///
/// `usize` fields (`processed`, `total`, `matched`, `salvaged_rules_count`)
/// are narrowed to `u64` for the wire; these are in-process counters that
/// cannot realistically approach `u64::MAX`, and protobuf has no native
/// `usize` type.
fn map_core_event(event: CoreEvent) -> Event {
    let kind = match event {
        CoreEvent::SyncStarted { account_id } => Kind::SyncStarted(SyncStarted { account_id }),
        CoreEvent::SyncCompleted { account_id } => {
            Kind::SyncCompleted(SyncCompleted { account_id })
        }
        CoreEvent::MessageFlagsChanged { message_id, read } => {
            Kind::MessageFlagsChanged(MessageFlagsChanged { message_id, read })
        }
        CoreEvent::FilterExecuted {
            rule_id,
            message_id,
            action_taken,
        } => Kind::FilterExecuted(FilterExecuted {
            rule_id,
            message_id,
            action_taken,
        }),
        CoreEvent::BatchFilterProgress {
            processed,
            total,
            matched,
        } => Kind::BatchFilterProgress(BatchFilterProgress {
            processed: processed as u64,
            total: total as u64,
            matched: matched as u64,
        }),
        CoreEvent::DatabaseRecovered {
            backup_path,
            salvaged_rules_count,
            resync_triggered,
        } => Kind::DatabaseRecovered(DatabaseRecovered {
            backup_path,
            salvaged_rules_count: salvaged_rules_count as u64,
            resync_triggered,
        }),
        CoreEvent::UpdateAvailable {
            version,
            release_notes,
        } => Kind::UpdateAvailable(UpdateAvailable {
            version,
            release_notes,
        }),
        CoreEvent::Error { message } => Kind::Error(EventError { message }),
        CoreEvent::ShuttingDown => Kind::ShuttingDown(ShuttingDown {}),
    };
    Event { kind: Some(kind) }
}

/// `nuncio.v1.System` gRPC service implementation backed by the daemon's live
/// [`EventBus`] state.
struct SystemGrpcService {
    event_bus: Arc<EventBus>,
}

#[tonic::async_trait]
impl System for SystemGrpcService {
    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let state = self.event_bus.current_state();
        Ok(Response::new(GetStatusResponse {
            engine_status: format!("{:?}", state.status),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    /// Server-streaming push feed of daemon domain events (backlog story
    /// 1.A.4 / GH-151). Defined on `System` (not a separate service) so this
    /// RPC is automatically covered by the same `BearerAuthInterceptor` that
    /// guards `GetStatus`, rather than risking a second, un-intercepted
    /// service being mounted by mistake (see GH #165).
    ///
    /// The subscription is registered (`event_bus.subscribe_events()`)
    /// synchronously before this method returns, so once a client's
    /// `subscribe` call resolves, any event published afterwards is
    /// guaranteed to be observed on the returned stream — no fixed sleep is
    /// needed by callers to avoid a race between "start subscribing" and
    /// "event published".
    ///
    /// A lagging subscriber (`BroadcastStreamRecvError::Lagged`) is handled
    /// by logging a warning and skipping to the next available event rather
    /// than terminating the stream or panicking; the daemon's event bus is
    /// a best-effort broadcast feed, not a durable log, so a slow subscriber
    /// missing some events is preferable to it never recovering. The stream
    /// ends (returns `None`) only when the event bus itself is closed (all
    /// `EventBus` senders dropped) or the client disconnects.
    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<Event, Status>> + Send + 'static>>;

    async fn subscribe(
        &self,
        _request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let receiver = self.event_bus.subscribe_events();
        let mapped = BroadcastStream::new(receiver).filter_map(|item| match item {
            Ok(core_event) => Some(Ok(map_core_event(core_event))),
            Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                tracing::warn!(
                    "gRPC Subscribe stream lagged behind the daemon event bus; skipped {} \
                     event(s); resuming from the next available event",
                    skipped
                );
                None
            }
        });
        Ok(Response::new(Box::pin(mapped)))
    }
}

/// Maps a `nuncio_core::AccountProtocol` onto its wire-format
/// `nuncio.v1.AccountProtocol` enum value.
fn map_account_protocol_to_proto(protocol: nuncio_core::AccountProtocol) -> AccountProtocolProto {
    match protocol {
        nuncio_core::AccountProtocol::Jmap => AccountProtocolProto::Jmap,
        nuncio_core::AccountProtocol::ImapSmtp => AccountProtocolProto::ImapSmtp,
    }
}

/// Maps a wire-format `nuncio.v1.AccountProtocol` enum value back onto
/// `nuncio_core::AccountProtocol`. `Unspecified` is treated as an invalid
/// request rather than silently defaulting to a protocol the caller never
/// asked for.
fn map_account_protocol_from_proto(
    protocol: AccountProtocolProto,
) -> Result<nuncio_core::AccountProtocol, Status> {
    match protocol {
        AccountProtocolProto::Jmap => Ok(nuncio_core::AccountProtocol::Jmap),
        AccountProtocolProto::ImapSmtp => Ok(nuncio_core::AccountProtocol::ImapSmtp),
        AccountProtocolProto::Unspecified => {
            Err(Status::invalid_argument("account protocol is required"))
        }
    }
}

/// Maps a `nuncio_core::TlsMode` onto its wire-format `nuncio.v1.TlsMode`
/// enum value.
fn map_tls_mode_to_proto(mode: nuncio_core::TlsMode) -> TlsModeProto {
    match mode {
        nuncio_core::TlsMode::ImplicitTls => TlsModeProto::ImplicitTls,
        nuncio_core::TlsMode::StartTls => TlsModeProto::StartTls,
        nuncio_core::TlsMode::Plain => TlsModeProto::Plain,
    }
}

/// Maps a wire-format `nuncio.v1.TlsMode` enum value back onto
/// `nuncio_core::TlsMode`. `Unspecified` is rejected rather than silently
/// defaulting.
fn map_tls_mode_from_proto(mode: TlsModeProto) -> Result<nuncio_core::TlsMode, Status> {
    match mode {
        TlsModeProto::ImplicitTls => Ok(nuncio_core::TlsMode::ImplicitTls),
        TlsModeProto::StartTls => Ok(nuncio_core::TlsMode::StartTls),
        TlsModeProto::Plain => Ok(nuncio_core::TlsMode::Plain),
        TlsModeProto::Unspecified => Err(Status::invalid_argument("tls mode is required")),
    }
}

/// Maps a `nuncio_core::AccountConfig` onto its wire-format
/// `nuncio.v1.AccountConfig` representation for `ListAccounts` responses.
/// The password credential is NEVER part of `AccountConfig` (it lives only
/// in the OS keyring, keyed by `keyring_secret_key`), so there is nothing to
/// scrub here -- there is simply no field to carry it.
fn map_account_config_to_proto(config: nuncio_core::AccountConfig) -> AccountConfigProto {
    AccountConfigProto {
        id: config.id,
        name: config.name,
        email_address: config.email_address,
        protocol: map_account_protocol_to_proto(config.protocol).into(),
        server_host: config.server_host,
        server_port: u32::from(config.server_port),
        use_tls: config.use_tls,
        imap_tls_mode: map_tls_mode_to_proto(config.imap_tls_mode).into(),
        smtp_tls_mode: map_tls_mode_to_proto(config.smtp_tls_mode).into(),
        keyring_secret_key: config.keyring_secret_key,
        sync_interval_secs: config.sync_interval_secs,
    }
}

/// Maps a wire-format `nuncio.v1.AccountConfig` request payload back onto
/// `nuncio_core::AccountConfig`. Rejects a `server_port` outside `1..=65535`
/// with `Status::invalid_argument` rather than silently truncating it.
fn map_account_config_from_proto(
    config: AccountConfigProto,
) -> Result<nuncio_core::AccountConfig, Status> {
    let protocol = map_account_protocol_from_proto(config.protocol())?;
    let imap_tls_mode = map_tls_mode_from_proto(config.imap_tls_mode())?;
    let smtp_tls_mode = map_tls_mode_from_proto(config.smtp_tls_mode())?;
    let server_port = u16::try_from(config.server_port)
        .map_err(|_| Status::invalid_argument("server_port must be in range 1..=65535"))?;

    Ok(nuncio_core::AccountConfig {
        id: config.id,
        name: config.name,
        email_address: config.email_address,
        protocol,
        server_host: config.server_host,
        server_port,
        use_tls: config.use_tls,
        imap_tls_mode,
        smtp_tls_mode,
        keyring_secret_key: config.keyring_secret_key,
        sync_interval_secs: config.sync_interval_secs,
    })
}

/// `nuncio.v1.Accounts` gRPC service implementation backed by the daemon's
/// live [`DatabaseEngine`] and [`SecretManager`] vault (backlog stories
/// 1.C.1 / 1.C.2, GH #156 / GH #157).
///
/// Persists [`nuncio_core::AccountConfig`] rows via `DatabaseEngine::save_account`
/// and routes the password credential exclusively to the OS keyring vault via
/// `SecretManager::set_secret` -- the password is never written to SQLite,
/// never logged, and never echoed back in any RPC response.
struct AccountsGrpcService {
    db: Arc<DatabaseEngine>,
    secrets: Arc<SecretManager>,
}

#[tonic::async_trait]
impl Accounts for AccountsGrpcService {
    async fn add_account(
        &self,
        request: Request<AddAccountRequest>,
    ) -> Result<Response<AddAccountResponse>, Status> {
        let req = request.into_inner();
        let proto_config = req
            .config
            .ok_or_else(|| Status::invalid_argument("config is required"))?;
        if req.password.is_empty() {
            return Err(Status::invalid_argument("password is required"));
        }

        let config = map_account_config_from_proto(proto_config)?;
        config
            .validate()
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        // Store the password credential in the OS keyring FIRST, keyed by
        // this account's `keyring_secret_key` -- it is never written to
        // SQLite. If this fails, the account row below is deliberately
        // never saved, so a failed credential write can never leave behind
        // an account config with no retrievable password.
        self.secrets
            .set_secret(&config.keyring_secret_key, &req.password)
            .map_err(|e| Status::internal(format!("failed to store credential in vault: {e}")))?;

        self.db
            .save_account(&config)
            .await
            .map_err(|e| Status::internal(format!("failed to persist account: {e}")))?;

        Ok(Response::new(AddAccountResponse { id: config.id }))
    }

    async fn list_accounts(
        &self,
        _request: Request<ListAccountsRequest>,
    ) -> Result<Response<ListAccountsResponse>, Status> {
        let accounts = self
            .db
            .list_accounts()
            .await
            .map_err(|e| Status::internal(format!("failed to list accounts: {e}")))?
            .into_iter()
            .map(map_account_config_to_proto)
            .collect();

        Ok(Response::new(ListAccountsResponse { accounts }))
    }
}

/// Bearer-token authentication interceptor for the loopback `nuncio.v1` gRPC
/// server.
///
/// Requires an `authorization: Bearer <token>` metadata header on every call
/// and rejects missing, malformed, or mismatched tokens with
/// `Status::unauthenticated`. The comparison against the expected token uses
/// [`subtle::ConstantTimeEq`] so a mismatching guess cannot be distinguished
/// by timing. The token itself is never logged.
#[derive(Clone)]
struct BearerAuthInterceptor {
    expected_token: Arc<str>,
}

impl BearerAuthInterceptor {
    fn new(expected_token: impl Into<Arc<str>>) -> Self {
        Self {
            expected_token: expected_token.into(),
        }
    }
}

impl tonic::service::Interceptor for BearerAuthInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let header = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("missing authorization metadata"))?;
        let header_str = header
            .to_str()
            .map_err(|_| Status::unauthenticated("authorization metadata is not valid ASCII"))?;
        let provided = header_str.strip_prefix("Bearer ").ok_or_else(|| {
            Status::unauthenticated("expected 'Bearer <token>' authorization scheme")
        })?;

        let matches: bool = provided
            .as_bytes()
            .ct_eq(self.expected_token.as_bytes())
            .into();
        if matches {
            Ok(request)
        } else {
            Err(Status::unauthenticated("invalid bearer token"))
        }
    }
}

/// Binds a loopback TCP listener at `addr` and serves the `nuncio.v1.System`
/// and `nuncio.v1.Accounts` gRPC services on it, both authenticated by
/// `token`, until the transport server errors.
///
/// `addr` MUST be a loopback address (e.g. `127.0.0.1:PORT`); callers are
/// responsible for passing loopback-only addresses (see
/// [`grpc_addr_from_env`]).
pub async fn serve(
    addr: &str,
    event_bus: Arc<EventBus>,
    db: Arc<DatabaseEngine>,
    secrets: Arc<SecretManager>,
    token: impl Into<Arc<str>>,
) -> Result<(), GrpcServeError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| GrpcServeError::Bind {
            addr: addr.to_string(),
            source,
        })?;
    serve_on_listener(listener, event_bus, db, secrets, token).await
}

/// Serves the `nuncio.v1.System` and `nuncio.v1.Accounts` gRPC services on
/// an already-bound [`TcpListener`].
///
/// # Security (GH #165)
///
/// EVERY service mounted on this server MUST be wrapped in its own
/// [`BearerAuthInterceptor`] via `*Server::with_interceptor`, exactly like
/// `System` and `Accounts` below -- never `add_service(SomeServer::new(...))`
/// unwrapped. This is a hard, non-negotiable invariant: an un-intercepted
/// service mounted here would be reachable by any local process without
/// authentication. If a future story adds another service, mount it the
/// same way.
///
/// Exposed separately from [`serve`] so tests can bind an ephemeral loopback
/// port (`127.0.0.1:0`), read back the OS-assigned port via
/// `TcpListener::local_addr`, and connect a client to it deterministically:
/// once `TcpListener::bind` returns, the OS is already accepting connections
/// on that port, so no fixed sleep is needed before dialing it.
pub async fn serve_on_listener(
    listener: TcpListener,
    event_bus: Arc<EventBus>,
    db: Arc<DatabaseEngine>,
    secrets: Arc<SecretManager>,
    token: impl Into<Arc<str>>,
) -> Result<(), GrpcServeError> {
    let token: Arc<str> = token.into();

    let system_service = SystemGrpcService { event_bus };
    let system_interceptor = BearerAuthInterceptor::new(token.clone());
    let system_svc = SystemServer::with_interceptor(system_service, system_interceptor);

    let accounts_service = AccountsGrpcService { db, secrets };
    let accounts_interceptor = BearerAuthInterceptor::new(token);
    let accounts_svc = AccountsServer::with_interceptor(accounts_service, accounts_interceptor);

    Server::builder()
        .add_service(system_svc)
        .add_service(accounts_svc)
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await
        .map_err(GrpcServeError::Transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::{CoreCommand, EngineStatus};
    use nuncio_proto::v1::accounts_client::AccountsClient;
    use nuncio_proto::v1::system_client::SystemClient;
    use tonic::Code;

    /// Spawns a test server backed by a fresh ephemeral database and a
    /// fresh [`SecretManager::mock`] vault -- the right default for every
    /// test that only exercises `System` or doesn't care about pre-existing
    /// account/keyring state. Tests that DO care (persistence-across-restart,
    /// credential-secrecy) build their own `db`/`secrets` and call
    /// [`spawn_test_server_with`] directly instead.
    async fn spawn_test_server(
        event_bus: Arc<EventBus>,
        token: &str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let secrets = Arc::new(SecretManager::mock());
        spawn_test_server_with(event_bus, Arc::new(db), secrets, token).await
    }

    /// Spawns a test server on an ephemeral loopback port backed by the
    /// given `db` and `secrets`, so tests can share (and re-open) the same
    /// database path / mock keyring state across multiple server instances.
    async fn spawn_test_server_with(
        event_bus: Arc<EventBus>,
        db: Arc<DatabaseEngine>,
        secrets: Arc<SecretManager>,
        token: &str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        let token = token.to_string();
        let handle = tokio::spawn(async move {
            let _ = serve_on_listener(listener, event_bus, db, secrets, token).await;
        });
        (addr, handle)
    }

    #[test]
    fn grpc_addr_from_env_defaults_when_unset() {
        // Serialize access to the shared process environment variable so
        // this test cannot interleave with any other test touching it.
        static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());

        std::env::remove_var(GRPC_ADDR_ENV_VAR);
        assert_eq!(grpc_addr_from_env(), DEFAULT_GRPC_ADDR);

        std::env::set_var(GRPC_ADDR_ENV_VAR, "127.0.0.1:12345");
        assert_eq!(grpc_addr_from_env(), "127.0.0.1:12345");
        std::env::remove_var(GRPC_ADDR_ENV_VAR);
    }

    #[test]
    fn grpc_serve_error_messages_do_not_leak_and_are_displayable() {
        let bind_err = GrpcServeError::Bind {
            addr: "127.0.0.1:9420".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::AddrInUse, "in use"),
        };
        assert!(bind_err.to_string().contains("127.0.0.1:9420"));
    }

    #[tokio::test]
    async fn get_status_rejects_missing_bearer_token() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .get_status(GetStatusRequest {})
            .await
            .expect_err("missing token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_status_rejects_wrong_bearer_token() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let mut request = Request::new(GetStatusRequest {});
        request.metadata_mut().insert(
            "authorization",
            "Bearer wrong-token"
                .parse()
                .expect("valid ascii metadata value"),
        );
        let err = client
            .get_status(request)
            .await
            .expect_err("wrong token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_status_rejects_malformed_authorization_header() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let mut request = Request::new(GetStatusRequest {});
        request.metadata_mut().insert(
            "authorization",
            "not-a-bearer-scheme"
                .parse()
                .expect("valid ascii metadata value"),
        );
        let err = client
            .get_status(request)
            .await
            .expect_err("malformed scheme must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_status_accepts_correct_bearer_token_and_reports_live_state() {
        let event_bus = Arc::new(EventBus::new());
        event_bus.process_command(CoreCommand::SyncAll);
        assert_eq!(event_bus.current_state().status, EngineStatus::Syncing);

        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let mut request = Request::new(GetStatusRequest {});
        request.metadata_mut().insert(
            "authorization",
            "Bearer correct-token"
                .parse()
                .expect("valid ascii metadata value"),
        );
        let response = client
            .get_status(request)
            .await
            .expect("valid token is accepted")
            .into_inner();
        assert_eq!(response.engine_status, "Syncing");
        assert_eq!(response.version, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn serve_binds_and_starts_on_a_valid_loopback_address() {
        // Exercises the `serve()` wrapper's happy path (bind succeeds, then
        // delegates into `serve_on_listener`), as distinct from the
        // `spawn_test_server` helper above which pre-binds the listener
        // itself and calls `serve_on_listener` directly.
        let event_bus = Arc::new(EventBus::new());
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let secrets = Arc::new(SecretManager::mock());
        let _handle = tokio::spawn(async move {
            let _ = serve("127.0.0.1:0", event_bus, Arc::new(db), secrets, "token").await;
        });
        // Yield so the spawned task is polled at least once and actually
        // reaches the bind + delegate-to-`serve_on_listener` call before
        // this test function returns.
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn serve_fails_closed_when_bind_address_is_invalid() {
        let event_bus = Arc::new(EventBus::new());
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let secrets = Arc::new(SecretManager::mock());
        let err = serve(
            "not-a-valid-addr",
            event_bus,
            Arc::new(db),
            secrets,
            "token",
        )
        .await
        .expect_err("invalid bind address must fail");
        assert!(matches!(err, GrpcServeError::Bind { .. }));
    }

    fn authed_bearer_request<T>(payload: T) -> Request<T> {
        let mut request = Request::new(payload);
        request.metadata_mut().insert(
            "authorization",
            "Bearer correct-token"
                .parse()
                .expect("valid ascii metadata value"),
        );
        request
    }

    #[tokio::test]
    async fn subscribe_streams_mapped_core_events_from_the_live_event_bus() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus.clone(), "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        // `client.subscribe(..).await` only resolves once the server-side
        // `subscribe` method has returned `Response::new(stream)`, and that
        // method registers `event_bus.subscribe_events()` before returning.
        // So once this `.await` completes, the subscription is guaranteed
        // to already be live: publishing an event afterwards cannot race
        // ahead of the subscriber registering, and no fixed sleep is
        // needed to make this deterministic.
        let mut stream = client
            .subscribe(authed_bearer_request(SubscribeRequest {}))
            .await
            .expect("subscribe is accepted for a valid bearer token")
            .into_inner();

        event_bus.process_command(CoreCommand::SyncAccount {
            account_id: "acct-42".to_string(),
        });

        let event = stream
            .message()
            .await
            .expect("stream yields without a transport error")
            .expect("stream produces an event rather than ending");

        match event.kind {
            Some(Kind::SyncStarted(SyncStarted { account_id })) => {
                assert_eq!(account_id, Some("acct-42".to_string()));
            }
            other => panic!("expected a mapped SyncStarted event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn subscribe_rejects_missing_bearer_token() {
        // Confirms `Subscribe` inherits the same `BearerAuthInterceptor` as
        // `GetStatus` because both are defined on the single `System`
        // service (GH #165: no separate, un-intercepted service).
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .subscribe(SubscribeRequest {})
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn concurrent_unary_and_streaming_calls_do_not_interleave_or_corrupt() {
        // Regression test for the property the old hand-rolled JSON-RPC IPC
        // transport lacked: a response/notification demux race that could
        // corrupt reads when a streamed notification and a unary response
        // were in flight at the same time. gRPC/HTTP2 multiplexes streaming
        // and unary calls on independent logical streams, so this asserts
        // both an active event subscription and many concurrent unary
        // `GetStatus` calls complete correctly and without corrupting one
        // another.
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus.clone(), "correct-token").await;

        // Open (and await) the subscription first, on its own connection,
        // so the server-side registration has deterministically happened
        // before any event below is published (see the comment in the
        // single-event test above for why this ordering is race-free).
        let mut sub_client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("subscribe client connects");
        let mut stream = sub_client
            .subscribe(authed_bearer_request(SubscribeRequest {}))
            .await
            .expect("subscribe is accepted for a valid bearer token")
            .into_inner();

        const ROUNDS: usize = 20;
        let mut unary_client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("unary client connects");
        let unary_task = tokio::spawn(async move {
            let mut responses = Vec::with_capacity(ROUNDS);
            for _ in 0..ROUNDS {
                let response = unary_client
                    .get_status(authed_bearer_request(GetStatusRequest {}))
                    .await
                    .expect("unary call succeeds")
                    .into_inner();
                responses.push(response);
            }
            responses
        });

        for i in 0..ROUNDS {
            event_bus.process_command(CoreCommand::SyncAccount {
                account_id: format!("acct-{i}"),
            });
        }

        let unary_responses = unary_task.await.expect("unary task does not panic");
        assert_eq!(unary_responses.len(), ROUNDS);
        for response in &unary_responses {
            // Every unary response must be a well-formed, uncorrupted
            // `GetStatusResponse`: the correct build version, and a status
            // string matching a real `EngineStatus` variant -- never bytes
            // bled in from the concurrently-running event stream.
            assert_eq!(response.version, env!("CARGO_PKG_VERSION"));
            assert!(
                matches!(
                    response.engine_status.as_str(),
                    "Idle" | "Syncing" | "ShuttingDown"
                ),
                "unexpected/corrupted engine_status: {:?}",
                response.engine_status
            );
        }

        for i in 0..ROUNDS {
            let event = stream
                .message()
                .await
                .expect("stream yields without a transport error")
                .expect("stream produces an event rather than ending early");
            match event.kind {
                Some(Kind::SyncStarted(SyncStarted { account_id })) => {
                    assert_eq!(account_id, Some(format!("acct-{i}")));
                }
                other => panic!("expected mapped SyncStarted(acct-{i}) event, got {other:?}"),
            }
        }
    }

    fn sample_account_config_proto(id: &str, keyring_secret_key: &str) -> AccountConfigProto {
        AccountConfigProto {
            id: id.to_string(),
            name: "Restart Test Account".to_string(),
            email_address: format!("{id}@nuncio.mx"),
            protocol: AccountProtocolProto::ImapSmtp.into(),
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            use_tls: true,
            imap_tls_mode: TlsModeProto::ImplicitTls.into(),
            smtp_tls_mode: TlsModeProto::ImplicitTls.into(),
            keyring_secret_key: keyring_secret_key.to_string(),
            sync_interval_secs: 60,
        }
    }

    #[tokio::test]
    async fn add_account_rejects_missing_bearer_token() {
        // Confirms `Accounts` is mounted behind its own `BearerAuthInterceptor`
        // exactly like `System` (GH #165: no un-intercepted service).
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .add_account(AddAccountRequest {
                config: Some(sample_account_config_proto(
                    "acct-noauth",
                    "nuncio/acct-noauth",
                )),
                password: "irrelevant".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn list_accounts_rejects_missing_bearer_token() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .list_accounts(ListAccountsRequest {})
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn add_account_rejects_empty_password() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .add_account(authed_bearer_request(AddAccountRequest {
                config: Some(sample_account_config_proto("acct-nopw", "nuncio/acct-nopw")),
                password: String::new(),
            }))
            .await
            .expect_err("empty password must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn add_account_rejects_missing_config() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .add_account(authed_bearer_request(AddAccountRequest {
                config: None,
                password: "some-password".to_string(),
            }))
            .await
            .expect_err("missing config must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    /// Backlog story 1.C.1 (GH #156): proves an account added through the
    /// daemon persists across a daemon restart. Simulates the restart by
    /// dropping the first `DatabaseEngine`/server pair and opening a brand
    /// new one at the SAME on-disk database path (a real persistent temp
    /// file, not `connect_ephemeral`'s throwaway one), then calling
    /// `ListAccounts` against the second server instance.
    #[tokio::test]
    async fn add_account_persists_across_a_simulated_daemon_restart() {
        let dir = tempfile::tempdir().expect("create persistent temp dir");
        let db_path = dir.path().join("accounts_restart_test.db");
        let secrets = Arc::new(SecretManager::mock());

        // "First daemon run".
        let db_first_run = Arc::new(
            DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .expect("open db on first run"),
        );
        let (addr, _handle) = spawn_test_server_with(
            Arc::new(EventBus::new()),
            db_first_run.clone(),
            secrets.clone(),
            "correct-token",
        )
        .await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let add_response = client
            .add_account(authed_bearer_request(AddAccountRequest {
                config: Some(sample_account_config_proto(
                    "acct-restart-1",
                    "nuncio/acct-restart-1",
                )),
                password: "restart-secret-pw".to_string(),
            }))
            .await
            .expect("add_account succeeds")
            .into_inner();
        assert_eq!(add_response.id, "acct-restart-1");

        db_first_run.close().await;

        // "Second daemon run": a brand new `DatabaseEngine`, opened at the
        // exact same `db_path`, sharing the SAME mock `SecretManager` (so
        // the previously-provisioned keyring entry is still readable, just
        // like a real OS keyring survives a daemon restart).
        let db_second_run = Arc::new(
            DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .expect("re-open db on second run"),
        );
        let (addr2, _handle2) = spawn_test_server_with(
            Arc::new(EventBus::new()),
            db_second_run,
            secrets.clone(),
            "correct-token",
        )
        .await;

        let mut client2 = AccountsClient::connect(format!("http://{addr2}"))
            .await
            .expect("client connects");
        let list_response = client2
            .list_accounts(authed_bearer_request(ListAccountsRequest {}))
            .await
            .expect("list_accounts succeeds")
            .into_inner();

        assert_eq!(list_response.accounts.len(), 1);
        let restored = &list_response.accounts[0];
        assert_eq!(restored.id, "acct-restart-1");
        assert_eq!(restored.email_address, "acct-restart-1@nuncio.mx");
        assert_eq!(restored.keyring_secret_key, "nuncio/acct-restart-1");

        // The credential itself must have survived too, in the (mock)
        // keyring -- never in the database.
        let stored_password = secrets
            .get_secret("nuncio/acct-restart-1")
            .expect("password still retrievable from vault after restart");
        assert_eq!(stored_password, "restart-secret-pw");
    }

    /// Backlog story 1.C.2 (GH #157): proves the password credential is
    /// retrievable from the (mock) keyring, but never appears in any
    /// `accounts` SQLite column, and never appears in the `ListAccounts`
    /// wire response.
    #[tokio::test]
    async fn add_account_password_never_touches_sqlite_or_the_list_response() {
        const PASSWORD: &str = "super-secret-credential-42";

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());
        let (addr, _handle) = spawn_test_server_with(
            Arc::new(EventBus::new()),
            db.clone(),
            secrets.clone(),
            "correct-token",
        )
        .await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        client
            .add_account(authed_bearer_request(AddAccountRequest {
                config: Some(sample_account_config_proto(
                    "acct-secrecy-1",
                    "nuncio/acct-secrecy-1",
                )),
                password: PASSWORD.to_string(),
            }))
            .await
            .expect("add_account succeeds");

        // The password IS retrievable from the (mock) keyring vault, keyed
        // by this account's `keyring_secret_key`.
        let stored_password = secrets
            .get_secret("nuncio/acct-secrecy-1")
            .expect("password stored in vault");
        assert_eq!(stored_password, PASSWORD);

        // The password is NEVER present in any `accounts` table column.
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            String,
            i64,
        )> = sqlx::query_as(
            "SELECT id, name, email_address, protocol, server_host, server_port, use_tls, \
                 keyring_secret_key, sync_interval_secs FROM accounts",
        )
        .fetch_all(db.pool())
        .await
        .expect("query accounts table directly");
        assert_eq!(rows.len(), 1);
        let (id, name, email_address, protocol, server_host, _port, _tls, keyring_key, _interval) =
            &rows[0];
        for text_column in [
            id.as_str(),
            name.as_str(),
            email_address.as_str(),
            protocol.as_str(),
            server_host.as_str(),
            keyring_key.as_str(),
        ] {
            assert!(
                !text_column.contains(PASSWORD),
                "password leaked into an accounts DB column: {text_column:?}"
            );
        }

        // The password is NEVER present in the `ListAccounts` response.
        let list_response = client
            .list_accounts(authed_bearer_request(ListAccountsRequest {}))
            .await
            .expect("list_accounts succeeds")
            .into_inner();
        assert_eq!(list_response.accounts.len(), 1);
        let response_debug = format!("{list_response:?}");
        assert!(
            !response_debug.contains(PASSWORD),
            "password leaked into the ListAccounts response: {response_debug}"
        );
    }
}
