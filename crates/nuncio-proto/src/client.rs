//! Shared client-side connection helpers for the `nuncio.v1` gRPC API.
//!
//! Any thin presentation-shell client (`nuncio-cli`, and future `nuncio-tui`
//! / `nuncio-gui` / `nuncio-mcp` clients) that needs to talk to the running
//! `nunciod` daemon over gRPC should go through [`connect_system`] rather
//! than hand-rolling channel setup and bearer-token metadata injection. This
//! keeps those crates free of any dependency on the `nunciod` binary crate
//! itself (backlog story 1.A.3 / GH-150) while still sharing a single
//! implementation of the client-side authentication handshake with the
//! server-side interceptor in `nunciod::grpc`.

use crate::v1::accounts_client::AccountsClient;
use crate::v1::system_client::SystemClient;
use crate::v1::{Event, SubscribeRequest};
use thiserror::Error;
use tonic::codec::Streaming;
use tonic::metadata::errors::InvalidMetadataValue;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status};

/// Errors that can occur while establishing an authenticated client
/// connection to the `nuncio.v1.System` gRPC service.
#[derive(Debug, Error)]
pub enum ConnectError {
    /// The provided bearer token could not be encoded as gRPC metadata
    /// (e.g. contained non-ASCII bytes).
    #[error("bearer token is not valid gRPC metadata: {0}")]
    InvalidToken(#[from] InvalidMetadataValue),
    /// `addr` could not be parsed into a valid gRPC endpoint.
    #[error("invalid gRPC endpoint 'http://{addr}': {source}")]
    InvalidEndpoint {
        /// The address that failed to parse.
        addr: String,
        /// Underlying tonic transport error.
        #[source]
        source: tonic::transport::Error,
    },
    /// The transport connection to `addr` could not be established.
    #[error("failed to connect to nunciod gRPC endpoint 'http://{addr}': {source}")]
    Transport {
        /// The address that could not be reached.
        addr: String,
        /// Underlying tonic transport error.
        #[source]
        source: tonic::transport::Error,
    },
}

/// Client-side interceptor that injects `authorization: Bearer <token>` into
/// the metadata of every outgoing request, mirroring the header shape
/// expected by the server-side `BearerAuthInterceptor` in `nunciod::grpc`.
///
/// The header value is parsed once at construction time (via [`Self::new`])
/// rather than on every call, so a malformed token fails fast when
/// connecting instead of surfacing as an opaque per-request error.
#[derive(Debug, Clone)]
pub struct BearerTokenInterceptor {
    header_value: MetadataValue<Ascii>,
}

impl BearerTokenInterceptor {
    /// Builds the interceptor from a raw bearer token (e.g. the hex-encoded
    /// value produced by `SecretManager::get_or_create_key_bytes` +
    /// `hex::encode`).
    pub fn new(token: &str) -> Result<Self, ConnectError> {
        let header_value = format!("Bearer {token}").parse::<MetadataValue<Ascii>>()?;
        Ok(Self { header_value })
    }
}

impl tonic::service::Interceptor for BearerTokenInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request
            .metadata_mut()
            .insert("authorization", self.header_value.clone());
        Ok(request)
    }
}

/// The authenticated `nuncio.v1.System` client type returned by
/// [`connect_system`].
pub type AuthenticatedSystemClient =
    SystemClient<InterceptedService<Channel, BearerTokenInterceptor>>;

/// The authenticated `nuncio.v1.Accounts` client type returned by
/// [`connect_accounts`].
pub type AuthenticatedAccountsClient =
    AccountsClient<InterceptedService<Channel, BearerTokenInterceptor>>;

/// Dials `addr` (a `host:port` pair, e.g. `127.0.0.1:9420`) over plain HTTP
/// (the loopback gRPC transport is never TLS-wrapped; auth is via bearer
/// token instead) and returns the connected [`Channel`], shared by every
/// per-service `connect_*` helper in this module so they agree on identical
/// endpoint-parsing and connection-error handling.
///
/// `addr` is expected to be a loopback address; this helper does not
/// enforce that itself, matching the daemon's own loopback-only bind
/// contract (see `nunciod::grpc::serve`).
async fn dial(addr: &str) -> Result<Channel, ConnectError> {
    let uri = format!("http://{addr}");
    let endpoint = Endpoint::from_shared(uri).map_err(|source| ConnectError::InvalidEndpoint {
        addr: addr.to_string(),
        source,
    })?;
    endpoint
        .connect()
        .await
        .map_err(|source| ConnectError::Transport {
            addr: addr.to_string(),
            source,
        })
}

/// Dials the `nuncio.v1.System` gRPC endpoint at `addr` and returns a client
/// that injects `authorization: Bearer <token>` metadata on every call.
pub async fn connect_system(
    addr: &str,
    token: &str,
) -> Result<AuthenticatedSystemClient, ConnectError> {
    let interceptor = BearerTokenInterceptor::new(token)?;
    let channel = dial(addr).await?;
    Ok(SystemClient::with_interceptor(channel, interceptor))
}

/// Dials the `nuncio.v1.Accounts` gRPC endpoint at `addr` and returns a
/// client that injects `authorization: Bearer <token>` metadata on every
/// call (backlog stories 1.C.1 / 1.C.2, GH #156 / GH #157).
///
/// `Accounts` is guarded by the exact same `BearerAuthInterceptor` as
/// `System` on the server side (see `nunciod::grpc::serve_on_listener`,
/// GH #165), so this shares [`BearerTokenInterceptor`] and [`dial`] with
/// [`connect_system`] rather than hand-rolling a second auth handshake.
pub async fn connect_accounts(
    addr: &str,
    token: &str,
) -> Result<AuthenticatedAccountsClient, ConnectError> {
    let interceptor = BearerTokenInterceptor::new(token)?;
    let channel = dial(addr).await?;
    Ok(AccountsClient::with_interceptor(channel, interceptor))
}

/// Opens the `nuncio.v1.System/Subscribe` server-streaming RPC on an already
/// authenticated client (as returned by [`connect_system`]), reusing the
/// bearer-token interceptor already attached to `client` rather than
/// requiring callers to re-authenticate. Returns the live event stream;
/// callers pull events with `Streaming::message` (or the `Stream`/`StreamExt`
/// combinators) until the daemon closes the stream or the connection drops.
///
/// This is the client-side counterpart of `nunciod::grpc`'s `Subscribe`
/// implementation (backlog story 1.A.4 / GH-151) and keeps thin
/// presentation-shell clients free of any dependency on the `nunciod`
/// binary crate itself.
pub async fn subscribe_events(
    client: &mut AuthenticatedSystemClient,
) -> Result<Streaming<Event>, Status> {
    let response = client.subscribe(SubscribeRequest {}).await?;
    Ok(response.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::service::Interceptor as _;

    #[test]
    fn bearer_token_interceptor_rejects_token_with_invalid_header_bytes() {
        // A raw newline is not a legal HTTP header value byte (unlike
        // accented/opaque high bytes, which `HeaderValue` actually permits
        // as legacy obs-text), so this deterministically exercises the
        // `InvalidToken` failure path.
        let err = BearerTokenInterceptor::new("tok\ntoken")
            .expect_err("token containing a control byte must fail to build a header value");
        assert!(matches!(err, ConnectError::InvalidToken(_)));
        assert!(err.to_string().contains("not valid gRPC metadata"));
    }

    #[test]
    fn bearer_token_interceptor_injects_authorization_header() {
        let mut interceptor =
            BearerTokenInterceptor::new("abc123").expect("valid ascii token builds");
        let request = Request::new(());
        let request = interceptor
            .call(request)
            .expect("interceptor injects header without error");
        let header = request
            .metadata()
            .get("authorization")
            .expect("authorization header present")
            .to_str()
            .expect("header is valid ascii");
        assert_eq!(header, "Bearer abc123");
    }

    #[tokio::test]
    async fn connect_system_fails_closed_on_invalid_token() {
        let err = connect_system("127.0.0.1:0", "tok\ntoken")
            .await
            .expect_err("invalid token must fail before dialing");
        assert!(matches!(err, ConnectError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn connect_accounts_fails_closed_on_invalid_token() {
        let err = connect_accounts("127.0.0.1:0", "tok\ntoken")
            .await
            .expect_err("invalid token must fail before dialing");
        assert!(matches!(err, ConnectError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn connect_accounts_reports_transport_error_when_daemon_unreachable() {
        let err = connect_accounts("127.0.0.1:1", "abc123")
            .await
            .expect_err("connecting to an unreachable daemon must fail");
        assert!(matches!(err, ConnectError::Transport { .. }));
        assert!(err.to_string().contains("127.0.0.1:1"));
    }

    #[tokio::test]
    async fn connect_system_reports_transport_error_when_daemon_unreachable() {
        // Port 0 alone is not connectable; using an explicit closed port on
        // loopback deterministically triggers connection refusal without a
        // live daemon, proving `connect_system` surfaces an honest
        // `ConnectError::Transport` rather than hanging or fabricating
        // success.
        let err = connect_system("127.0.0.1:1", "abc123")
            .await
            .expect_err("connecting to an unreachable daemon must fail");
        assert!(matches!(err, ConnectError::Transport { .. }));
        assert!(err.to_string().contains("127.0.0.1:1"));
    }

    #[tokio::test]
    async fn connect_system_reports_invalid_endpoint_for_a_malformed_address() {
        // An embedded space is not a legal URI authority character, so
        // `Endpoint::from_shared` rejects `http://<addr>` before any
        // connection attempt is made, exercising `InvalidEndpoint`
        // separately from the `Transport` (connection-refused) failure
        // mode above.
        let err = connect_system("127.0.0.1: 9420", "abc123")
            .await
            .expect_err("malformed address must fail to build a valid endpoint");
        assert!(matches!(err, ConnectError::InvalidEndpoint { .. }));
        assert!(err.to_string().contains("127.0.0.1: 9420"));
    }
}
