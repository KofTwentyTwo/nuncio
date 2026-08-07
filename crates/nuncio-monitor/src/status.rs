//! Owns the authenticated gRPC channel to `nunciod` and feeds live daemon
//! state to the UI.
//!
//! [`EngineController`](crate::engine::EngineController) deliberately never
//! dials gRPC: its `liveness()` reports only what the advisory instance lock
//! knows, so its own unit tests never touch the network or the OS keyring.
//! That means a wedged daemon -- lock held, endpoint not answering -- is
//! indistinguishable from a healthy one *at that layer alone*. This module
//! is where the authenticated channel already lives, so it is the layer
//! that turns "lock held" plus "did `GetStatus` answer" into the composite
//! [`EngineState`] the UI actually renders (see [`composite_state`]).
//!
//! Two failure modes are handled deliberately rather than silently:
//!
//! - **No token in the vault** means `nunciod` has never run on this
//!   machine. [`StatusPoller::connect`] reports that as
//!   [`StatusError::NoToken`] via [`SecretManager::get_secret`] -- it never
//!   calls `get_or_create_key_bytes`, which would mint and persist a fresh
//!   token into the user's OS keyring merely because the monitor was
//!   opened.
//! - **A failed poll never reports zeros.** `status`/`health` on
//!   [`StatusUpdate`] are `Option`s that become `None` the moment the
//!   corresponding RPC cannot be completed, so the UI can render "unknown"
//!   distinctly from a genuine zero count.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nuncio_proto::client::{connect_accounts, connect_system, subscribe_events, ConnectError};
use nuncio_proto::v1::{
    AccountConfig, GetHealthRequest, GetHealthResponse, GetStatusRequest, GetStatusResponse,
    ListAccountsRequest,
};
use nuncio_store::vault::{SecretManager, VaultError, GRPC_TOKEN_ACCOUNT};
use tokio::sync::mpsc;

use crate::engine::EngineState;

/// How often the poll loop calls `GetStatus`/`GetHealth`/`ListAccounts`.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Starting delay before the first `Subscribe` reconnect attempt after the
/// stream drops.
const INITIAL_SUBSCRIBE_BACKOFF: Duration = Duration::from_secs(1);

/// Ceiling for the capped exponential backoff between `Subscribe` reconnect
/// attempts, so a persistently unreachable daemon never gets hammered.
const MAX_SUBSCRIBE_BACKOFF: Duration = Duration::from_secs(30);

/// Bound on the [`StatusUpdate`] channel. A slow or absent UI consumer backs
/// up the poll loop rather than an unbounded queue growing without limit.
const CHANNEL_CAPACITY: usize = 16;

/// Failure modes for [`StatusPoller::connect`].
#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    /// No gRPC bearer token exists in the vault yet. This means `nunciod`
    /// has never run on this machine -- a state the UI must report, not
    /// paper over by minting a token.
    #[error("no gRPC bearer token in vault; nunciod has never run on this machine")]
    NoToken,
    /// The vault could not be read at all, distinct from the key simply
    /// being absent.
    #[error("failed to read gRPC bearer token from vault: {0}")]
    Vault(String),
    /// The token was resolved but the endpoint could not be dialed.
    #[error(transparent)]
    Connect(#[from] ConnectError),
}

/// One snapshot of daemon state pushed to the UI.
///
/// `status` and `health` are `None` -- never a zeroed default -- whenever
/// this cycle's corresponding RPC failed to complete; `accounts` is an empty
/// `Vec` in that same case, since `ListAccounts` carries no numeric field
/// that could be mistaken for a real zero.
#[derive(Debug, Clone)]
pub struct StatusUpdate {
    /// `GetStatus` response for this cycle, or `None` if it could not be
    /// fetched.
    pub status: Option<GetStatusResponse>,
    /// `GetHealth` response for this cycle, or `None` if it could not be
    /// fetched.
    pub health: Option<GetHealthResponse>,
    /// `ListAccounts` result for this cycle; empty if it could not be
    /// fetched.
    pub accounts: Vec<AccountConfig>,
    /// Whether the `Subscribe` push stream is currently known-good. `true`
    /// whenever the stream is disconnected, reconnecting, or has never
    /// successfully connected -- a dead stream must never look like a quiet
    /// one.
    pub stream_stale: bool,
    /// The composite engine state the UI should render (see
    /// [`composite_state`]).
    pub engine_state: EngineState,
}

/// Combines the instance-lock signal (from
/// [`EngineController::liveness`](crate::engine::EngineController::liveness))
/// with this cycle's RPC reachability into the single [`EngineState`] the UI
/// renders.
///
/// Pure and injectable by design: exercised entirely by the unit tests
/// below, with no socket ever opened. That is deliberate -- a wedged daemon
/// (lock held, endpoint not answering) must be representable and testable
/// without a live dial, otherwise `EngineState::NotResponding` could never
/// actually be reached from anywhere in this codebase.
///
/// `rpc_ready` is `None` when `GetStatus` could not be completed this cycle
/// (the endpoint is unreachable), or `Some(response.ready)` when it could.
pub fn composite_state(lock: EngineState, rpc_ready: Option<bool>) -> EngineState {
    match lock {
        // The lock is free: `Stopped`/`Starting` are already fully decided
        // by the lock alone (a launched-but-not-yet-listening daemon cannot
        // be told apart from "not started" by any RPC), and the lock file
        // being unprobeable is already `NotResponding`. None of these
        // benefit from consulting `rpc_ready`.
        EngineState::Stopped => EngineState::Stopped,
        EngineState::Starting => EngineState::Starting,
        EngineState::NotResponding => EngineState::NotResponding,
        // The lock is held: only here does whether the endpoint actually
        // answers change the displayed state.
        EngineState::Running => match rpc_ready {
            Some(true) => EngineState::Running,
            Some(false) => EngineState::Starting,
            None => EngineState::NotResponding,
        },
    }
}

/// Owns the connect/spawn entry points for polling and following a running
/// `nunciod`. Carries no state itself -- each call resolves its own token
/// and dials fresh -- so it is a namespace, not a handle.
pub struct StatusPoller;

impl StatusPoller {
    /// Resolves the gRPC bearer token from `secrets` and dials the
    /// `nuncio.v1.System` service at `addr`.
    ///
    /// Uses [`SecretManager::get_secret`], never `get_or_create_key_bytes`:
    /// the monitor is a read-only client, so a missing token must surface
    /// as [`StatusError::NoToken`] rather than silently minting fresh key
    /// material into the user's OS keyring just because the tray app was
    /// opened. `get_secret` already returns the hex-encoded token, so it is
    /// passed straight through -- re-encoding it (as the decode-then-encode
    /// round trip `get_or_create_key_bytes` + `hex::encode` performs
    /// elsewhere) would produce a token that never authenticates.
    pub async fn connect(
        addr: &str,
        secrets: &SecretManager,
    ) -> Result<nuncio_proto::client::AuthenticatedSystemClient, StatusError> {
        let token = resolve_token(secrets)?;
        Ok(connect_system(addr, &token).await?)
    }

    /// Spawns the background poll loop (`GetStatus`/`GetHealth`/
    /// `ListAccounts` on a timer) and a separate `Subscribe` follower loop,
    /// and returns the receiving end of the [`StatusUpdate`] channel.
    ///
    /// `lock_state` supplies this cycle's instance-lock signal --
    /// deliberately injected rather than read internally, since
    /// [`EngineController`](crate::engine::EngineController) must stay free
    /// of any gRPC dependency. Passing it in is what lets this function turn
    /// that lock signal plus RPC reachability into the composite
    /// [`EngineState`] on every [`StatusUpdate`] via [`composite_state`].
    ///
    /// Both loops resolve their own token on every attempt (never cached),
    /// so a token that appears after the monitor starts (a first `nunciod`
    /// run happening while the tray is already open) is picked up on the
    /// very next cycle rather than requiring a restart.
    pub fn spawn(
        addr: impl Into<String>,
        secrets: Arc<SecretManager>,
        lock_state: impl Fn() -> EngineState + Send + Sync + 'static,
    ) -> mpsc::Receiver<StatusUpdate> {
        let addr = addr.into();
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);

        // `stream_stale` is shared: the subscribe loop is the only writer,
        // the poll loop only reads it when assembling each `StatusUpdate` --
        // this keeps the two loops independent (a hung poll can't wedge the
        // subscribe reconnect logic and vice versa) while still letting one
        // `StatusUpdate` carry both signals.
        let stream_stale = Arc::new(AtomicBool::new(true));

        tokio::spawn(subscribe_loop(
            addr.clone(),
            Arc::clone(&secrets),
            Arc::clone(&stream_stale),
        ));
        tokio::spawn(poll_loop(addr, secrets, lock_state, stream_stale, tx));

        rx
    }
}

/// Resolves the gRPC bearer token, mapping vault outcomes onto
/// [`StatusError`] rather than ever minting a fresh token (see
/// [`StatusPoller::connect`]'s doc comment for why).
fn resolve_token(secrets: &SecretManager) -> Result<String, StatusError> {
    match secrets.get_secret(GRPC_TOKEN_ACCOUNT) {
        Ok(hex_token) => Ok(hex_token),
        Err(VaultError::NotFound(_)) => Err(StatusError::NoToken),
        Err(other) => Err(StatusError::Vault(other.to_string())),
    }
}

/// Drives the timed `GetStatus` + `GetHealth` + `ListAccounts` poll and
/// pushes one [`StatusUpdate`] per tick, until the receiver is dropped.
async fn poll_loop(
    addr: String,
    secrets: Arc<SecretManager>,
    lock_state: impl Fn() -> EngineState + Send + Sync + 'static,
    stream_stale: Arc<AtomicBool>,
    tx: mpsc::Sender<StatusUpdate>,
) {
    // `tokio::time::interval` fires its first tick immediately, so the UI
    // gets a first snapshot without waiting a full `POLL_INTERVAL`.
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    loop {
        interval.tick().await;

        let update = poll_once(&addr, &secrets, &lock_state, &stream_stale).await;
        if tx.send(update).await.is_err() {
            // The UI side is gone; stop polling rather than running forever
            // against a channel nobody reads.
            return;
        }
    }
}

/// One poll cycle: resolves the token, attempts `GetStatus`/`GetHealth` over
/// a single connection, and `ListAccounts` over a separate one (a distinct
/// `nuncio.v1` service), degrading each independently to `None`/empty rather
/// than letting one failure abort the whole cycle.
async fn poll_once(
    addr: &str,
    secrets: &SecretManager,
    lock_state: &(impl Fn() -> EngineState + Send + Sync + 'static),
    stream_stale: &AtomicBool,
) -> StatusUpdate {
    let Ok(token) = resolve_token(secrets) else {
        // No token at all: nothing over gRPC can be attempted this cycle.
        // The lock signal is still real and still reported -- only the RPC
        // side of the composite is unknown.
        return StatusUpdate {
            status: None,
            health: None,
            accounts: Vec::new(),
            stream_stale: stream_stale.load(Ordering::Relaxed),
            engine_state: composite_state(lock_state(), None),
        };
    };

    // `StatusPoller::connect` re-resolves the token itself; that duplicate
    // resolution is cheap (a mocked or keyring-cached read) and keeps this
    // call site sharing the exact same resolve-then-dial sequence
    // [`StatusPoller::connect`]'s own tests pin down, rather than
    // hand-rolling a second copy of it here.
    let mut system_client = StatusPoller::connect(addr, secrets).await.ok();

    let status = match system_client.as_mut() {
        Some(client) => client
            .get_status(GetStatusRequest {})
            .await
            .ok()
            .map(|response| response.into_inner()),
        None => None,
    };

    let health = match system_client.as_mut() {
        Some(client) => client
            .get_health(GetHealthRequest {})
            .await
            .ok()
            .map(|response| response.into_inner()),
        None => None,
    };

    let accounts = fetch_accounts(addr, &token).await;

    let rpc_ready = status.as_ref().map(|s| s.ready);
    StatusUpdate {
        status,
        health,
        accounts,
        stream_stale: stream_stale.load(Ordering::Relaxed),
        engine_state: composite_state(lock_state(), rpc_ready),
    }
}

/// Fetches every persisted account configuration over the `Accounts`
/// service, returning an empty `Vec` (never fabricated entries) if the
/// endpoint could not be reached or the call failed.
async fn fetch_accounts(addr: &str, token: &str) -> Vec<AccountConfig> {
    let Ok(mut client) = connect_accounts(addr, token).await else {
        return Vec::new();
    };
    client
        .list_accounts(ListAccountsRequest {})
        .await
        .map(|response| response.into_inner().accounts)
        .unwrap_or_default()
}

/// Follows `System/Subscribe` for the lifetime of the monitor, marking
/// `stream_stale` the instant the stream is not known-good (never
/// connected, dropped, or errored) and reconnecting with capped exponential
/// backoff. Runs until the process exits -- there is no receiver to signal
/// this loop to stop, since its only output is the shared `stream_stale`
/// flag consumed by [`poll_once`].
async fn subscribe_loop(addr: String, secrets: Arc<SecretManager>, stream_stale: Arc<AtomicBool>) {
    let mut backoff = INITIAL_SUBSCRIBE_BACKOFF;
    loop {
        let Ok(token) = resolve_token(&secrets) else {
            stream_stale.store(true, Ordering::Relaxed);
            tokio::time::sleep(backoff).await;
            backoff = next_backoff(backoff);
            continue;
        };

        let mut client = match connect_system(&addr, &token).await {
            Ok(client) => client,
            Err(_) => {
                stream_stale.store(true, Ordering::Relaxed);
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
        };

        let mut stream = match subscribe_events(&mut client).await {
            Ok(stream) => stream,
            Err(_) => {
                stream_stale.store(true, Ordering::Relaxed);
                tokio::time::sleep(backoff).await;
                backoff = next_backoff(backoff);
                continue;
            }
        };

        // The stream is open: clear staleness now, not only once the first
        // event arrives. A live but currently quiet stream (no domain
        // events yet) must not be indistinguishable from a dead one.
        stream_stale.store(false, Ordering::Relaxed);
        backoff = INITIAL_SUBSCRIBE_BACKOFF;

        loop {
            match stream.message().await {
                Ok(Some(_event)) => stream_stale.store(false, Ordering::Relaxed),
                Ok(None) | Err(_) => {
                    // The daemon closed the stream, or it errored. Either
                    // way this must read as stale, never as a quiet-but-
                    // healthy stream, so the outer loop's reconnect kicks in
                    // with backoff rather than silently going dark.
                    stream_stale.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = next_backoff(backoff);
    }
}

/// Doubles `current`, capped at [`MAX_SUBSCRIBE_BACKOFF`].
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_SUBSCRIBE_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_proto::v1::accounts_server::{Accounts as AccountsService, AccountsServer};
    use nuncio_proto::v1::system_server::{System as SystemService, SystemServer};
    use nuncio_proto::v1::{
        AccountConfig as AccountConfigProto, AddAccountRequest, AddAccountResponse, Event,
        GetHealthResponse as GetHealthResponseProto, GetStatusResponse as GetStatusResponseProto,
        ListAccountsResponse, RemoveAccountRequest, RemoveAccountResponse,
        ShutdownRequest as ShutdownRequestProto, ShutdownResponse as ShutdownResponseProto,
        SubscribeRequest, TestAccountConnectionRequest, TestAccountConnectionResponse,
        UpdateAccountRequest, UpdateAccountResponse,
    };
    use tokio_stream::wrappers::ReceiverStream;

    // --- composite_state: the whole table, exercised with no socket ---

    #[test]
    fn composite_state_free_lock_is_stopped_regardless_of_rpc() {
        assert_eq!(
            composite_state(EngineState::Stopped, Some(true)),
            EngineState::Stopped
        );
        assert_eq!(
            composite_state(EngineState::Stopped, None),
            EngineState::Stopped
        );
    }

    #[test]
    fn composite_state_starting_lock_passes_through_regardless_of_rpc() {
        assert_eq!(
            composite_state(EngineState::Starting, Some(true)),
            EngineState::Starting
        );
        assert_eq!(
            composite_state(EngineState::Starting, None),
            EngineState::Starting
        );
    }

    #[test]
    fn composite_state_lock_probe_error_passes_through_as_not_responding() {
        assert_eq!(
            composite_state(EngineState::NotResponding, Some(true)),
            EngineState::NotResponding
        );
    }

    #[test]
    fn composite_state_held_lock_reachable_and_ready_is_running() {
        assert_eq!(
            composite_state(EngineState::Running, Some(true)),
            EngineState::Running
        );
    }

    #[test]
    fn composite_state_held_lock_reachable_but_not_ready_is_starting() {
        assert_eq!(
            composite_state(EngineState::Running, Some(false)),
            EngineState::Starting
        );
    }

    #[test]
    fn composite_state_held_lock_unreachable_is_not_responding() {
        // This is the row the module doc calls out: a wedged daemon (lock
        // held, endpoint not answering) must resolve to `NotResponding`,
        // never a falsely healthy `Running` or falsely safe `Stopped`.
        assert_eq!(
            composite_state(EngineState::Running, None),
            EngineState::NotResponding
        );
    }

    // --- token resolution: the required failing-first test from the brief ---

    #[tokio::test]
    async fn a_missing_token_is_an_honest_error_not_an_empty_status() {
        let secrets = SecretManager::mock();
        let err = StatusPoller::connect("127.0.0.1:9420", &secrets)
            .await
            .expect_err("must fail without a minted token");
        assert!(matches!(err, StatusError::NoToken));
    }

    #[tokio::test]
    async fn connect_never_mints_a_token_as_a_side_effect_of_failing() {
        let secrets = SecretManager::mock();
        let _ = StatusPoller::connect("127.0.0.1:9420", &secrets).await;
        // A second read must still see nothing: `connect`'s failure path
        // must never have called `get_or_create_key_bytes` (or otherwise
        // written) as a side effect of resolving the token.
        assert!(matches!(
            secrets.get_secret(GRPC_TOKEN_ACCOUNT),
            Err(VaultError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_present_token_is_used_verbatim_not_re_encoded() {
        // `get_secret` already returns the hex-encoded token as stored; if
        // `connect` re-encoded it (the `get_or_create_key_bytes` +
        // `hex::encode` round trip used elsewhere), a plain hex string
        // stored directly here would fail to decode and connect would never
        // reach the transport-error stage below.
        let secrets = SecretManager::mock();
        secrets
            .set_secret(GRPC_TOKEN_ACCOUNT, "deadbeef")
            .expect("mock vault write succeeds");

        // Port 1 on loopback is not listening, so this deterministically
        // reaches `ConnectError::Transport` rather than hanging -- proving
        // the token was accepted and dialing was actually attempted.
        let err = StatusPoller::connect("127.0.0.1:1", &secrets)
            .await
            .expect_err("nothing listens on this port");
        assert!(matches!(
            err,
            StatusError::Connect(ConnectError::Transport { .. })
        ));
    }

    // --- spawn: end-to-end against local stub daemons ---

    /// Minimal stub of `nuncio.v1.System` for `spawn` tests: fixed
    /// `GetStatus`/`GetHealth`, and a `Subscribe` stream that stays open
    /// (backed by a channel whose sender the caller holds) without ever
    /// emitting an event, so tests can observe "connected but quiet" and
    /// "reachable" independently of whether any domain event was ever sent.
    struct StubSystem {
        subscribe_rx: std::sync::Mutex<Option<mpsc::Receiver<Result<Event, tonic::Status>>>>,
    }

    #[tonic::async_trait]
    impl SystemService for StubSystem {
        async fn get_status(
            &self,
            _request: tonic::Request<nuncio_proto::v1::GetStatusRequest>,
        ) -> Result<tonic::Response<GetStatusResponseProto>, tonic::Status> {
            Ok(tonic::Response::new(GetStatusResponseProto {
                engine_status: "Ready".to_string(),
                version: "9.9.9".to_string(),
                uptime: Some(nuncio_proto::time::duration_from_secs(5)),
                accounts_loaded: 1,
                unread_count: 3,
                last_error: None,
                outbox_depth: 0,
                account_sync_states: Vec::new(),
                ready: true,
                db_healthy: true,
            }))
        }

        async fn get_health(
            &self,
            _request: tonic::Request<nuncio_proto::v1::GetHealthRequest>,
        ) -> Result<tonic::Response<GetHealthResponseProto>, tonic::Status> {
            Ok(tonic::Response::new(GetHealthResponseProto {
                account_queues: Vec::new(),
                wal_size_bytes: 1024,
                db_healthy: true,
            }))
        }

        type SubscribeStream = std::pin::Pin<
            Box<dyn tokio_stream::Stream<Item = Result<Event, tonic::Status>> + Send>,
        >;

        async fn subscribe(
            &self,
            _request: tonic::Request<SubscribeRequest>,
        ) -> Result<tonic::Response<Self::SubscribeStream>, tonic::Status> {
            let rx = self
                .subscribe_rx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .expect("subscribe called at most once in these tests");
            Ok(tonic::Response::new(Box::pin(ReceiverStream::new(rx))))
        }

        async fn shutdown(
            &self,
            _request: tonic::Request<ShutdownRequestProto>,
        ) -> Result<tonic::Response<ShutdownResponseProto>, tonic::Status> {
            Ok(tonic::Response::new(ShutdownResponseProto {}))
        }
    }

    /// Minimal stub of `nuncio.v1.Accounts`: only `list_accounts` returns
    /// real data; the rest exist solely to satisfy the trait.
    #[derive(Default)]
    struct StubAccounts;

    #[tonic::async_trait]
    impl AccountsService for StubAccounts {
        async fn add_account(
            &self,
            _request: tonic::Request<AddAccountRequest>,
        ) -> Result<tonic::Response<AddAccountResponse>, tonic::Status> {
            Err(tonic::Status::unimplemented("not exercised by these tests"))
        }

        async fn update_account(
            &self,
            _request: tonic::Request<UpdateAccountRequest>,
        ) -> Result<tonic::Response<UpdateAccountResponse>, tonic::Status> {
            Ok(tonic::Response::new(UpdateAccountResponse {}))
        }

        async fn remove_account(
            &self,
            _request: tonic::Request<RemoveAccountRequest>,
        ) -> Result<tonic::Response<RemoveAccountResponse>, tonic::Status> {
            Ok(tonic::Response::new(RemoveAccountResponse {}))
        }

        async fn test_account_connection(
            &self,
            _request: tonic::Request<TestAccountConnectionRequest>,
        ) -> Result<tonic::Response<TestAccountConnectionResponse>, tonic::Status> {
            Ok(tonic::Response::new(TestAccountConnectionResponse {
                imap_ok: true,
                smtp_ok: true,
                imap_error: None,
                smtp_error: None,
            }))
        }

        async fn list_accounts(
            &self,
            _request: tonic::Request<ListAccountsRequest>,
        ) -> Result<tonic::Response<ListAccountsResponse>, tonic::Status> {
            Ok(tonic::Response::new(ListAccountsResponse {
                accounts: vec![AccountConfigProto {
                    id: "acct-stub-1".to_string(),
                    name: "Stub Account".to_string(),
                    email_address: "stub@nuncio.mx".to_string(),
                    keyring_secret_key: "nuncio/acct-stub-1".to_string(),
                    sync_interval: Some(nuncio_proto::time::duration_from_secs(300)),
                    transport: None,
                }],
            }))
        }
    }

    /// Binds both stub services on one ephemeral loopback listener and
    /// spawns the server, returning the address to dial.
    async fn spawn_stub_daemon(
        subscribe_rx: mpsc::Receiver<Result<Event, tonic::Status>>,
    ) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        let system = StubSystem {
            subscribe_rx: std::sync::Mutex::new(Some(subscribe_rx)),
        };
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(SystemServer::new(system))
                .add_service(AccountsServer::new(StubAccounts))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });
        addr.to_string()
    }

    #[tokio::test]
    async fn spawn_reports_none_fields_and_not_responding_when_daemon_is_unreachable() {
        let secrets = Arc::new(SecretManager::mock());
        secrets
            .set_secret(GRPC_TOKEN_ACCOUNT, "deadbeef")
            .expect("mock vault write succeeds");

        // Nothing listens on this port, so every dial this cycle fails
        // closed rather than hanging.
        let mut rx = StatusPoller::spawn("127.0.0.1:1", secrets, || EngineState::Running);

        let update = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("first update arrives within the timeout")
            .expect("channel stays open");

        assert!(update.status.is_none());
        assert!(update.health.is_none());
        assert!(update.accounts.is_empty());
        assert!(update.stream_stale);
        // The lock says the daemon is running, but nothing answered: this
        // is exactly the case `EngineState::NotResponding` exists for.
        assert_eq!(update.engine_state, EngineState::NotResponding);
    }

    #[tokio::test]
    async fn spawn_reports_live_status_and_running_when_daemon_is_reachable() {
        let (subscribe_tx, subscribe_rx) = mpsc::channel(1);
        let addr = spawn_stub_daemon(subscribe_rx).await;
        // Held for the test's duration so the `Subscribe` stream stays open
        // (a dropped sender would close it) without ever sending an event --
        // this proves a quiet-but-live stream reports as not stale.
        let _keep_subscribe_open = subscribe_tx;

        let secrets = Arc::new(SecretManager::mock());
        secrets
            .set_secret(GRPC_TOKEN_ACCOUNT, "deadbeef")
            .expect("mock vault write succeeds");

        let mut rx = StatusPoller::spawn(addr, secrets, || EngineState::Running);

        // Poll until the stream is observed non-stale or the timeout
        // expires -- the poll loop and subscribe loop connect
        // independently, so the very first `StatusUpdate` can legitimately
        // still show `stream_stale: true` if it lands before the subscribe
        // loop finishes connecting.
        let update = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let update = rx.recv().await.expect("channel stays open");
                if !update.stream_stale && update.status.is_some() {
                    return update;
                }
            }
        })
        .await
        .expect("a non-stale update with live status arrives within the timeout");

        let status = update.status.expect("status present");
        assert_eq!(status.engine_status, "Ready");
        assert_eq!(status.version, "9.9.9");
        assert!(status.ready);

        let health = update.health.expect("health present");
        assert_eq!(health.wal_size_bytes, 1024);

        assert_eq!(update.accounts.len(), 1);
        assert_eq!(update.accounts[0].id, "acct-stub-1");

        assert_eq!(update.engine_state, EngineState::Running);
        assert!(!update.stream_stale);
    }
}
