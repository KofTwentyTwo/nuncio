#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{AccountRecord, Store};
use rusqlite::{params, Connection};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[tokio::test]
async fn abandoned_large_sync_cleanup_preserves_published_mail_and_completes_within_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let key = [0x54; 32];
    let store = Store::open(directory.path(), Zeroizing::new(key.to_vec()))
        .await
        .unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    for id in [&account, &other] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "imap".into(),
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
        .begin_sync_run(account.clone(), run.id.clone(), None)
        .await
        .unwrap();
    store
        .sync_run_progress(account.clone(), run.id.clone(), 66_524, None)
        .await
        .unwrap();
    store.close().await.unwrap();

    // Seed a large abandoned generation in one transaction to measure actual
    // encrypted cleanup separately from network latency and fixture fsyncs.
    let mut connection = Connection::open(directory.path().join("store.db")).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode(key)))
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let tx = connection.transaction().unwrap();
    tx.execute(
        "INSERT INTO blobs VALUES(?1,'blob',?2,4)",
        params![
            account,
            "230d8358dc8e8890b4c58deeb62912ee2f20357ae92a5cc861b98e68fe31acb5"
        ],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO blob_chunks VALUES(?1,'blob',0,?2)",
        params![account, b"body".as_slice()],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO staged_collections VALUES(?1,?2,'inbox','INBOX','system')",
        params![account, run.id],
    )
    .unwrap();
    for i in 0..66_524 {
        let provider = format!("synthetic-{i}");
        tx.execute("INSERT INTO staged_messages(account_id,run_id,provider_id,subject,search_text,body_availability,raw_blob_id) VALUES(?1,?2,?3,'Abandoned','synthetic searchable content','available','blob')",params![account,run.id,provider]).unwrap();
        for (ordinal, name) in [(0, "From"), (1, "To"), (2, "Subject")] {
            tx.execute(
                "INSERT INTO staged_headers VALUES(?1,?2,?3,?4,?5,'synthetic')",
                params![account, run.id, provider, ordinal, name],
            )
            .unwrap();
        }
        tx.execute(
            "INSERT INTO staged_memberships VALUES(?1,?2,?3,'inbox')",
            params![account, run.id, provider],
        )
        .unwrap();
        tx.execute("INSERT INTO staged_attachments VALUES(?1,?2,?3,0,'synthetic.txt','text/plain',NULL,'blob')",params![account,run.id,provider]).unwrap();
    }
    for id in [&account, &other] {
        tx.execute("INSERT INTO messages(account_id,id,provider_id,subject,body_availability) VALUES(?1,'published','published','Preserved','missing')",[id]).unwrap();
    }
    tx.commit().unwrap();
    drop(connection);

    let store = Store::open(directory.path(), Zeroizing::new(key.to_vec()))
        .await
        .unwrap();
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(30), store.recover_sync_runs(2))
        .await
        .unwrap()
        .unwrap();
    eprintln!(
        "encrypted_sync_recovery_entries=66524 elapsed_ms={}",
        started.elapsed().as_millis()
    );
    let recovered = store.sync_run(account.clone(), run.id).await.unwrap();
    assert_eq!(recovered.state, "failed");
    assert_eq!(recovered.error_code.as_deref(), Some("interrupted"));
    assert_eq!(recovered.processed, 66_524);
    store.close().await.unwrap();
    let connection = Connection::open(directory.path().join("store.db")).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode(key)))
        .unwrap();
    for table in [
        "staged_messages",
        "staged_headers",
        "staged_memberships",
        "staged_attachments",
        "staged_collections",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            count, 0,
            "all abandoned staged children must be removed: {table}"
        );
    }
    for id in [&account, &other] {
        let subject: String = connection
            .query_row(
                "SELECT subject FROM messages WHERE account_id=?1 AND id='published'",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(subject, "Preserved");
    }
    assert!(connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query([])
        .unwrap()
        .next()
        .unwrap()
        .is_none());
}
