#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{AccountRecord, Store};
use zeroize::Zeroizing;

#[tokio::test]
async fn encrypted_mail_schema_enforces_account_scope_and_indexes_text() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x61; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let alpha = uuid::Uuid::new_v4().to_string();
    let beta = uuid::Uuid::new_v4().to_string();
    for id in [&alpha, &beta] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "schema@example.test".into(),
            })
            .await
            .unwrap();
    }
    store.close().await.unwrap();
    let connection = rusqlite::Connection::open(temp.path().join("store.db")).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode(&key)))
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let message = uuid::Uuid::new_v4().to_string();
    let collection = uuid::Uuid::new_v4().to_string();
    connection.execute("INSERT INTO messages(account_id,id,provider_id,body_availability) VALUES (?1,?2,'m001','missing')",rusqlite::params![alpha,message]).unwrap();
    connection
        .execute(
            "INSERT INTO collections(account_id,id,provider_id) VALUES (?1,?2,'INBOX')",
            rusqlite::params![beta, collection],
        )
        .unwrap();
    assert!(connection
        .execute(
            "INSERT INTO memberships(account_id,message_id,collection_id) VALUES (?1,?2,?3)",
            rusqlite::params![alpha, message, collection]
        )
        .is_err());
    assert!(connection
        .execute(
            "INSERT INTO memberships(account_id,message_id,collection_id) VALUES (?1,?2,?3)",
            rusqlite::params![beta, message, collection]
        )
        .is_err());
    connection.execute("INSERT INTO message_search(account_id,message_id,subject,body) VALUES (?1,?2,'encrypted title','search needle')",rusqlite::params![alpha,message]).unwrap();
    let found: i64 = connection
        .query_row(
            "SELECT count(*) FROM message_search WHERE message_search MATCH ?1 AND account_id=?2",
            rusqlite::params!["needle", alpha],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(found, 1);
    let other: i64 = connection
        .query_row(
            "SELECT count(*) FROM message_search WHERE message_search MATCH ?1 AND account_id=?2",
            rusqlite::params!["needle", beta],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(other, 0);
    connection.close().unwrap();
    let bytes = std::fs::read(temp.path().join("store.db")).unwrap();
    assert!(!bytes
        .windows(b"search needle".len())
        .any(|w| w == b"search needle"));
    Store::open(temp.path(), key)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}
