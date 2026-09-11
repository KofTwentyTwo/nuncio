#![allow(clippy::unwrap_used)]
#[allow(dead_code)]
#[path = "support/historical_store.rs"]
mod historical;
use historical::*;
use nuncio_engine::store::{
    AttemptKind, AttemptOutcome, ReconcileOperation, ReconciliationMode, Store, StoreError,
};
use zeroize::Zeroizing;

async fn restored() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("snapshot.nuncio");
    backup_fixture(&source, 20, "synthetic reconciliation backup");
    let stage = nuncio_engine::store::stage_restore(
        &source,
        Zeroizing::new("synthetic reconciliation backup".into()),
        Zeroizing::new(KEY.to_vec()),
        temp.path(),
        2000,
    )
    .unwrap();
    let target = temp.path().join("restored");
    stage.activate(&target).unwrap();
    let store = Store::open(&target, Zeroizing::new(KEY.to_vec()))
        .await
        .unwrap();
    (temp, store)
}
fn request(account: &str, mode: ReconciliationMode) -> ReconcileOperation {
    ReconcileOperation {
        account_id: account.into(),
        operation_id: OPERATION.into(),
        request_id: "d2b3cd06-2451-47ce-b3f7-f9e9921858a9".into(),
        expected_version: 2,
        mode,
    }
}

#[tokio::test]
async fn explicit_reconciliation_is_scoped_idempotent_durable_and_cannot_replay_a_restored_smtp_send(
) {
    for mode in [ReconciliationMode::Observe, ReconciliationMode::ResumeSafe] {
        let (temp, store) = restored().await;
        let input = request(IMAP, mode);
        let before = store.status().await.unwrap().revision;
        let accepted = store
            .request_reconciliation(input.clone(), 3000)
            .await
            .unwrap();
        assert_eq!(accepted.version, 3);
        assert!(accepted.needs_reconciliation);
        let receipt = accepted.reconciliation.unwrap();
        assert_eq!(receipt.request_id, input.request_id);
        assert_eq!(receipt.expected_version, 2);
        assert_eq!(receipt.mode, mode);
        assert_eq!(receipt.first_attempt_ordinal, None);
        assert!(receipt.active);
        assert_eq!(store.status().await.unwrap().revision, before + 1);
        let replay = store
            .request_reconciliation(input.clone(), 5000)
            .await
            .unwrap();
        assert_eq!(replay.version, 3);
        assert_eq!(store.status().await.unwrap().revision, before + 1);
        let mut changed = input.clone();
        changed.mode = if mode == ReconciliationMode::Observe {
            ReconciliationMode::ResumeSafe
        } else {
            ReconciliationMode::Observe
        };
        assert!(matches!(
            store.request_reconciliation(changed, 3000).await,
            Err(StoreError::VersionConflict)
        ));
        store
            .request_reconciliation(request(ACCOUNT, mode), 3000)
            .await
            .unwrap();
        let attempt = store
            .begin_operation_attempt(IMAP.into(), OPERATION.into(), AttemptKind::Reconcile, 3000)
            .await
            .unwrap();
        assert_eq!(attempt.ordinal, 1);
        assert_eq!(
            store
                .get_operation(IMAP.into(), OPERATION.into())
                .await
                .unwrap()
                .reconciliation
                .unwrap()
                .first_attempt_ordinal,
            Some(1)
        );
        store.close().await.unwrap();
        let store = Store::open(&temp.path().join("restored"), Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap();
        assert_eq!(store.recover_operations(4000).await.unwrap(), 1);
        let attempt = store
            .begin_operation_attempt(IMAP.into(), OPERATION.into(), AttemptKind::Reconcile, 4000)
            .await
            .unwrap();
        let held = store
            .finish_operation_attempt(
                IMAP.into(),
                OPERATION.into(),
                attempt.ordinal,
                AttemptOutcome::RetryUnstartedSmtp { retry_at_ms: 4000 },
                4000,
            )
            .await
            .unwrap();
        assert_eq!(held.state, "uncertain");
        assert_eq!(
            held.error_code.as_deref(),
            Some("restored_smtp_acceptance_unknown")
        );
        assert!(!held.needs_reconciliation);
        assert!(held.next_attempt_at_ms.is_none());
        assert!(matches!(
            store
                .begin_operation_attempt(IMAP.into(), OPERATION.into(), AttemptKind::Dispatch, 5000)
                .await,
            Err(StoreError::VersionConflict)
        ));
        assert_eq!(
            store
                .request_reconciliation(input, 5000)
                .await
                .unwrap()
                .version,
            held.version
        );
        assert_eq!(
            store
                .operation_attempts(IMAP.into(), OPERATION.into())
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store
                .send_payload(IMAP.into(), OPERATION.into())
                .await
                .unwrap()
                .message_id,
            "<legacy@example.test>"
        );
        store.close().await.unwrap();
    }
}

#[tokio::test]
async fn restoring_an_admitted_request_preserves_its_audit_without_reenabling_it() {
    let (temp, store) = restored().await;
    let input = request(ACCOUNT, ReconciliationMode::ResumeSafe);
    store
        .request_reconciliation(input.clone(), 3000)
        .await
        .unwrap();
    let backup = temp.path().join("second.nuncio");
    let artifact = store
        .create_backup(
            Zeroizing::new("synthetic second reconciliation backup".into()),
            3000,
        )
        .await
        .unwrap();
    std::fs::copy(artifact.path(), &backup).unwrap();
    store.close().await.unwrap();
    let stage = nuncio_engine::store::stage_restore(
        &backup,
        Zeroizing::new("synthetic second reconciliation backup".into()),
        Zeroizing::new(KEY.to_vec()),
        temp.path(),
        4000,
    )
    .unwrap();
    let target = temp.path().join("second");
    stage.activate(&target).unwrap();
    let store = Store::open(&target, Zeroizing::new(KEY.to_vec()))
        .await
        .unwrap();
    let held = store
        .get_operation(ACCOUNT.into(), OPERATION.into())
        .await
        .unwrap();
    assert!(!held.needs_reconciliation);
    assert!(!held.reconciliation.unwrap().active);
    let replay = store
        .request_reconciliation(input.clone(), 5000)
        .await
        .unwrap();
    assert!(!replay.needs_reconciliation);
    assert_eq!(store.recover_operations(5000).await.unwrap(), 0);
    let mut fresh = input;
    fresh.request_id = "f41543e4-bfc1-4fb8-a760-c2c0fd39c567".into();
    fresh.expected_version = replay.version;
    let admitted = store.request_reconciliation(fresh, 5000).await.unwrap();
    assert!(admitted.needs_reconciliation);
    assert!(admitted.reconciliation.unwrap().active);
    store.close().await.unwrap();
}

#[tokio::test]
async fn invalid_reconciliation_admission_and_exhausted_history_leave_all_state_unchanged() {
    let (_temp, store) = restored().await;
    let before = store.status().await.unwrap().revision;
    for (request_id, account, operation, version, error) in [
        ("invalid", IMAP, OPERATION, 2, "invalid"),
        (
            "0f49e8df-68b2-4f44-adff-22f2a97e1160",
            "",
            OPERATION,
            2,
            "invalid",
        ),
        (
            "0f49e8df-68b2-4f44-adff-22f2a97e1160",
            IMAP,
            OPERATION,
            0,
            "invalid",
        ),
        (
            "0f49e8df-68b2-4f44-adff-22f2a97e1160",
            IMAP,
            OPERATION,
            i64::MAX as u64,
            "invalid",
        ),
        (
            "0f49e8df-68b2-4f44-adff-22f2a97e1160",
            IMAP,
            OPERATION,
            1,
            "version",
        ),
        (
            "0f49e8df-68b2-4f44-adff-22f2a97e1160",
            IMAP,
            "missing",
            2,
            "missing",
        ),
    ] {
        let failure = store
            .request_reconciliation(
                ReconcileOperation {
                    account_id: account.into(),
                    operation_id: operation.into(),
                    request_id: request_id.into(),
                    expected_version: version,
                    mode: ReconciliationMode::Observe,
                },
                3000,
            )
            .await;
        assert!(matches!(
            (failure, error),
            (Err(StoreError::InvalidInput), "invalid")
                | (Err(StoreError::VersionConflict), "version")
                | (Err(StoreError::NotFound), "missing")
        ));
        assert_eq!(store.status().await.unwrap().revision, before);
    }
    store.close().await.unwrap();
    for requests in [true, false] {
        let (temp, store) = restored().await;
        store.close().await.unwrap();
        let path = temp.path().join("restored/store.db");
        let mut c = opened(&path);
        let tx = c.transaction().unwrap();
        for ordinal in 1..=1000 {
            if requests {
                let request_id = format!("00000000-0000-4000-8000-{ordinal:012}");
                tx.execute("INSERT INTO operation_reconciliation_requests(account_id,request_id,operation_id,expected_version,mode,requested_at_ms) VALUES(?1,?2,?3,?4,'observe',2000)",rusqlite::params![IMAP,request_id,OPERATION,ordinal]).unwrap();
            } else {
                tx.execute("INSERT INTO operation_attempts(account_id,operation_id,ordinal,kind,started_at_ms,finished_at_ms,outcome) VALUES(?1,?2,?3,'reconcile',2000,2000,'uncertain')",rusqlite::params![IMAP,OPERATION,ordinal]).unwrap();
            }
        }
        tx.execute(
            "UPDATE operations SET version=1001 WHERE account_id=?1 AND id=?2",
            rusqlite::params![IMAP, OPERATION],
        )
        .unwrap();
        tx.commit().unwrap();
        let before = snapshot(&c);
        c.close().unwrap();
        let store = Store::open(&temp.path().join("restored"), Zeroizing::new(KEY.to_vec()))
            .await
            .unwrap();
        let mut input = request(IMAP, ReconciliationMode::Observe);
        input.expected_version = 1001;
        assert!(
            matches!(
                store.request_reconciliation(input, 3000).await,
                Err(StoreError::ResultTooLarge)
            ),
            "exhausted request={requests} history must fail before admission"
        );
        store.close().await.unwrap();
        let c = opened(&path);
        for (name, table) in before {
            assert_eq!(
                rows(&c, &name, &table.columns),
                table.rows,
                "limit refusal altered {name}"
            );
        }
        c.close().unwrap();
    }
}
