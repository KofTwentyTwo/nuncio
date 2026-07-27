//! Centralized Standalone Background Daemon Server Binary (`nunciod`).
//! Owns storage persistence, background sync loops, protocol connections,
//! filter automation engine, outbox retries, and multi-client IPC socket distribution.

use nuncio_core::ipc::server::CustomRpcHandler;
use nuncio_core::ipc::IpcDaemonServer;
use nuncio_core::{CoreCommand, CoreEvent, EventBus};
use nuncio_filter::{FilterEngine, NsqlParser, NsqlValidator, OutboxManager, ValidationOptions};
use serde_json::json;
use std::sync::Arc;

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
    // PERSISTENT database path (backlog stories 1.C.1 / 1.C.2, GH #156 /
    // GH #157): defaults to `~/.nuncio/nuncio.db` -- NOT a temp/ephemeral
    // path -- so accounts (and everything else) survive a daemon restart.
    // `NUNCIO_DB_PATH` overrides it, which tests/CI use to point at an
    // isolated temp path instead of touching a real user's home directory.
    let db_path = std::env::var("NUNCIO_DB_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| nunciod::default_db_path());

    let orchestrator = nunciod::SelfHealingSyncOrchestrator::new(&db_path, event_bus.clone());
    let (db, _summary) = orchestrator.initialize_and_recover().await?;

    // Shared OS keyring vault (backlog stories 1.C.1 / 1.C.2 / 1.C.3, GH
    // #156 / #157 / #158). ONE `SecretManager` instance is used for every
    // subsystem that reads/writes account credentials in this process --
    // the gRPC `Accounts` service below AND the real inbound-sync command
    // loop -- rather than provisioning separate wrapper instances.
    let account_secrets = Arc::new(nuncio_store::vault::SecretManager::production());

    // Load active rules from SQLite
    let initial_rules = db.list_filter_rules().await.unwrap_or_default();
    let filter_engine = Arc::new(FilterEngine::new(initial_rules)?);

    // Real inbound-sync `CoreCommand` consumer (backlog story 1.C.3, GH
    // #158). This is the ONE place that claims `EventBus`'s command
    // receiver, so it is what finally makes `CoreCommand::SyncAll` /
    // `CoreCommand::SyncAccount` do REAL work -- connecting a real mail
    // backend, fetching folders/messages, and persisting them via
    // `DatabaseEngine::save_email` -- instead of only flipping a status
    // flag. It also finally gives `SelfHealingSyncOrchestrator::
    // trigger_background_resync`'s post-recovery `send_command` calls a
    // consumer: previously nothing read this channel, so that resync
    // trigger was inert.
    if let Some(mut command_rx) = command_rx {
        let db_sync = db.clone();
        let secrets_sync = account_secrets.clone();
        let event_bus_sync = event_bus.clone();
        let _sync_command_task = tokio::spawn(async move {
            while let Some(cmd) = command_rx.recv().await {
                match cmd {
                    CoreCommand::SyncAll => {
                        let synced = nunciod::sync::run_all_accounts_sync(
                            &db_sync,
                            &secrets_sync,
                            &event_bus_sync,
                        )
                        .await;
                        tracing::info!("SyncAll completed: {} message(s) synced", synced);
                    }
                    CoreCommand::SyncAccount { account_id } => {
                        match nunciod::sync::run_account_sync(
                            &db_sync,
                            &secrets_sync,
                            &event_bus_sync,
                            &account_id,
                        )
                        .await
                        {
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
        });
    } else {
        tracing::error!(
            "EventBus command receiver was already taken; real sync command processing will not run"
        );
    }

    // Background Outbox Worker Task (#273)
    let db_outbox = db.clone();
    let _outbox_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Ok(pending) = db_outbox.list_pending_mutations(50).await {
                for item in pending {
                    let next_retry = item.retry_count + 1;
                    if next_retry > OutboxManager::MAX_RETRIES {
                        let _ = db_outbox
                            .update_mutation_status(&item.id, "failed", next_retry)
                            .await;
                        continue;
                    }
                    let backoff_ms = OutboxManager::calculate_backoff_ms(item.retry_count);
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    // Outbound IMAP/JMAP remote mutation execution (actually
                    // applying a filter action like move/delete/flag on the
                    // real mail server) is NOT wired yet -- that is backlog
                    // story 1.C.5. This worker deliberately does NOT mark
                    // mutations "completed": doing so would fabricate
                    // success for work that never happened. It only bumps
                    // the retry counter (so an item that keeps failing still
                    // eventually flips to "failed" once retries are
                    // exhausted) and leaves the item "pending" so it is
                    // retried on the next poll tick once 1.C.5 wires real
                    // execution.
                    let _ = db_outbox
                        .update_mutation_status(&item.id, "pending", next_retry)
                        .await;
                }
            }
        }
    });

    // Background Auto-Update Check Listener Loop (24h interval).
    //
    // SECURITY (GH #140 / backlog story 0.B.2): `UpdateEngine`'s checksum
    // verification is currently fail-open (installs proceed unverified if
    // `SHA256SUMS.txt` is missing from the release). That will be fixed
    // properly in Phase 4. Until then, nunciod must never autonomously
    // check for or install updates, so this loop only runs when an
    // operator explicitly opts in via `NUNCIO_AUTO_UPDATE_ENABLED=1`.
    let _update_task = if nunciod::auto_update_task_enabled() {
        let event_bus_update = event_bus.clone();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400));
            interval.tick().await;
            loop {
                interval.tick().await;
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
        }))
    } else {
        tracing::info!(
            "Autonomous auto-update check loop disabled (set {} to enable); \
             pending Phase 4 checksum fail-open fix, see GH #140",
            nunciod::AUTO_UPDATE_ENV_VAR
        );
        None
    };

    // Custom RPC Handler for filter.* and update.* methods

    let db_rpc = db.clone();
    let engine_rpc = filter_engine.clone();
    let event_bus_rpc = event_bus.clone();

    let handler: CustomRpcHandler = Arc::new(move |method, params| {
        let db = db_rpc.clone();
        let engine = engine_rpc.clone();
        let event_bus = event_bus_rpc.clone();
        let method_str = method.to_string();

        Box::pin(async move {
            match method_str.as_str() {
                "filter.list" => match db.list_filter_rules().await {
                    Ok(rules) => Some(Ok(json!(rules))),
                    Err(e) => Some(Err(e.to_string())),
                },
                "filter.create" => {
                    let name = params
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Untitled Rule");
                    let nsql = params.get("nsql").and_then(|v| v.as_str()).unwrap_or("");
                    let priority =
                        params.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

                    match NsqlParser::parse_rule(name, priority, nsql) {
                        Ok(rule) => {
                            let val_opts = ValidationOptions::default();
                            if let Err(val_err) = NsqlValidator::validate(&rule, &val_opts) {
                                return Some(Err(val_err.to_string()));
                            }
                            if let Err(e) = db.save_filter_rule(&rule).await {
                                return Some(Err(e.to_string()));
                            }
                            if let Ok(all_rules) = db.list_filter_rules().await {
                                let _ = engine.reload_rules(all_rules);
                            }
                            Some(Ok(json!(rule)))
                        }
                        Err(parse_err) => Some(Err(parse_err.to_string())),
                    }
                }
                "filter.edit" => {
                    let id = match params.get("id").and_then(|v| v.as_str()) {
                        Some(id) => id,
                        None => return Some(Err("missing rule id".to_string())),
                    };
                    let name = params
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Updated Rule");
                    let nsql = params.get("nsql").and_then(|v| v.as_str()).unwrap_or("");
                    let priority =
                        params.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

                    match NsqlParser::parse_rule(name, priority, nsql) {
                        Ok(mut rule) => {
                            rule.id = id.to_string();
                            let val_opts = ValidationOptions::default();
                            if let Err(val_err) = NsqlValidator::validate(&rule, &val_opts) {
                                return Some(Err(val_err.to_string()));
                            }
                            if let Err(e) = db.save_filter_rule(&rule).await {
                                return Some(Err(e.to_string()));
                            }
                            if let Ok(all_rules) = db.list_filter_rules().await {
                                let _ = engine.reload_rules(all_rules);
                            }
                            Some(Ok(json!(rule)))
                        }
                        Err(parse_err) => Some(Err(parse_err.to_string())),
                    }
                }
                "filter.delete" => {
                    let id = match params.get("id").and_then(|v| v.as_str()) {
                        Some(id) => id,
                        None => return Some(Err("missing rule id".to_string())),
                    };
                    if let Err(e) = db.delete_filter_rule(id).await {
                        return Some(Err(e.to_string()));
                    }
                    if let Ok(all_rules) = db.list_filter_rules().await {
                        let _ = engine.reload_rules(all_rules);
                    }
                    Some(Ok(json!({ "status": "deleted" })))
                }
                "filter.preview" => {
                    if let Some(email_val) = params.get("email") {
                        if let Ok(email) =
                            serde_json::from_value::<nuncio_core::model::Email>(email_val.clone())
                        {
                            let preview = engine.preview(&email);
                            return Some(Ok(json!(preview)));
                        }
                    }
                    Some(Err("invalid email payload for preview".to_string()))
                }
                "filter.logs" => {
                    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                    match db.list_filter_execution_logs(limit).await {
                        Ok(logs) => Some(Ok(json!(logs))),
                        Err(e) => Some(Err(e.to_string())),
                    }
                }
                "filter.triage_keyset" => {
                    // Keyset Chunking Triage Engine (#272)
                    let batch_size = params
                        .get("batch_size")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1000) as usize;
                    let mut last_id = String::new();
                    let mut processed = 0;
                    let mut matched_count = 0;

                    loop {
                        let chunk = match db.get_message_chunk(&last_id, batch_size).await {
                            Ok(c) => c,
                            Err(e) => return Some(Err(e.to_string())),
                        };
                        let Some(last) = chunk.last() else {
                            break;
                        };
                        last_id = last.id.clone();
                        for email in &chunk {
                            processed += 1;
                            let matches = engine.evaluate(email);
                            for (rule, actions) in matches {
                                matched_count += 1;
                                for action in actions {
                                    let action_str = action.to_nsql();
                                    let _ = db
                                        .save_filter_execution_log(&rule.id, &email.id, &action_str)
                                        .await;
                                    let outbox_item = OutboxManager::create_mutation(
                                        &rule.id,
                                        &email.id,
                                        &action_str,
                                        None,
                                    );
                                    let _ = db.save_pending_mutation(&outbox_item).await;

                                    event_bus.publish_event(CoreEvent::FilterExecuted {
                                        rule_id: rule.id.clone(),
                                        message_id: email.id.clone(),
                                        action_taken: action_str,
                                    });
                                }
                            }
                        }
                        event_bus.publish_event(CoreEvent::BatchFilterProgress {
                            processed,
                            total: processed,
                            matched: matched_count,
                        });
                    }

                    Some(Ok(
                        json!({ "processed": processed, "matched": matched_count }),
                    ))
                }
                "update.check" => match nuncio_core::UpdateEngine::new() {
                    Ok(updater) => match updater.check_for_updates().await {
                        Ok(res) => Some(Ok(json!(res))),
                        Err(e) => Some(Err(e.to_string())),
                    },
                    Err(e) => Some(Err(e.to_string())),
                },
                "update.apply" => {
                    // SECURITY (GH #140 / backlog story 0.B.2): `UpdateEngine`'s
                    // checksum verification is currently fail-open (an update
                    // installs unverified if `SHA256SUMS.txt` is missing from
                    // the release). Until that is fixed in Phase 4, nunciod
                    // must never perform an on-demand install either. Use
                    // `update.check` for a read-only version check, and
                    // install updates manually in the meantime.
                    Some(Err(
                        "auto-update is disabled pending Phase 4 (fail-open checksum fix, \
                         see GH #140); install updates manually for now"
                            .to_string(),
                    ))
                }
                _ => None,
            }
        })
    });

    // gRPC `nuncio.v1.System` + `nuncio.v1.Accounts` server (backlog
    // stories 1.A.2 / GH-149 and 1.C.1 + 1.C.2 / GH-156 + GH-157).
    //
    // Runs ALONGSIDE the existing JSON-RPC IPC server below; the migration
    // off the hand-rolled JSON-RPC transport happens in later stories. The
    // bearer token is minted (or loaded, on subsequent runs) from the real
    // OS keyring vault via the shared `account_secrets` `SecretManager`
    // (same instance the real-sync command loop above uses), fails closed
    // if the keyring is unavailable, and is never logged. `Accounts` reuses
    // this SAME `SecretManager` to write account password credentials to
    // the OS keyring (never to SQLite) -- see `nunciod::grpc`'s security
    // comment (GH #165) on why EVERY mounted service shares this one
    // `BearerAuthInterceptor`.
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
    let grpc_event_bus = event_bus.clone();
    let grpc_db = db.clone();
    // The gRPC `Filters` service (backlog story 2.A, GH #171) shares this
    // SAME `filter_engine` instance with the JSON-RPC `filter.*` handler
    // above, so a rule created/deleted through EITHER transport reloads the
    // one live `ArcSwap`-backed rule set both transports evaluate against.
    let grpc_filter_engine = filter_engine.clone();
    let _grpc_task = tokio::spawn(async move {
        if let Err(e) = nunciod::grpc::serve(
            &grpc_addr,
            grpc_event_bus,
            grpc_db,
            grpc_filter_engine,
            grpc_secrets,
            grpc_token,
        )
        .await
        {
            tracing::error!("nunciod gRPC server failed: {}", e);
        }
    });

    let addr = std::env::var("NUNCIO_IPC_ADDR").unwrap_or_else(|_| "127.0.0.1:9422".to_string());
    let server = IpcDaemonServer::with_handler(event_bus.clone(), &addr, handler);

    tracing::info!("nunciod listening on {}", addr);
    server.run_server().await?;

    Ok(())
}
