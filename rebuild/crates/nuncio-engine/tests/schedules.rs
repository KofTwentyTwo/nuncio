#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{ConnectedAccount, Store};
use zeroize::Zeroizing;

#[tokio::test]
async fn provider_retry_deadline_survives_restart_and_does_not_affect_calendar() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x36; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    store.configure_poll_interval(100).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let reference = "synthetic-schedule-credential".to_owned();
    store.prepare_credential(reference.clone()).await.unwrap();
    store
        .connect_google(ConnectedAccount {
            id: account.clone(),
            subject: "100001".into(),
            address: "schedule@example.test".into(),
            credential_ref: reference,
        })
        .await
        .unwrap();
    let initial = store.sync_schedules().await.unwrap();
    assert_eq!(initial.len(), 2);
    assert!(initial
        .iter()
        .all(|s| s.next_attempt_at_ms == 0 && s.last_success_at_ms.is_none()));
    store
        .provider_retry_after(account.clone(), "gmail".into(), 10_000)
        .await
        .unwrap();
    let run = store
        .start_mail_run(account.clone(), "full".into(), 1000)
        .await
        .unwrap();
    store
        .begin_sync_run(account.clone(), run.id.clone(), Some("history".into()))
        .await
        .unwrap();
    store
        .finish_sync_run_error(
            account.clone(),
            run.id.clone(),
            "failed".into(),
            "unavailable".into(),
            1000,
        )
        .await
        .unwrap();
    let failed = store.sync_schedules().await.unwrap();
    let mail = failed.iter().find(|s| s.scope == "gmail").unwrap();
    assert!(mail.next_attempt_at_ms >= 10_000);
    assert_eq!(mail.consecutive_failures, 1);
    assert_eq!(mail.error_code.as_deref(), Some("unavailable"));
    assert_eq!(mail.last_run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(
        failed
            .iter()
            .find(|s| s.scope == "calendar")
            .unwrap()
            .next_attempt_at_ms,
        0
    );
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(
        store
            .provider_retry_deadline(account.clone(), "gmail".into())
            .await
            .unwrap(),
        Some(10_000)
    );
    let recovered = store.sync_schedules().await.unwrap();
    assert!(
        recovered
            .iter()
            .find(|s| s.scope == "gmail")
            .unwrap()
            .next_attempt_at_ms
            >= 10_000
    );
    let success = store
        .start_mail_run(account.clone(), "full".into(), 11_000)
        .await
        .unwrap();
    store
        .begin_sync_run(
            account.clone(),
            success.id.clone(),
            Some("new-history".into()),
        )
        .await
        .unwrap();
    store
        .promote_mail(
            account.clone(),
            success.id,
            Some("new-history".into()),
            11_000,
        )
        .await
        .unwrap();
    let success = store.sync_schedules().await.unwrap();
    let mail = success.iter().find(|s| s.scope == "gmail").unwrap();
    assert_eq!(mail.last_success_at_ms, Some(11_000));
    assert_eq!(mail.next_attempt_at_ms, 11_100);
    assert_eq!(mail.consecutive_failures, 0);
    assert!(mail.error_code.is_none());
    assert!(store
        .provider_retry_deadline(account, "gmail".into())
        .await
        .unwrap()
        .is_none());
    store.close().await.unwrap();
}
