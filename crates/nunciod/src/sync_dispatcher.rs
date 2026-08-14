//! Bounded-concurrency sync dispatcher.
//!
//! Account syncs used to be launched directly and independently from two
//! places (the client-facing `Mail/Sync` RPC handler and the daemon's
//! `CoreCommand` consumer), each of which ran an account's sync to completion
//! before the next one started. One slow or hung account could therefore
//! head-of-line-block every account behind it, and nothing bounded how many
//! syncs ran at once or how long a single account was allowed to take.
//!
//! [`SyncDispatcher`] is the single place all account syncs now funnel
//! through. It:
//!
//! * caps how many account syncs run at once with a [`Semaphore`]
//!   (env-tunable via [`CONCURRENCY_ENV_VAR`], default
//!   [`DEFAULT_CONCURRENCY`]); excess requests queue on the permit rather
//!   than run unbounded;
//! * coalesces a second request for an account that is already syncing onto
//!   the in-flight one instead of double-fetching -- a client `Sync` issued
//!   while a scheduled sync of the same account is running joins it and
//!   observes its result;
//! * bounds each account's sync with a per-account timeout (env-tunable via
//!   [`TIMEOUT_ENV_VAR`], default [`DEFAULT_TIMEOUT`]); a timed-out sync is
//!   cancelled cleanly and surfaces [`SyncDispatchError::Timeout`] without
//!   poisoning the dispatcher for other accounts; and
//! * respects the shared shutdown signal: once shutdown is requested it stops
//!   admitting new work and races each in-flight sync against the signal
//!   (mirroring the outbox executor) so a hung account cannot delay graceful
//!   shutdown beyond the daemon's grace period.
//!
//! # Testability
//!
//! The actual per-account sync is injected behind the [`AccountSyncer`]
//! trait, so the dispatcher's concurrency, coalescing, timeout, and shutdown
//! behavior is exercised deterministically with an in-memory mock syncer --
//! no real network, keyring, or database. Production wires
//! [`ProductionAccountSyncer`], which delegates to
//! [`crate::sync::run_account_sync`].

use crate::lifecycle::ShutdownSignal;
use crate::sync::SyncError;
use async_trait::async_trait;
use nuncio_core::{CoreCommand, EventBus};
use nuncio_filter::FilterEngine;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::SecretManager;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::sync::{watch, Semaphore};
use tracing::Instrument;

/// Environment variable bounding how many account syncs may run at once.
pub const CONCURRENCY_ENV_VAR: &str = "NUNCIO_SYNC_CONCURRENCY";

/// Environment variable bounding a single account's sync, in whole seconds.
pub const TIMEOUT_ENV_VAR: &str = "NUNCIO_SYNC_TIMEOUT";

/// Default simultaneous-sync cap when [`CONCURRENCY_ENV_VAR`] is unset or
/// unparseable. Chosen to overlap a handful of accounts without letting an
/// unbounded fan-out open a socket per account at once.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// Default per-account sync timeout when [`TIMEOUT_ENV_VAR`] is unset or
/// unparseable. Generous enough for a healthy full mailbox fetch, tight
/// enough that a hung account cannot stall others (or shutdown) indefinitely.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// The outcome of a single dispatched account sync. Cloneable so a coalesced
/// result can fan out to every joined waiter.
type SyncOutcome = Result<usize, SyncDispatchError>;

/// Errors surfaced by [`SyncDispatcher::sync_account`].
#[derive(Debug, Clone, Error)]
pub enum SyncDispatchError {
    /// The account's sync exceeded the configured per-account timeout and was
    /// cancelled. Retryable: nothing about the dispatcher is left broken.
    #[error("account '{account_id}' sync exceeded the {timeout:?} per-account timeout")]
    Timeout {
        /// The account whose sync timed out.
        account_id: String,
        /// The timeout that was exceeded.
        timeout: Duration,
    },
    /// Shutdown was requested before or during the sync; no new work is
    /// admitted and in-flight work is abandoned for the next start.
    #[error("sync dispatcher is shutting down")]
    ShuttingDown,
    /// The in-flight sync ended without producing a result (e.g. its task was
    /// dropped). Retryable.
    #[error("account sync was cancelled before it produced a result")]
    Cancelled,
    /// The underlying account sync itself failed. Shared behind an [`Arc`] so
    /// a coalesced failure can be reported to every joined waiter.
    #[error(transparent)]
    Sync(Arc<SyncError>),
}

/// Live sync phase of a single account, derived from the dispatcher's
/// in-flight map and last-result tracking for the daemon health surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountSyncPhase {
    /// No sync in flight; the last attempt (if any) succeeded.
    Idle,
    /// A sync for the account is currently running.
    Syncing,
    /// No sync in flight, but the most recent attempt failed.
    Error,
}

/// A snapshot of one account's sync state, combining its current phase with
/// the outcome of its most recent completed attempt. `last_success_at`
/// survives a subsequent failure so a transient error does not erase the
/// last known-good sync time.
#[derive(Debug, Clone)]
pub struct AccountSyncStatus {
    /// Current phase.
    pub phase: AccountSyncPhase,
    /// When the account last completed a SUCCESSFUL sync, if ever since start.
    pub last_success_at: Option<SystemTime>,
    /// The most recent attempt's failure detail, present only when the last
    /// attempt failed.
    pub last_error: Option<String>,
}

/// The recorded outcome of an account's most recent completed sync attempt.
/// A success clears `last_error` and stamps `last_success_at`; a failure sets
/// `last_error` while leaving any earlier `last_success_at` intact.
#[derive(Default, Clone)]
struct AccountSyncRecord {
    last_success_at: Option<SystemTime>,
    last_error: Option<String>,
}

/// Injectable seam for the actual work of syncing one account. Production
/// uses [`ProductionAccountSyncer`]; tests inject an in-memory mock.
#[async_trait]
pub trait AccountSyncer: Send + Sync {
    /// Sync exactly one account to completion, returning the number of
    /// messages processed.
    async fn sync_account(&self, account_id: &str) -> Result<usize, SyncError>;
}

/// Production [`AccountSyncer`]: delegates to [`crate::sync::run_account_sync`],
/// which resolves the account's credentials, builds a real mail backend, and
/// runs a full inbound sync with the standard engine-status lifecycle.
pub struct ProductionAccountSyncer {
    db: Arc<DatabaseEngine>,
    secrets: Arc<SecretManager>,
    event_bus: Arc<EventBus>,
    filter_engine: Arc<FilterEngine>,
}

impl ProductionAccountSyncer {
    /// Build the production syncer from the daemon's shared subsystems.
    pub fn new(
        db: Arc<DatabaseEngine>,
        secrets: Arc<SecretManager>,
        event_bus: Arc<EventBus>,
        filter_engine: Arc<FilterEngine>,
    ) -> Self {
        Self {
            db,
            secrets,
            event_bus,
            filter_engine,
        }
    }
}

#[async_trait]
impl AccountSyncer for ProductionAccountSyncer {
    async fn sync_account(&self, account_id: &str) -> Result<usize, SyncError> {
        crate::sync::run_account_sync(
            &self.db,
            &self.secrets,
            &self.event_bus,
            &self.filter_engine,
            account_id,
        )
        .await
    }
}

/// Reads [`CONCURRENCY_ENV_VAR`], falling back to [`DEFAULT_CONCURRENCY`].
/// A value of zero (or unparseable) is treated as the default; the effective
/// cap is always at least one.
fn concurrency_from_env() -> usize {
    std::env::var(CONCURRENCY_ENV_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_CONCURRENCY)
}

/// Reads [`TIMEOUT_ENV_VAR`] as whole seconds, falling back to
/// [`DEFAULT_TIMEOUT`]. A value of zero (or unparseable) is treated as the
/// default rather than "no timeout", so a misconfiguration can never disable
/// the safety bound.
fn timeout_from_env() -> Duration {
    std::env::var(TIMEOUT_ENV_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// Shared inner state, held behind an [`Arc`] so the dispatcher is cheap to
/// clone and can be moved into the per-account tasks it spawns.
struct Inner {
    syncer: Arc<dyn AccountSyncer>,
    semaphore: Arc<Semaphore>,
    /// Accounts with a sync currently in flight, mapped to a watch receiver
    /// that publishes the sync's outcome once. A second request for the same
    /// account clones the receiver and joins the in-flight sync.
    inflight: Mutex<HashMap<String, watch::Receiver<Option<SyncOutcome>>>>,
    /// The most recent completed outcome per account, retained after the
    /// in-flight entry is cleared so the health surface can report each
    /// account's last sync time and last error.
    last_results: Mutex<HashMap<String, AccountSyncRecord>>,
    per_account_timeout: Duration,
    shutdown: ShutdownSignal,
}

impl Inner {
    /// Run one account sync under the semaphore permit and per-account
    /// timeout, racing both the permit wait and the sync itself against the
    /// shutdown signal (mirroring the outbox executor). The permit is
    /// released as soon as the sync resolves.
    async fn run_guarded(&self, account_id: &str) -> SyncOutcome {
        let mut shutdown = self.shutdown.clone();

        let permit = tokio::select! {
            biased;
            _ = shutdown.wait() => return Err(SyncDispatchError::ShuttingDown),
            acquired = self.semaphore.clone().acquire_owned() => match acquired {
                Ok(permit) => permit,
                // The semaphore is never closed in practice; treat a closed
                // semaphore as a cancellation rather than fabricating success.
                Err(_) => return Err(SyncDispatchError::Cancelled),
            },
        };

        let outcome = tokio::select! {
            biased;
            _ = shutdown.wait() => Err(SyncDispatchError::ShuttingDown),
            result = tokio::time::timeout(
                self.per_account_timeout,
                self.syncer.sync_account(account_id),
            ) => match result {
                Ok(Ok(count)) => Ok(count),
                Ok(Err(e)) => Err(SyncDispatchError::Sync(Arc::new(e))),
                Err(_elapsed) => Err(SyncDispatchError::Timeout {
                    account_id: account_id.to_string(),
                    timeout: self.per_account_timeout,
                }),
            },
        };

        drop(permit);
        outcome
    }
}

/// The single entry point every account sync funnels through. Cheap to clone
/// (an [`Arc`] handle); every clone shares the same concurrency cap,
/// in-flight map, timeout, and shutdown signal.
#[derive(Clone)]
pub struct SyncDispatcher {
    inner: Arc<Inner>,
}

impl SyncDispatcher {
    /// Build a dispatcher with explicit bounds. `concurrency` is clamped up to
    /// at least one so the dispatcher can always make progress.
    pub fn new(
        syncer: Arc<dyn AccountSyncer>,
        shutdown: ShutdownSignal,
        concurrency: usize,
        per_account_timeout: Duration,
    ) -> Self {
        let concurrency = concurrency.max(1);
        Self {
            inner: Arc::new(Inner {
                syncer,
                semaphore: Arc::new(Semaphore::new(concurrency)),
                inflight: Mutex::new(HashMap::new()),
                last_results: Mutex::new(HashMap::new()),
                per_account_timeout,
                shutdown,
            }),
        }
    }

    /// Build a dispatcher whose bounds come from the environment
    /// ([`CONCURRENCY_ENV_VAR`] / [`TIMEOUT_ENV_VAR`]), falling back to
    /// [`DEFAULT_CONCURRENCY`] / [`DEFAULT_TIMEOUT`].
    pub fn from_env(syncer: Arc<dyn AccountSyncer>, shutdown: ShutdownSignal) -> Self {
        Self::new(syncer, shutdown, concurrency_from_env(), timeout_from_env())
    }

    /// Dispatch a sync for a single account, applying bounded concurrency,
    /// in-flight coalescing, the per-account timeout, and shutdown handling.
    ///
    /// If a sync for `account_id` is already in flight, this joins it and
    /// returns the SAME outcome rather than starting a duplicate fetch.
    pub async fn sync_account(&self, account_id: &str) -> SyncOutcome {
        if self.inner.shutdown.is_requested() {
            return Err(SyncDispatchError::ShuttingDown);
        }

        // Either join the in-flight sync for this account or become its owner
        // by inserting a fresh result channel and spawning the guarded run.
        // The lock is released before awaiting, so it never spans `.await`.
        let mut rx = {
            let mut inflight = self
                .inner
                .inflight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(existing) = inflight.get(account_id) {
                existing.clone()
            } else {
                let (tx, rx) = watch::channel::<Option<SyncOutcome>>(None);
                inflight.insert(account_id.to_string(), rx.clone());

                let inner = self.inner.clone();
                let id = account_id.to_string();
                // The actual sync runs in a detached task, which does NOT
                // inherit the caller's span automatically. Capture the current
                // span (the RPC's correlation span when a `Mail/Sync` triggered
                // this) and instrument the task with it, so the sync's logs are
                // correlatable to the request that kicked it off. A request
                // that COALESCES onto an already-running sync joins the owner's
                // task and is therefore correlated to the owner's request_id,
                // not its own.
                let span = tracing::Span::current();
                tokio::spawn(
                    async move {
                        let outcome = inner.run_guarded(&id).await;
                        // Record this attempt's outcome for the health surface
                        // before clearing the in-flight entry: a success stamps
                        // the last-synced time and clears any prior error; a
                        // failure records the error while preserving the last
                        // known-good sync time.
                        {
                            let mut results = inner
                                .last_results
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner);
                            let record = results.entry(id.clone()).or_default();
                            match &outcome {
                                Ok(_) => {
                                    record.last_success_at = Some(SystemTime::now());
                                    record.last_error = None;
                                }
                                Err(e) => record.last_error = Some(e.to_string()),
                            }
                        }
                        // Remove the in-flight entry before publishing so a
                        // request arriving after completion starts a fresh sync
                        // rather than joining an already-finished one.
                        inner
                            .inflight
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .remove(&id);
                        let _ = tx.send(Some(outcome));
                    }
                    .instrument(span),
                );
                rx
            }
        };

        // Clone the published outcome out of the borrowed watch guard before
        // the guard (and `rx`) drop at the end of this scope.
        let published = match rx.wait_for(Option::is_some).await {
            Ok(guard) => guard.clone(),
            // The sender was dropped without publishing (the owning task was
            // itself dropped). Honestly report a cancellation, never success.
            Err(_) => None,
        };
        published.unwrap_or(Err(SyncDispatchError::Cancelled))
    }

    /// Report the live sync state of a single account for the daemon health
    /// surface: `Syncing` while a sync is in flight, otherwise `Error` if the
    /// most recent attempt failed, else `Idle`. `last_success_at`/`last_error`
    /// carry the genuine outcome of the last completed attempt (never
    /// fabricated); an account that has not synced since daemon start reports
    /// `Idle` with neither set.
    pub fn sync_status(&self, account_id: &str) -> AccountSyncStatus {
        let syncing = self
            .inner
            .inflight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(account_id);
        let record = self
            .inner
            .last_results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(account_id)
            .cloned()
            .unwrap_or_default();
        let phase = if syncing {
            AccountSyncPhase::Syncing
        } else if record.last_error.is_some() {
            AccountSyncPhase::Error
        } else {
            AccountSyncPhase::Idle
        };
        AccountSyncStatus {
            phase,
            last_success_at: record.last_success_at,
            last_error: record.last_error,
        }
    }

    /// Dispatch syncs for several accounts concurrently, each through the same
    /// bounded/coalesced/timed per-account path, and return the total number
    /// of messages synced across the accounts that succeeded. Per-account
    /// failures are logged and do not abort the batch (the underlying
    /// production syncer additionally reports them on the event bus).
    pub async fn sync_all(&self, account_ids: Vec<String>) -> usize {
        let mut set = tokio::task::JoinSet::new();
        // Propagate the caller's span (the RPC correlation span for an
        // all-accounts `Mail/Sync`) into each per-account task so the fan-out
        // stays correlated to the triggering request.
        let span = tracing::Span::current();
        for account_id in account_ids {
            let dispatcher = self.clone();
            set.spawn(
                async move {
                    let result = dispatcher.sync_account(&account_id).await;
                    (account_id, result)
                }
                .instrument(span.clone()),
            );
        }

        let mut total = 0usize;
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((_, Ok(count))) => total += count,
                Ok((account_id, Err(e))) => {
                    tracing::warn!("dispatched sync for account '{account_id}' failed: {e}");
                }
                Err(join_err) => {
                    tracing::warn!("dispatched sync task failed to join: {join_err}");
                }
            }
        }
        total
    }
}

/// Sync every configured mail account through `dispatcher`, wrapped in the
/// standard batch `SyncStarted{None}` / `SyncCompleted{None}` envelope.
///
/// DAV-protocol accounts have no inbound mail backend and are skipped (they
/// sync through their own domain service). This is the single production
/// entry point for an all-accounts sync, shared by the `Mail/Sync` RPC
/// (`account_id == None`) and the daemon's `CoreCommand::SyncAll` consumer, so
/// the two never launch account syncs through separate, differently-bounded
/// paths. Returns the total number of messages synced across all accounts.
pub async fn sync_all_configured(
    dispatcher: &SyncDispatcher,
    db: &DatabaseEngine,
    event_bus: &EventBus,
) -> usize {
    event_bus.process_command(CoreCommand::SyncAll);

    let account_ids = match db.list_accounts().await {
        Ok(accounts) => accounts
            .into_iter()
            .filter(|a| !a.is_dav())
            .map(|a| a.id)
            .collect::<Vec<_>>(),
        Err(e) => {
            event_bus.process_command(CoreCommand::ReportError {
                message: format!("failed to list accounts for sync: {e}"),
            });
            Vec::new()
        }
    };

    let total = dispatcher.sync_all(account_ids).await;
    event_bus.complete_sync(None);
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::{AccountConfig, CoreEvent, DavTransport, JmapTransport, Transport};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{sleep, Duration};

    /// In-memory [`AccountSyncer`] that records invocations, tracks peak
    /// concurrency, and can be told to block indefinitely for named accounts
    /// (to model a hung account) or to add a fixed per-call delay (to make
    /// concurrent calls genuinely overlap).
    struct MockSyncer {
        calls: Mutex<Vec<String>>,
        concurrent: AtomicUsize,
        peak: AtomicUsize,
        blocked: HashSet<String>,
        gate: Arc<tokio::sync::Notify>,
        per_call_delay: Option<Duration>,
        fail: HashSet<String>,
    }

    impl MockSyncer {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                concurrent: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                blocked: HashSet::new(),
                gate: Arc::new(tokio::sync::Notify::new()),
                per_call_delay: None,
                fail: HashSet::new(),
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.per_call_delay = Some(delay);
            self
        }

        fn blocking(mut self, account_id: &str) -> Self {
            self.blocked.insert(account_id.to_string());
            self
        }

        fn failing(mut self, account_id: &str) -> Self {
            self.fail.insert(account_id.to_string());
            self
        }

        fn call_ids(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn call_count(&self) -> usize {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len()
        }
    }

    #[async_trait]
    impl AccountSyncer for MockSyncer {
        async fn sync_account(&self, account_id: &str) -> Result<usize, SyncError> {
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(account_id.to_string());
            let current = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(current, Ordering::SeqCst);

            if self.blocked.contains(account_id) {
                self.gate.notified().await;
            } else if let Some(delay) = self.per_call_delay {
                sleep(delay).await;
            }

            self.concurrent.fetch_sub(1, Ordering::SeqCst);

            if self.fail.contains(account_id) {
                return Err(SyncError::AccountNotFound(account_id.to_string()));
            }
            Ok(1)
        }
    }

    fn dispatcher_with(
        syncer: Arc<MockSyncer>,
        concurrency: usize,
        timeout: Duration,
    ) -> SyncDispatcher {
        SyncDispatcher::new(syncer, ShutdownSignal::never(), concurrency, timeout)
    }

    #[tokio::test]
    async fn bounds_simultaneous_syncs_to_the_configured_cap() {
        let syncer = Arc::new(MockSyncer::new().with_delay(Duration::from_millis(80)));
        let dispatcher = dispatcher_with(syncer.clone(), 2, Duration::from_secs(30));

        let mut set = tokio::task::JoinSet::new();
        for i in 0..6 {
            let d = dispatcher.clone();
            set.spawn(async move { d.sync_account(&format!("acct-{i}")).await });
        }
        while let Some(joined) = set.join_next().await {
            assert!(matches!(joined, Ok(Ok(1))), "each distinct sync succeeds");
        }

        assert_eq!(
            syncer.peak.load(Ordering::SeqCst),
            2,
            "no more than the configured cap of 2 syncs may run at once"
        );
        assert_eq!(syncer.call_count(), 6, "every distinct account is synced");
    }

    #[tokio::test]
    async fn coalesces_a_duplicate_in_flight_sync_for_the_same_account() {
        let syncer = Arc::new(MockSyncer::new().with_delay(Duration::from_millis(80)));
        let dispatcher = dispatcher_with(syncer.clone(), 4, Duration::from_secs(30));

        // Three concurrent requests for the SAME account, launched close
        // enough together to all observe the first as still in flight.
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..3 {
            let d = dispatcher.clone();
            set.spawn(async move { d.sync_account("acct-dup").await });
        }
        while let Some(joined) = set.join_next().await {
            assert!(
                matches!(joined, Ok(Ok(1))),
                "every joined waiter sees the result"
            );
        }

        assert_eq!(
            syncer.call_ids(),
            vec!["acct-dup".to_string()],
            "the underlying sync must run exactly once, not once per request"
        );
    }

    #[tokio::test]
    async fn a_hung_account_does_not_block_other_accounts() {
        // Account A blocks forever; account B must still complete. With the
        // cap at 2 both can hold a permit at once, proving the fix for the
        // head-of-line blocking regression.
        let syncer = Arc::new(MockSyncer::new().blocking("acct-A"));
        let dispatcher = dispatcher_with(syncer.clone(), 2, Duration::from_secs(30));

        let d_a = dispatcher.clone();
        let a = tokio::spawn(async move { d_a.sync_account("acct-A").await });

        let b = dispatcher.sync_account("acct-B").await;
        assert!(
            matches!(b, Ok(1)),
            "account B completes even while account A is hung, got {b:?}"
        );

        // A is still blocked; it has not resolved.
        assert!(!a.is_finished(), "account A's sync is still in flight");
        a.abort();
    }

    #[tokio::test]
    async fn a_hung_account_times_out_without_poisoning_the_dispatcher() {
        let syncer = Arc::new(MockSyncer::new().blocking("acct-hung"));
        let dispatcher = dispatcher_with(syncer.clone(), 2, Duration::from_millis(60));

        let hung = dispatcher.sync_account("acct-hung").await;
        assert!(
            matches!(hung, Err(SyncDispatchError::Timeout { .. })),
            "a hung account surfaces a typed Timeout, got {hung:?}"
        );

        // The dispatcher is not poisoned: a subsequent sync of a healthy
        // account still works.
        let ok = dispatcher.sync_account("acct-healthy").await;
        assert!(
            matches!(ok, Ok(1)),
            "a later sync still works after a timeout, got {ok:?}"
        );
    }

    #[tokio::test]
    async fn surfaces_the_underlying_sync_error_typed() {
        let syncer = Arc::new(MockSyncer::new().failing("acct-bad"));
        let dispatcher = dispatcher_with(syncer, 2, Duration::from_secs(30));

        let err = dispatcher.sync_account("acct-bad").await;
        assert!(
            matches!(err, Err(SyncDispatchError::Sync(_))),
            "an underlying sync failure surfaces as SyncDispatchError::Sync, got {err:?}"
        );
    }

    #[tokio::test]
    async fn sync_status_reports_idle_for_an_unknown_account() {
        let syncer = Arc::new(MockSyncer::new());
        let dispatcher = dispatcher_with(syncer, 2, Duration::from_secs(30));

        let status = dispatcher.sync_status("never-seen");
        assert_eq!(status.phase, AccountSyncPhase::Idle);
        assert!(status.last_success_at.is_none());
        assert!(status.last_error.is_none());
    }

    #[tokio::test]
    async fn sync_status_reports_syncing_while_a_sync_is_in_flight() {
        let syncer = Arc::new(MockSyncer::new().blocking("acct-live"));
        let dispatcher = dispatcher_with(syncer.clone(), 2, Duration::from_secs(30));

        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.sync_account("acct-live").await });

        // Wait until the mock has actually entered the sync (it blocks there).
        while syncer.call_count() == 0 {
            sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            dispatcher.sync_status("acct-live").phase,
            AccountSyncPhase::Syncing
        );

        syncer.gate.notify_one();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn sync_status_reports_idle_with_last_synced_after_success() {
        let syncer = Arc::new(MockSyncer::new());
        let dispatcher = dispatcher_with(syncer, 2, Duration::from_secs(30));

        assert!(matches!(dispatcher.sync_account("acct-ok").await, Ok(1)));

        let status = dispatcher.sync_status("acct-ok");
        assert_eq!(status.phase, AccountSyncPhase::Idle);
        assert!(status.last_success_at.is_some());
        assert!(status.last_error.is_none());
    }

    #[tokio::test]
    async fn sync_status_reports_error_after_a_failed_attempt() {
        let syncer = Arc::new(MockSyncer::new().failing("acct-bad"));
        let dispatcher = dispatcher_with(syncer, 2, Duration::from_secs(30));

        assert!(matches!(
            dispatcher.sync_account("acct-bad").await,
            Err(SyncDispatchError::Sync(_))
        ));

        let status = dispatcher.sync_status("acct-bad");
        assert_eq!(status.phase, AccountSyncPhase::Error);
        assert!(status.last_error.is_some());
        assert!(status.last_success_at.is_none());
    }

    #[tokio::test]
    async fn rejects_new_work_once_shutdown_is_requested() {
        let event_bus = Arc::new(EventBus::new());
        let (controller, shutdown) = crate::lifecycle::ShutdownController::new(event_bus);
        let syncer = Arc::new(MockSyncer::new());
        let dispatcher = SyncDispatcher::new(syncer.clone(), shutdown, 2, Duration::from_secs(30));

        controller.trigger();

        let rejected = dispatcher.sync_account("acct-late").await;
        assert!(
            matches!(rejected, Err(SyncDispatchError::ShuttingDown)),
            "no new sync is admitted after shutdown, got {rejected:?}"
        );
        assert_eq!(
            syncer.call_count(),
            0,
            "the underlying syncer must not even be invoked after shutdown"
        );
    }

    #[tokio::test]
    async fn shutdown_unblocks_an_in_flight_sync_promptly() {
        let event_bus = Arc::new(EventBus::new());
        let (controller, shutdown) = crate::lifecycle::ShutdownController::new(event_bus);
        let syncer = Arc::new(MockSyncer::new().blocking("acct-drain"));
        let dispatcher = SyncDispatcher::new(syncer, shutdown, 2, Duration::from_secs(300));

        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.sync_account("acct-drain").await });

        // Let the sync reach its blocking point, then request shutdown.
        sleep(Duration::from_millis(30)).await;
        controller.trigger();

        let outcome = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("in-flight sync must unblock on shutdown, not hang")
            .expect("task joins");
        assert!(
            matches!(outcome, Err(SyncDispatchError::ShuttingDown)),
            "a sync racing shutdown resolves as ShuttingDown, got {outcome:?}"
        );
    }

    fn jmap_account(id: &str) -> AccountConfig {
        AccountConfig {
            id: id.to_string(),
            name: "Test".to_string(),
            email_address: format!("{id}@nuncio.mx"),
            keyring_secret_key: format!("nuncio/{id}"),
            sync_interval_secs: 60,
            filters_enabled: false,
            transport: Transport::Jmap(JmapTransport {
                endpoint_host: "jmap.nuncio.mx".to_string(),
            }),
        }
    }

    fn caldav_account(id: &str) -> AccountConfig {
        AccountConfig {
            transport: Transport::Dav(DavTransport {
                collection_url: "https://dav.nuncio.mx/cal/".to_string(),
            }),
            ..jmap_account(id)
        }
    }

    #[tokio::test]
    async fn sync_all_configured_dispatches_every_mail_account_and_skips_dav() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("ephemeral db");
        db.save_account(&jmap_account("acct-mail-1"))
            .await
            .expect("save mail account 1");
        db.save_account(&jmap_account("acct-mail-2"))
            .await
            .expect("save mail account 2");
        db.save_account(&caldav_account("acct-dav"))
            .await
            .expect("save dav account");

        let event_bus = EventBus::new();
        let mut events = event_bus.subscribe_events();
        let syncer = Arc::new(MockSyncer::new());
        let dispatcher = dispatcher_with(syncer.clone(), 4, Duration::from_secs(30));

        let total = sync_all_configured(&dispatcher, &db, &event_bus).await;
        assert_eq!(total, 2, "both mail accounts synced one message each");

        let mut synced: Vec<String> = syncer.call_ids();
        synced.sort();
        assert_eq!(
            synced,
            vec!["acct-mail-1".to_string(), "acct-mail-2".to_string()],
            "only the mail accounts are dispatched; the CalDAV account is skipped"
        );

        // The batch is wrapped in a single all-accounts start/complete pair.
        assert_eq!(
            events.recv().await.expect("start event"),
            CoreEvent::SyncStarted { account_id: None }
        );
    }
}
