#![allow(clippy::unwrap_used)]

use nuncio_engine::store::{AccountRecord, Store};
use rusqlite::{params, Connection};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[tokio::test]
async fn large_promotion_preserves_other_account_search_and_status_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let key = [0x53; 32];
    let store = Store::open(directory.path(), Zeroizing::new(key.to_vec()))
        .await
        .unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    for id in [&account, &other] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "synthetic@example.test".into(),
            })
            .await
            .unwrap();
    }
    let run = store
        .start_mail_run(account.clone(), "full".into(), 1)
        .await
        .unwrap();
    store
        .begin_sync_run(account.clone(), run.id.clone(), Some("10".into()))
        .await
        .unwrap();

    // Seed an encrypted, valid staged generation without timing provider requests
    // or thousands of fixture fsyncs. Promotion still uses the real store worker.
    let mut connection = Connection::open(directory.path().join("store.db")).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode(key)))
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let transaction = connection.transaction().unwrap();
    for index in 0..10_000 {
        let provider = format!("resource-{index:05}");
        transaction.execute(
            "INSERT INTO messages(account_id,id,provider_id,subject,body_availability) VALUES(?1,?2,?2,'Unchanged','missing')",
            params![other, provider],
        ).unwrap();
        transaction.execute(
            "INSERT INTO message_search(account_id,message_id,subject,body) VALUES(?1,?2,'Unchanged','preserved other account')",
            params![other, provider],
        ).unwrap();
        transaction.execute(
            "INSERT INTO staged_messages(account_id,run_id,provider_id,subject,search_text,body_availability) VALUES(?1,?2,?3,'Promoted','new account body','missing')",
            params![account, run.id, provider],
        ).unwrap();
    }
    transaction.commit().unwrap();
    drop(connection);

    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let (promotion, status) = tokio::join!(
            biased;
            store.promote_mail(account.clone(), run.id, Some("20".into()), 2),
            store.status()
        );
        promotion.unwrap();
        assert_eq!(status.unwrap().account_count, 2);
    })
    .await;
    eprintln!(
        "encrypted_promotion_and_queued_status_ms={}",
        started.elapsed().as_millis()
    );
    assert!(
        result.is_ok(),
        "promotion must not starve a status request past its production 30-second deadline"
    );
    store.close().await.unwrap();

    let connection = Connection::open(directory.path().join("store.db")).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode(key)))
        .unwrap();
    for (id, subject, body) in [
        (&account, "Promoted", "new account body"),
        (&other, "Unchanged", "preserved other account"),
    ] {
        let count: i64 = connection.query_row(
            "SELECT count(*) FROM message_search s JOIN messages m ON m.account_id=s.account_id AND m.id=s.message_id WHERE s.account_id=?1 AND s.subject=?2 AND s.body=?3",
            params![id, subject, body], |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 10_000);
        let all: i64 = connection
            .query_row(
                "SELECT count(*) FROM message_search WHERE account_id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(all, 10_000, "no duplicate or orphan search rows");
    }
}
