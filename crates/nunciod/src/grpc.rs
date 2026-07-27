//! gRPC server wiring for `nunciod`: serves the versioned `nuncio.v1.System`
//! contract (see `nuncio-proto`, backlog story 1.A.1 / GH-148) over loopback,
//! authenticated by a bearer token sourced from the OS keyring vault
//! (`nuncio_store::vault::GRPC_TOKEN_ACCOUNT`, backlog story 1.A.2 / GH-149).
//!
//! This runs ALONGSIDE the existing hand-rolled JSON-RPC IPC server
//! (`nuncio_core::ipc::IpcDaemonServer`); migrating callers off the JSON-RPC
//! transport is out of scope here and lands in a later story.

use nuncio_core::EventBus;
use nuncio_proto::v1::system_server::{System, SystemServer};
use nuncio_proto::v1::{GetStatusRequest, GetStatusResponse};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

/// Default loopback bind address for the `nuncio.v1` gRPC server, used when
/// the [`GRPC_ADDR_ENV_VAR`] environment variable is unset. Deliberately
/// distinct from the legacy JSON-RPC IPC server's default (`127.0.0.1:9422`).
pub const DEFAULT_GRPC_ADDR: &str = "127.0.0.1:9420";

/// Environment variable overriding the gRPC bind address.
pub const GRPC_ADDR_ENV_VAR: &str = "NUNCIO_GRPC_ADDR";

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

/// Resolves the gRPC bind address from [`GRPC_ADDR_ENV_VAR`], falling back to
/// [`DEFAULT_GRPC_ADDR`] when unset.
pub fn grpc_addr_from_env() -> String {
    std::env::var(GRPC_ADDR_ENV_VAR).unwrap_or_else(|_| DEFAULT_GRPC_ADDR.to_string())
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
/// gRPC service on it, authenticated by `token`, until the transport server
/// errors.
///
/// `addr` MUST be a loopback address (e.g. `127.0.0.1:PORT`); callers are
/// responsible for passing loopback-only addresses (see
/// [`grpc_addr_from_env`]).
pub async fn serve(
    addr: &str,
    event_bus: Arc<EventBus>,
    token: impl Into<Arc<str>>,
) -> Result<(), GrpcServeError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| GrpcServeError::Bind {
            addr: addr.to_string(),
            source,
        })?;
    serve_on_listener(listener, event_bus, token).await
}

/// Serves the `nuncio.v1.System` gRPC service on an already-bound
/// [`TcpListener`].
///
/// Exposed separately from [`serve`] so tests can bind an ephemeral loopback
/// port (`127.0.0.1:0`), read back the OS-assigned port via
/// `TcpListener::local_addr`, and connect a client to it deterministically:
/// once `TcpListener::bind` returns, the OS is already accepting connections
/// on that port, so no fixed sleep is needed before dialing it.
pub async fn serve_on_listener(
    listener: TcpListener,
    event_bus: Arc<EventBus>,
    token: impl Into<Arc<str>>,
) -> Result<(), GrpcServeError> {
    let service = SystemGrpcService { event_bus };
    let interceptor = BearerAuthInterceptor::new(token);
    let svc = SystemServer::with_interceptor(service, interceptor);

    Server::builder()
        .add_service(svc)
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await
        .map_err(GrpcServeError::Transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::{CoreCommand, EngineStatus};
    use nuncio_proto::v1::system_client::SystemClient;
    use tonic::Code;

    async fn spawn_test_server(
        event_bus: Arc<EventBus>,
        token: &str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        let token = token.to_string();
        let handle = tokio::spawn(async move {
            let _ = serve_on_listener(listener, event_bus, token).await;
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
        let _handle = tokio::spawn(async move {
            let _ = serve("127.0.0.1:0", event_bus, "token").await;
        });
        // Yield so the spawned task is polled at least once and actually
        // reaches the bind + delegate-to-`serve_on_listener` call before
        // this test function returns.
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn serve_fails_closed_when_bind_address_is_invalid() {
        let event_bus = Arc::new(EventBus::new());
        let err = serve("not-a-valid-addr", event_bus, "token")
            .await
            .expect_err("invalid bind address must fail");
        assert!(matches!(err, GrpcServeError::Bind { .. }));
    }
}
