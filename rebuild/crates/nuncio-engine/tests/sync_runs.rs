#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{AccountRecord, Store};
use zeroize::Zeroizing;

#[tokio::test]
async fn sync_intent_and_progress_survive_restart_and_cannot_cross_accounts() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x63; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    for id in [&a, &b] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "sync@example.test".into(),
            })
            .await
            .unwrap();
    }
    let run = store
        .start_mail_run(a.clone(), "full".into(), 1_700_000_000_000)
        .await
        .unwrap();
    assert_eq!(run.state, "queued");
    let queued = store.changes_after(2, 100).await.unwrap();
    assert_eq!(queued.changes.len(), 1);
    assert_eq!(
        queued.changes[0].resource_id.as_deref(),
        Some(run.id.as_str())
    );
    assert!(store
        .start_mail_run(a.clone(), "full".into(), 1_700_000_000_001)
        .await
        .is_err());
    assert!(store.sync_run(b, run.id.clone()).await.is_err());
    assert!(
        store
            .begin_sync_run(a.clone(), run.id.clone(), None)
            .await
            .is_err(),
        "a full scan requires its captured history cursor before running"
    );
    store
        .begin_sync_run(a.clone(), run.id.clone(), Some("9007199254740993".into()))
        .await
        .unwrap();
    store
        .sync_run_progress(a.clone(), run.id.clone(), 2, Some("opaque-page".into()))
        .await
        .unwrap();
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let status = store.sync_run(a.clone(), run.id.clone()).await.unwrap();
    assert_eq!(status.state, "running");
    assert_eq!(status.processed, 2);
    let transitions = store.changes_after(2, 100).await.unwrap();
    assert_eq!(transitions.changes.len(), 3);
    assert!(transitions
        .changes
        .iter()
        .all(|c| c.kind == "sync_run" && c.resource_id.as_deref() == Some(run.id.as_str())));
    store
        .finish_sync_run_error(
            a.clone(),
            run.id.clone(),
            "cancelled".into(),
            "cancelled".into(),
            1_700_000_000_002,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .sync_run(a.clone(), run.id.clone())
            .await
            .unwrap()
            .state,
        "cancelled"
    );
    assert!(store
        .sync_run_progress(a.clone(), run.id, 3, None)
        .await
        .is_err());
    assert!(store
        .start_mail_run(a.clone(), "full".into(), 1_700_000_000_003)
        .await
        .is_ok());
    store.recover_sync_runs(1_700_000_000_004).await.unwrap();
    let schedules = store.sync_schedules().await.unwrap();
    let recovered = schedules
        .iter()
        .find(|s| s.account_id == a && s.scope == "gmail")
        .unwrap();
    assert_eq!(recovered.error_code.as_deref(), Some("interrupted"));
    assert!(recovered.next_attempt_at_ms >= 1_700_000_001_004);
    assert_eq!(recovered.run_state.as_deref(), Some("failed"));
    store.close().await.unwrap();
}
