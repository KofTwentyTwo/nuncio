//! Centralized Standalone Background Daemon Server Binary (`nunciod`).
//! Owns storage persistence, background sync loops, protocol connections,
//! filter automation engine, outbox retries, and the multi-client gRPC API.

use nuncio_core::{CoreCommand, CoreEvent, EventBus};
use nuncio_filter::FilterEngine;
use nunciod::lifecycle::{install_signal_handlers, ShutdownController};
use std::sync::Arc;
use std::time::Duration;

/// Bound on how long shutdown waits for in-flight work (gRPC requests still
/// being served, an outbox drain pass in progress, a background task's
/// current loop iteration) to finish on its own before the process moves on
/// to closing the database and exiting anyway. Keeps a stuck task from
/// turning a graceful shutdown into a hang.
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Nuncio Central Daemon Service (nunciod)...");

    // The `CoreCommand` receiver must be claimed BEFORE the `EventBus` is
    // wrapped in an `Arc` (`take_command_receiver` needs `&mut self`, which
    // an `Arc<EventBus>` shared across every subsystem below cannot offer).
    let mut event_bus_owned = EventBus::new();
    let command_rx = event_bus_owned.take_command_receiver();
    let event_bus = Arc::new(event_bus_owned);

    // Single shutdown signal shared by every long-running task spawned
    // below (the real-sync command consumer, the outbox worker, the
    // update-check loop, and the gRPC server itself), so a Ctrl+C/SIGTERM
    // drains all of them cleanly instead of the OS killing the process
    // mid-write. `install_signal_handlers` also drives the existing
    // `CoreCommand::Shutdown` / `CoreEvent::ShuttingDown` event-bus path,
    // previously wired but never triggered outside tests.
    let (shutdown_controller, shutdown_signal) = ShutdownController::new(event_bus.clone());
    let shutdown_controller = Arc::new(shutdown_controller);
    let signal_task = tokio::spawn(install_signal_handlers(shutdown_controller));
    // PERSISTENT database path: defaults to `~/.nuncio/nuncio.db` -- NOT a
    // temp/ephemeral path -- so accounts (and everything else) survive a
    // daemon restart.
    // `NUNCIO_DB_PATH` overrides it, which tests/CI use to point at an
    // isolated temp path instead of touching a real user's home directory.
    let db_path = std::env::var("NUNCIO_DB_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| nunciod::default_db_path());

    // Exclusive single-instance lock on the database path, acquired BEFORE
    // the store is opened. Held for the entire process lifetime (dropped at
    // the end of `main`, releasing it on any clean exit) so a second
    // `nunciod` pointed at the same database fails fast instead of the two
    // instances corrupting SQLite state between them.
    let _instance_lock =
        nunciod::lock::InstanceLock::acquire(&db_path).map_err(|e| format!("{e}"))?;

    let orchestrator = nunciod::SelfHealingSyncOrchestrator::new(&db_path, event_bus.clone());
    let (db, _summary) = orchestrator.initialize_and_recover().await?;

    // Shared OS keyring vault. ONE `SecretManager` instance is used for
    // every subsystem that reads/writes account credentials in this
    // process -- the gRPC `Accounts` service below AND the real
    // inbound-sync command loop -- rather than provisioning separate
    // wrapper instances.
    let account_secrets = Arc::new(nuncio_store::vault::SecretManager::production());

    // Load active rules from SQLite. See `load_initial_filter_rules` for why
    // a load failure must fail startup rather than silently become an empty
    // rule set.
    let initial_rules = nunciod::load_initial_filter_rules(&db).await?;
    let filter_engine = Arc::new(FilterEngine::new(initial_rules)?);

    // Single bounded-concurrency sync dispatcher shared by BOTH the
    // client-facing `Mail/Sync` RPC (via the gRPC server below) and this
    // process's `CoreCommand` sync consumer, so account syncs from either
    // origin queue against one concurrency cap, coalesce per account, and
    // observe one per-account timeout -- instead of each entry point launching
    // syncs independently and head-of-line-blocking one another. It races
    // in-flight syncs against the SAME shutdown signal every other subsystem
    // shares.
    let sync_dispatcher = {
        let syncer = Arc::new(nunciod::sync_dispatcher::ProductionAccountSyncer::new(
            db.clone(),
            account_secrets.clone(),
            event_bus.clone(),
            filter_engine.clone(),
        ));
        Arc::new(nunciod::sync_dispatcher::SyncDispatcher::from_env(
            syncer,
            shutdown_signal.clone(),
        ))
    };

    // Real inbound-sync `CoreCommand` consumer. This is the ONE place that
    // claims `EventBus`'s command receiver, so it is what finally makes
    // `CoreCommand::SyncAll` /
    // `CoreCommand::SyncAccount` do REAL work -- connecting a real mail
    // backend, fetching folders/messages, and persisting them via
    // `DatabaseEngine::save_email` -- instead of only flipping a status
    // flag. It also finally gives `SelfHealingSyncOrchestrator::
    // trigger_background_resync`'s post-recovery `send_command` calls a
    // consumer: previously nothing read this channel, so that resync
    // trigger was inert.
    let sync_command_task = if let Some(mut command_rx) = command_rx {
        let db_sync = db.clone();
        let event_bus_sync = event_bus.clone();
        let dispatcher_sync = sync_dispatcher.clone();
        let mut shutdown = shutdown_signal.clone();
        Some(tokio::spawn(async move {
            loop {
                let cmd = tokio::select! {
                    _ = shutdown.wait() => {
                        tracing::info!("sync command consumer exiting on shutdown signal");
                        break;
                    }
                    received = command_rx.recv() => match received {
                        Some(cmd) => cmd,
                        None => break,
                    },
                };
                match cmd {
                    CoreCommand::SyncAll => {
                        let synced = nunciod::sync_dispatcher::sync_all_configured(
                            &dispatcher_sync,
                            &db_sync,
                            &event_bus_sync,
                        )
                        .await;
                        tracing::info!("SyncAll completed: {} message(s) synced", synced);
                    }
                    CoreCommand::SyncAccount { account_id } => {
                        match dispatcher_sync.sync_account(&account_id).await {
                            Ok(count) => tracing::info!(
                                "SyncAccount({}) completed: {} message(s) synced",
                                account_id,
                                count
                            ),
                            Err(e) => {
                                tracing::warn!("SyncAccount({}) failed: {}", account_id, e)
                            }
                        }
                    }
                    _ => {}
                }
            }
        }))
    } else {
        tracing::error!(
            "EventBus command receiver was already taken; real sync command processing will not run"
        );
        None
    };

    // Background Outbox Worker Task. Each tick drains the pending remote
    // mutations the filter engine enqueued (move/copy/flag/unflag/delete on the
    // real mail server, forward via SMTP, or a signed webhook call) and applies
    // them against the resolved account's backend. A mutation is marked
    // "completed" ONLY when the real operation genuinely succeeded; a transient
    // failure is retried on later ticks and a permanent one (gone message,
    // unknown action) fails honestly -- success is never fabricated.
    let db_outbox = db.clone();
    let outbox_env = Arc::new(nunciod::outbox::ProductionExecutionEnv::new(
        db.clone(),
        account_secrets.clone(),
    ));
    let mut outbox_shutdown = shutdown_signal.clone();
    let mut outbox_drain_shutdown = shutdown_signal.clone();
    let outbox_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = outbox_shutdown.wait() => {
                    tracing::info!("outbox worker exiting on shutdown signal");
                    break;
                }
                _ = interval.tick() => {
                    // A drain pass already in progress is never aborted
                    // mid-write by a shutdown signal that arrives after this
                    // branch was chosen -- but `execute_pending_mutations`
                    // itself races each item's execution against
                    // `outbox_drain_shutdown`, so a hung remote op cannot
                    // block this pass (and therefore shutdown) indefinitely.
                    let summary = nunciod::outbox::execute_pending_mutations(
                        &db_outbox,
                        outbox_env.as_ref(),
                        50,
                        &mut outbox_drain_shutdown,
                    )
                    .await;
                    if summary.completed > 0 || summary.failed > 0 {
                        tracing::info!(
                            "outbox drain: {} completed, {} retried, {} failed",
                            summary.completed,
                            summary.retried,
                            summary.failed
                        );
                    }
                }
            }
        }
    });

    // Background Auto-Update Check Listener Loop (24h interval).
    //
    // SECURITY: `UpdateEngine`'s checksum verification is currently
    // fail-open (installs proceed unverified if `SHA256SUMS.txt` is
    // missing from the release). Until that checksum-verification gap is
    // closed, nunciod must never autonomously check for or install
    // updates, so this loop only runs when an operator explicitly opts in
    // via `NUNCIO_AUTO_UPDATE_ENABLED=1`.
    let update_task = if nunciod::auto_update_task_enabled() {
        let event_bus_update = event_bus.clone();
        let mut update_shutdown = shutdown_signal.clone();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400));
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = update_shutdown.wait() => {
                        tracing::info!("update-check loop exiting on shutdown signal");
                        break;
                    }
                    _ = interval.tick() => {
                        if let Ok(updater) = nuncio_core::UpdateEngine::new() {
                            if let Ok(result) = updater.check_for_updates().await {
                                if result.update_available {
                                    if let Some(info) = result.release_info {
                                        event_bus_update.publish_event(CoreEvent::UpdateAvailable {
                                            version: info.version,
                                            release_notes: info.release_notes,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }))
    } else {
        tracing::info!(
            "Autonomous auto-update check loop disabled (set {} to enable); \
             update verification is not yet safe",
            nunciod::AUTO_UPDATE_ENV_VAR
        );
        None
    };

    // gRPC `nuncio.v1.System`, `nuncio.v1.Accounts`, `nuncio.v1.Mail`,
    // `nuncio.v1.Filters`, `nuncio.v1.Export`, and `nuncio.v1.Audit` server.
    // This is the daemon's ONLY client-facing transport: the bearer token is
    // minted (or loaded, on subsequent runs) from the real OS keyring vault
    // via the shared `account_secrets` `SecretManager` (same instance the
    // real-sync command loop above uses), fails closed if the keyring is
    // unavailable, and is never logged. `Accounts` reuses this SAME
    // `SecretManager` to write account password credentials to the OS
    // keyring (never to SQLite) -- see `nunciod::grpc`'s security comment on
    // why EVERY mounted service shares this one `BearerAuthInterceptor`.
    let grpc_secrets = account_secrets.clone();
    let grpc_token_bytes = grpc_secrets
        .get_or_create_key_bytes(nuncio_store::vault::GRPC_TOKEN_ACCOUNT, 32)
        .map_err(|e| format!("failed to provision gRPC bearer token from vault: {e}"))?;
    let grpc_token = hex::encode(grpc_token_bytes);
    let grpc_addr = nunciod::grpc::grpc_addr_from_env();
    tracing::info!(
        "nunciod gRPC (nuncio.v1.System, nuncio.v1.Accounts, nuncio.v1.Mail, nuncio.v1.Filters, \
         nuncio.v1.Export, nuncio.v1.Audit) starting on {} (loopback only)",
        grpc_addr
    );

    // The gRPC server is the long-running foreground task that keeps this
    // process alive; every other subsystem above is a background worker
    // spawned on top of it. `serve_with_shutdown` stops accepting new
    // connections and starts draining in-flight requests as soon as
    // `shutdown_signal` fires; the `select!` below bounds how long that
    // drain is allowed to take before this process moves on regardless.
    let db_for_serve = db.clone();
    let mut grpc_shutdown_future = shutdown_signal.clone();
    let mut grace_timer_signal = shutdown_signal.clone();
    let grpc_result = tokio::select! {
        result = nunciod::grpc::serve_with_shutdown(
            &grpc_addr,
            event_bus,
            db_for_serve,
            filter_engine,
            grpc_secrets,
            grpc_token,
            sync_dispatcher,
            async move { grpc_shutdown_future.wait().await },
        ) => result,
        _ = async move {
            grace_timer_signal.wait().await;
            tokio::time::sleep(SHUTDOWN_GRACE_PERIOD).await;
        } => {
            tracing::warn!(
                "gRPC server did not finish draining in-flight requests within the {:?} \
                 shutdown grace period; closing the database and exiting anyway",
                SHUTDOWN_GRACE_PERIOD
            );
            Ok(())
        }
    };

    // Bound how long the background workers get to notice the shutdown
    // signal and exit their loops before the database is closed out from
    // under them.
    let _ = tokio::time::timeout(SHUTDOWN_GRACE_PERIOD, async {
        if let Some(task) = sync_command_task {
            let _ = task.await;
        }
        let _ = outbox_task.await;
        if let Some(task) = update_task {
            let _ = task.await;
        }
    })
    .await;
    signal_task.abort();

    // Force the WAL checkpoint before the process exits so no committed
    // page is left for SQLite's own deferred "last connection closes"
    // teardown to race (see `DatabaseEngine::close`'s doc comment).
    db.close().await;

    grpc_result.map_err(|e| format!("nunciod gRPC server failed: {e}"))?;

    Ok(())
}
