//! Centralized Standalone Background Daemon Server Binary (`nunciod`).
//! Owns storage persistence, background sync loops, protocol connections,
//! filter automation engine, outbox retries, and multi-client IPC socket distribution.

use nuncio_core::ipc::server::CustomRpcHandler;
use nuncio_core::ipc::IpcDaemonServer;
use nuncio_core::{CoreEvent, EventBus};
use nuncio_filter::{
    FilterEngine, NsqlParser, NsqlValidator, OutboxManager, ValidationOptions,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Nuncio Central Daemon Service (nunciod)...");

    let event_bus = Arc::new(EventBus::new());
    let db_path = std::env::var("NUNCIO_DB_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::temp_dir().join("nuncio_main.db")
        });

    let orchestrator = nunciod::SelfHealingSyncOrchestrator::new(&db_path, event_bus.clone());
    let (db, _summary) = orchestrator.initialize_and_recover().await?;

    // Load active rules from SQLite
    let initial_rules = db.list_filter_rules().await.unwrap_or_default();
    let filter_engine = Arc::new(FilterEngine::new(initial_rules).expect("initialize filter engine"));

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
                        let _ = db_outbox.update_mutation_status(&item.id, "failed", next_retry).await;
                        continue;
                    }
                    let backoff_ms = OutboxManager::calculate_backoff_ms(item.retry_count);
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    // Simulate remote IMAP/JMAP mutation execution
                    let _ = db_outbox.update_mutation_status(&item.id, "completed", next_retry).await;
                }
            }
        }
    });

    // Custom RPC Handler for filter.* methods
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
                "filter.list" => {
                    match db.list_filter_rules().await {
                        Ok(rules) => Some(Ok(json!(rules))),
                        Err(e) => Some(Err(e.to_string())),
                    }
                }
                "filter.create" => {
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("Untitled Rule");
                    let nsql = params.get("nsql").and_then(|v| v.as_str()).unwrap_or("");
                    let priority = params.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

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
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("Updated Rule");
                    let nsql = params.get("nsql").and_then(|v| v.as_str()).unwrap_or("");
                    let priority = params.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

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
                        if let Ok(email) = serde_json::from_value::<nuncio_core::model::Email>(email_val.clone()) {
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
                    let batch_size = params.get("batch_size").and_then(|v| v.as_u64()).unwrap_or(1000) as usize;
                    let mut last_id = String::new();
                    let mut processed = 0;
                    let mut matched_count = 0;

                    loop {
                        let chunk = match db.get_message_chunk(&last_id, batch_size).await {
                            Ok(c) => c,
                            Err(e) => return Some(Err(e.to_string())),
                        };
                        if chunk.is_empty() {
                            break;
                        }
                        last_id = chunk.last().unwrap().id.clone();
                        for email in &chunk {
                            processed += 1;
                            let matches = engine.evaluate(email);
                            for (rule, actions) in matches {
                                matched_count += 1;
                                for action in actions {
                                    let action_str = action.to_nsql();
                                    let _ = db.save_filter_execution_log(&rule.id, &email.id, &action_str, "secret_ledger_key").await;
                                    let outbox_item = OutboxManager::create_mutation(&rule.id, &email.id, &action_str, None);
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

                    Some(Ok(json!({ "processed": processed, "matched": matched_count })))
                }
                _ => None,
            }
        })
    });

    let addr = std::env::var("NUNCIO_IPC_ADDR").unwrap_or_else(|_| "127.0.0.1:9422".to_string());
    let server = IpcDaemonServer::with_handler(event_bus.clone(), &addr, handler);

    tracing::info!("nunciod listening on {}", addr);
    server.run_server().await?;

    Ok(())
}
