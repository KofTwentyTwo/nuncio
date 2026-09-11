#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::mail::decode_mime,
    store::{AccountRecord, MailQuery, StagedMail, Store, StoreError},
};
use zeroize::Zeroizing;

fn message(id: &str, subject: &str, labels: &[&str]) -> StagedMail {
    let raw=format!("From: sender@example.test\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nsearchable original {id}\r\n").into_bytes();
    let decoded = decode_mime(&raw, 64 * 1024 * 1024).unwrap();
    StagedMail {
        provider_id: id.into(),
        thread_id: Some("thread-one".into()),
        history_id: Some("9007199254740993".into()),
        internal_date_ms: Some(1_700_000_000_123),
        provider_json: "{}".into(),
        subject: Some(subject.into()),
        headers: vec![],
        labels: labels.iter().map(|s| s.to_string()).collect(),
        raw: Some(raw),
        decoded: Some(decoded),
        availability: "available".into(),
    }
}
fn query(account: &str) -> MailQuery {
    MailQuery {
        account_id: account.into(),
        collection_id: None,
        query: None,
        page_size: 1,
        page_token: None,
    }
}
async fn begin(store: &Store, account: &str) -> String {
    let run = store
        .start_mail_run(account.into(), "full".into(), 1)
        .await
        .unwrap();
    store
        .begin_sync_run(
            account.into(),
            run.id.clone(),
            Some("9007199254740993".into()),
        )
        .await
        .unwrap();
    run.id
}
#[tokio::test]
async fn incomplete_projection_is_invisible_and_promotion_preserves_identity_and_scoped_search() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x39; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    for id in [&a, &b] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "synthetic@example.test".into(),
            })
            .await
            .unwrap();
    }
    let run = begin(&store, &a).await;
    store
        .stage_mail(
            a.clone(),
            run.clone(),
            message("one", "First", &["INBOX", "STARRED"]),
        )
        .await
        .unwrap();
    store
        .stage_mail(a.clone(), run.clone(), message("two", "Second", &["INBOX"]))
        .await
        .unwrap();
    assert!(store.query_mail(query(&a)).await.unwrap().items.is_empty());
    assert!(store
        .promote_mail(b.clone(), run.clone(), Some("9007199254741007".into()), 2)
        .await
        .is_err());
    store
        .promote_mail(a.clone(), run.clone(), Some("9007199254741007".into()), 2)
        .await
        .unwrap();
    assert_eq!(
        store.sync_run(a.clone(), run).await.unwrap().state,
        "succeeded"
    );
    let first = store.query_mail(query(&a)).await.unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.coverage.state, "current");
    assert_eq!(first.coverage.cursor.as_deref(), Some("9007199254741007"));
    let mut second_query = query(&a);
    second_query.page_token = first.next_page_token.clone();
    assert!(second_query.page_token.is_some());
    let second = store.query_mail(second_query.clone()).await.unwrap();
    assert_ne!(first.items[0].id, second.items[0].id);
    assert!(second.next_page_token.is_none());
    let original = [&first.items[0], &second.items[0]]
        .into_iter()
        .find(|m| m.provider_id == "one")
        .unwrap();
    assert_eq!(original.collections.len(), 2);
    assert!(store
        .get_mail(b.clone(), original.id.clone())
        .await
        .is_err());
    let mut search = query(&a);
    search.query = Some("searchable".into());
    search.page_size = 100;
    assert_eq!(
        store.query_mail(search.clone()).await.unwrap().items.len(),
        2
    );
    search.account_id = b.clone();
    assert!(store.query_mail(search).await.unwrap().items.is_empty());
    let failed = begin(&store, &a).await;
    store
        .stage_mail(
            a.clone(),
            failed.clone(),
            message("one", "Should stay hidden", &[]),
        )
        .await
        .unwrap();
    store
        .finish_sync_run_error(
            a.clone(),
            failed.clone(),
            "failed".into(),
            "provider_unavailable".into(),
            3,
        )
        .await
        .unwrap();
    assert!(store
        .promote_mail(a.clone(), failed, Some("later".into()), 4)
        .await
        .is_err());
    assert_eq!(
        store
            .get_mail(a.clone(), original.id.clone())
            .await
            .unwrap()
            .message
            .subject
            .as_deref(),
        Some("First")
    );
    let replacement = begin(&store, &a).await;
    store
        .stage_mail(
            a.clone(),
            replacement.clone(),
            message("one", "Updated", &["ARCHIVE"]),
        )
        .await
        .unwrap();
    store
        .promote_mail(a.clone(), replacement, Some("9007199254741021".into()), 5)
        .await
        .unwrap();
    assert!(matches!(
        store.query_mail(second_query).await,
        Err(StoreError::RefreshRequired)
    ));
    let updated = store.query_mail(query(&a)).await.unwrap();
    assert_eq!(updated.items.len(), 1);
    assert_eq!(updated.items[0].id, original.id);
    assert_eq!(updated.items[0].subject.as_deref(), Some("Updated"));
    assert_eq!(updated.items[0].collections.len(), 1);
    let raw = store
        .mail_blob(a.clone(), original.id.clone(), "raw".into(), None)
        .await
        .unwrap();
    let downloaded = store.blob_chunk(a.clone(), raw.id, 0).await.unwrap();
    assert!(String::from_utf8(downloaded)
        .unwrap()
        .contains("Subject: Updated"));
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(
        store.query_mail(query(&a)).await.unwrap().items[0].id,
        original.id
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn full_repair_rebuilds_all_account_search_rows_only_on_success() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x39; 32]))
        .await
        .unwrap();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    for account in [&a, &b] {
        store
            .add_account(AccountRecord {
                id: account.clone(),
                provider: "google".into(),
                address: "synthetic@example.test".into(),
            })
            .await
            .unwrap();
        let run = begin(&store, account).await;
        store
            .stage_mail(
                account.clone(),
                run.clone(),
                message("one", "Original", &["INBOX"]),
            )
            .await
            .unwrap();
        store
            .promote_mail(account.clone(), run, Some("10".into()), 2)
            .await
            .unwrap();
    }
    let original = store.query_mail(query(&a)).await.unwrap().items[0].clone();
    let connection = rusqlite::Connection::open(temp.path().join("store.db")).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode([0x39; 32])))
        .unwrap();
    for account in [&a, &b] {
        connection.execute("INSERT INTO message_search(account_id,message_id,subject,body) VALUES (?1,'orphan','Stale','orphaned')", [account]).unwrap();
    }
    connection.execute("INSERT INTO message_search(account_id,message_id,subject,body) VALUES (?1,?2,'Duplicate','stale')", rusqlite::params![a,original.id]).unwrap();
    let count = |account: &str| {
        connection
            .query_row(
                "SELECT count(*) FROM message_search WHERE account_id=?1",
                [account],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    assert_eq!(count(&a), 3);
    assert_eq!(count(&b), 2);
    let repair = begin(&store, &a).await;
    store
        .stage_mail(
            a.clone(),
            repair.clone(),
            message("one", "Repaired", &["INBOX"]),
        )
        .await
        .unwrap();
    assert_eq!(count(&a), 3);
    connection.execute_batch("CREATE TRIGGER reject_repair BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT,'synthetic repair storage failure'); END;").unwrap();
    assert!(store
        .promote_mail(a.clone(), repair.clone(), Some("20".into()), 3)
        .await
        .is_err());
    assert_eq!(count(&a), 3);
    assert_eq!(
        store
            .get_mail(a.clone(), original.id.clone())
            .await
            .unwrap()
            .message
            .subject,
        original.subject
    );
    connection
        .execute_batch("DROP TRIGGER reject_repair")
        .unwrap();
    store
        .promote_mail(a.clone(), repair, Some("20".into()), 4)
        .await
        .unwrap();
    assert_eq!(
        count(&a),
        1,
        "a full repair must discard orphan FTS rows too"
    );
    assert_eq!(
        count(&b),
        2,
        "another account's projection must remain untouched"
    );
    let mut search = query(&a);
    search.query = Some("Repaired".into());
    let result = store.query_mail(search).await.unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id, original.id);
    drop(connection);
    store.close().await.unwrap();
}
