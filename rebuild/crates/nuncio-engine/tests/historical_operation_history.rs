#![allow(clippy::unwrap_used)]
#[allow(dead_code)]
#[path = "support/historical_store.rs"]
mod historical;
use historical::*;
use nuncio_engine::store::{AttemptKind, Store, StoreError};
use rusqlite::{params, types::Value, Connection};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use zeroize::Zeroizing;

const NOW: i64 = 4000;
const CASES: [(&str, Option<&str>); 10] = [
    ("running", None),
    ("applied", None),
    ("retry_wait", None),
    ("conflict", None),
    ("uncertain", None),
    ("failed", None),
    ("cancelled", None),
    ("uncertain", Some("abandoned")),
    ("uncertain", Some("resend_requested")),
    ("applied", Some("manual_confirmed")),
];
fn id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012}")
}
fn request(index: usize) -> String {
    format!("00000001-0000-4000-8000-{index:012}")
}
fn pending(state: &str, disposition: Option<&str>) -> bool {
    disposition.is_none()
        && matches!(
            state,
            "queued" | "running" | "retry_wait" | "conflict" | "uncertain"
        )
}
fn enrich(path: &Path, version: u32) {
    fixture(path, version);
    let c = opened(path);
    for account in [ACCOUNT, IMAP] {
        for (index, &(state, disposition)) in CASES.iter().enumerate() {
            let op = id(index);
            let message_id = format!("<historical-{index}@example.test>");
            let wire = std::str::from_utf8(WIRE)
                .unwrap()
                .replace(
                    "Message-ID: <legacy@example.test>",
                    &format!("Message-ID: {message_id}"),
                )
                .into_bytes();
            let blob = format!("wire-{index}");
            c.execute(
                "INSERT INTO blobs VALUES(?1,?2,?3,?4)",
                params![
                    account,
                    blob,
                    hex::encode(Sha256::digest(&wire)),
                    wire.len() as i64
                ],
            )
            .unwrap();
            c.execute(
                "INSERT INTO blob_chunks VALUES(?1,?2,0,?3)",
                params![account, blob, wire],
            )
            .unwrap();
            c.execute("INSERT INTO operations(account_id,id,request_id,kind,resource_id,fingerprint,request_json,desired_json,state,version,created_at_ms,updated_at_ms,next_attempt_at_ms,needs_reconciliation,error_code,disposition) SELECT account_id,?2,?3,kind,resource_id,fingerprint,request_json,json_set(desired_json,'$.message_id',?7),?4,9,created_at_ms,9000,?5,?6,NULL,?8 FROM operations WHERE account_id=?1 AND id=?9",params![account,op,request(index),state,if state=="retry_wait" {Some(10000)} else {None},state=="uncertain"&&disposition.is_none(),message_id,disposition,OPERATION]).unwrap();
            c.execute("INSERT INTO send_payloads SELECT account_id,?2,draft_id,draft_version,sender,recipients_json,?3,thread_id,?4,?4 FROM send_payloads WHERE account_id=?1 AND operation_id=?5",params![account,op,message_id,blob,OPERATION]).unwrap();
            if state != "cancelled" {
                let outcome = match state {
                    "running" => None,
                    "applied" => Some("applied"),
                    "retry_wait" => Some("repeatable"),
                    "conflict" => Some("conflict"),
                    "failed" => Some("rejected"),
                    _ => Some("uncertain"),
                };
                c.execute("INSERT INTO operation_attempts(account_id,operation_id,ordinal,kind,started_at_ms,finished_at_ms,outcome) VALUES(?1,?2,1,'dispatch',6000,?3,?4)",params![account,op,if state=="running"{None}else{Some(8000)},outcome]).unwrap();
                if matches!(state, "running" | "applied" | "uncertain") {
                    let receipt = json!({"kind":if account==IMAP{"smtp_accepted"}else{"google_send"},"source":"acknowledgement","provider_id":format!("accepted-{index}"),"etag":null});
                    c.execute(
                        "INSERT INTO operation_receipts VALUES(?1,?2,1,1,7000,?3)",
                        params![account, op, receipt.to_string()],
                    )
                    .unwrap();
                    if account == IMAP && state == "applied" {
                        let receipt = json!({"kind":"imap_append","source":"acknowledgement","provider_id":format!("[1,\"imap\",{{\"account_id\":\"{IMAP}\",\"mailbox_id\":\"{MAILBOX}\",\"uid_validity\":9001,\"uid\":{}}}]",index+2),"etag":null});
                        c.execute(
                            "INSERT INTO operation_receipts VALUES(?1,?2,1,2,8000,?3)",
                            params![account, op, receipt.to_string()],
                        )
                        .unwrap();
                    }
                }
            }
            if account == IMAP && version >= 17 {
                let step = match state {
                    "running" => "accepted",
                    "applied" => "copied",
                    "uncertain" => "appending",
                    _ => "prepared",
                };
                let placement=(step=="copied").then(||json!({"account_id":IMAP,"mailbox_id":MAILBOX,"uid_validity":9001,"uid":index+2}).to_string());
                c.execute("INSERT INTO smtp_submissions(account_id,operation_id,intent_json,step,placement_json) SELECT account_id,?2,intent_json,?3,?4 FROM smtp_submissions WHERE account_id=?1 AND operation_id=?5",params![account,op,step,placement,OPERATION]).unwrap();
                if version >= 18 {
                    c.execute("UPDATE smtp_submissions SET sent_floor=2 WHERE account_id=?1 AND operation_id=?2",params![account,op]).unwrap();
                }
            }
            if let Some(disposition) = disposition {
                let decision = match disposition {
                    "abandoned" => "abandon",
                    "resend_requested" => "resend",
                    _ => "confirm_applied",
                };
                let details = match disposition {
                    "abandoned" => {
                        json!({"decision":"abandon","reason":"Historical explicit abandonment"})
                    }
                    "resend_requested" => {
                        json!({"decision":"resend","request_id":REQUEST,"accept_duplicate_risk":true,"reason":"Historical explicit replacement"})
                    }
                    _ => {
                        json!({"decision":"confirm_applied","evidence":{"provider_id":format!("accepted-{index}"),"message_id":message_id,"etag":null,"observed_at_ms":8000,"note":"Historical independent confirmation","smtp_acceptance_note":if account==IMAP{Some("Historical independent SMTP acceptance")}else{None}}})
                    }
                };
                let evidence = json!({"account_id":account,"operation_id":op,"expected_version":8,"decision":details});
                c.execute(
                    "INSERT INTO operation_resolutions VALUES(?1,?2,1,9000,?3,?4,?5)",
                    params![
                        account,
                        op,
                        decision,
                        evidence.to_string(),
                        (decision == "resend").then_some(OPERATION)
                    ],
                )
                .unwrap();
            }
        }
        if version >= 19 {
            c.execute(
                "INSERT INTO restored_operations VALUES(?1,?2,?3,'queued',1,500)",
                params![account, id(4), "11".repeat(32)],
            )
            .unwrap();
        }
    }
    c.close().unwrap();
}
fn value<'a>(row: &'a [Value], table: &Table, column: &str) -> &'a Value {
    &row[table.columns.iter().position(|c| c == column).unwrap()]
}
fn replace(row: &mut [Value], table: &Table, column: &str, new: Value) {
    row[table.columns.iter().position(|c| c == column).unwrap()] = new;
}
fn text(value: &Value) -> &str {
    rusqlite::types::ValueRef::from(value).as_str().unwrap()
}
fn compare(c: &Connection, before: &Snapshot, restored: bool) {
    for (name, table) in before {
        if restored
            && matches!(
                name.as_str(),
                "store_meta" | "change_log" | "restored_operations"
            )
        {
            continue;
        }
        let mut expected = table.rows.clone();
        if restored {
            for row in &mut expected {
                if name == "operations"
                    && pending(
                        text(value(row, table, "state")),
                        match value(row, table, "disposition") {
                            Value::Null => None,
                            v => Some(text(v)),
                        },
                    )
                {
                    let old_version = match value(row, table, "version") {
                        Value::Integer(v) => *v,
                        _ => 0,
                    };
                    let updated = match value(row, table, "updated_at_ms") {
                        Value::Integer(v) => *v,
                        _ => 0,
                    };
                    for (column, new) in [
                        ("state", Value::Text("uncertain".into())),
                        ("version", Value::Integer(old_version + 1)),
                        ("needs_reconciliation", Value::Integer(0)),
                        ("next_attempt_at_ms", Value::Null),
                        ("updated_at_ms", Value::Integer(updated.max(NOW))),
                        (
                            "error_code",
                            Value::Text("restored_snapshot_unreconciled".into()),
                        ),
                    ] {
                        replace(row, table, column, new);
                    }
                }
                if name == "operation_attempts"
                    && *value(row, table, "finished_at_ms") == Value::Null
                {
                    let started = match value(row, table, "started_at_ms") {
                        Value::Integer(v) => *v,
                        _ => 0,
                    };
                    replace(
                        row,
                        table,
                        "finished_at_ms",
                        Value::Integer(started.max(NOW)),
                    );
                    replace(row, table, "outcome", Value::Text("uncertain".into()));
                    replace(
                        row,
                        table,
                        "error_code",
                        Value::Text("restored_snapshot_interrupted".into()),
                    );
                }
            }
        }
        assert_eq!(
            rows(c, name, &table.columns),
            expected,
            "historical {name} differs, restored={restored}"
        );
    }
}
async fn public_history(store: &Store, restored: bool) {
    for account in [ACCOUNT, IMAP] {
        for (index, &(state, disposition)) in CASES.iter().enumerate() {
            let op = store
                .get_operation(account.into(), id(index))
                .await
                .unwrap();
            let held = restored && pending(state, disposition);
            assert_eq!(op.state, if held { "uncertain" } else { state });
            assert_eq!(op.version, if held { 10 } else { 9 });
            assert_eq!(op.disposition.as_deref(), disposition);
            assert_eq!(op.request_id, request(index));
            assert_eq!(op.resolutions.len(), usize::from(disposition.is_some()));
            let attempts = store
                .operation_attempts(account.into(), id(index))
                .await
                .unwrap();
            assert_eq!(attempts.len(), usize::from(state != "cancelled"));
            if state == "running" {
                assert_eq!(attempts[0].finished_at_ms, restored.then_some(6000));
                assert_eq!(
                    attempts[0].error_code.as_deref(),
                    restored.then_some("restored_snapshot_interrupted")
                );
            }
            if matches!(state, "running" | "applied" | "uncertain") {
                assert_eq!(
                    attempts[0].receipts.len(),
                    if account == IMAP && state == "applied" {
                        2
                    } else {
                        1
                    }
                );
                assert_eq!(
                    attempts[0].receipts[0].provider_id.as_deref(),
                    Some(format!("accepted-{index}").as_str())
                );
            }
            let payload = store.send_payload(account.into(), id(index)).await.unwrap();
            let wire = store
                .blob_chunk(account.into(), payload.wire.id, 0)
                .await
                .unwrap();
            assert_eq!(
                wire,
                std::str::from_utf8(WIRE)
                    .unwrap()
                    .replace(
                        "Message-ID: <legacy@example.test>",
                        &format!("Message-ID: <historical-{index}@example.test>")
                    )
                    .into_bytes()
            );
            if held {
                for kind in [AttemptKind::Dispatch, AttemptKind::Reconcile] {
                    assert!(matches!(
                        store
                            .begin_operation_attempt(account.into(), id(index), kind, 12000)
                            .await,
                        Err(StoreError::VersionConflict)
                    ));
                }
            }
        }
    }
}
#[tokio::test]
async fn all_operation_schema_generations_preserve_attempts_receipts_and_decisions_during_migration(
) {
    for version in 10..=history().last_schema {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.db");
        enrich(&path, version);
        let c = opened(&path);
        let before = snapshot(&c);
        c.close().unwrap();
        let store = Store::open(temp.path(), Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap();
        public_history(&store, false).await;
        store.close().await.unwrap();
        let c = opened(&path);
        compare(&c, &before, false);
        c.close().unwrap();
    }
}
#[tokio::test]
async fn historical_restores_preserve_positive_evidence_and_resolutions_while_holding_all_pending_states(
) {
    const PASSPHRASE: &str = "Synthetic rich historical backup phrase";
    for version in 10..=history().last_schema {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("history.nuncio");
        enrich(&source, version);
        let c = opened(&source);
        let before = snapshot(&c);
        c.execute_batch("PRAGMA application_id=0x4e554e42; CREATE TABLE nuncio_backup_metadata(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format_version INTEGER NOT NULL,schema_version INTEGER NOT NULL,created_at_ms INTEGER NOT NULL,revision INTEGER NOT NULL);").unwrap();
        c.execute(
            "INSERT INTO nuncio_backup_metadata VALUES(1,1,?1,11000,41)",
            [version],
        )
        .unwrap();
        c.pragma_update(None, "rekey", PASSPHRASE).unwrap();
        c.close().unwrap();
        let original = std::fs::read(&source).unwrap();
        let stage = nuncio_engine::store::stage_restore(
            &source,
            Zeroizing::new(PASSPHRASE.into()),
            Zeroizing::new(KEY.to_vec()),
            temp.path(),
            NOW,
        )
        .unwrap();
        let target = temp.path().join("restored");
        let report = stage.activate(&target).unwrap();
        assert_eq!(report.revision, 42);
        assert_eq!(report.held_operations, 10);
        assert_eq!(report.backup.operations, 22);
        assert!(std::fs::read(&source).unwrap() == original);
        let store = Store::open(&target, Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap();
        public_history(&store, true).await;
        assert_eq!(store.recover_operations(12000).await.unwrap(), 0);
        store.close().await.unwrap();
        let c = opened(&target.join("store.db"));
        compare(&c, &before, true);
        assert_eq!(
            c.query_row("SELECT count(*) FROM restored_operations", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            10 + if version >= 19 { 2 } else { 0 }
        );
        if version >= 19 {
            let table = &before["restored_operations"];
            let retained = rows(&c, "restored_operations", &table.columns)
                .into_iter()
                .filter(|row| text(value(row, table, "backup_sha256")) == "11".repeat(32))
                .collect::<Vec<_>>();
            assert_eq!(retained, table.rows);
        }
        for account in [ACCOUNT, IMAP] {
            let provenance=c.prepare("SELECT source_state,source_version FROM restored_operations WHERE account_id=?1 AND backup_sha256=?2 ORDER BY source_state").unwrap().query_map(params![account,report.backup.sha256],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
            assert_eq!(
                provenance,
                vec![
                    ("conflict".into(), 9),
                    ("queued".into(), 1),
                    ("retry_wait".into(), 9),
                    ("running".into(), 9),
                    ("uncertain".into(), 9)
                ]
            );
            c.execute(
                "UPDATE accounts SET state='connected' WHERE id=?1",
                [account],
            )
            .unwrap();
        }
        c.close().unwrap();
        let store = Store::open(&target, Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap();
        assert!(store.ready_operations(20000).await.unwrap().is_empty());
        assert_eq!(store.recover_operations(20000).await.unwrap(), 0);
        public_history(&store, true).await;
        store.close().await.unwrap();
    }
}
