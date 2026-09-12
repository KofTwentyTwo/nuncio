#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{AccountRecord, Store};
use zeroize::Zeroizing;
#[tokio::test]
async fn calendar_objects_and_occurrences_cannot_reference_another_accounts_calendar() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x67; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    for id in [&a, &b] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "calendar-schema@example.test".into(),
            })
            .await
            .unwrap();
    }
    store.close().await.unwrap();
    let c = rusqlite::Connection::open(temp.path().join("store.db")).unwrap();
    c.pragma_update(None, "key", format!("x'{}'", hex::encode(&key)))
        .unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    let cal = uuid::Uuid::new_v4().to_string();
    c.execute("INSERT INTO calendars(account_id,id,provider_id,access_role,provider_json) VALUES (?1,?2,'primary','owner','{}')",rusqlite::params![a,cal]).unwrap();
    assert!(c.execute("INSERT INTO calendar_event_ids(account_id,calendar_id,provider_id,id) VALUES (?1,?2,'event-one','local-one')",rusqlite::params![b,cal]).is_err());
    c.execute("INSERT INTO calendar_event_ids(account_id,calendar_id,provider_id,id) VALUES (?1,?2,'event-one','local-one')",rusqlite::params![a,cal]).unwrap();
    let body = r#"{"canary":"encrypted-calendar-content"}"#;
    c.execute("INSERT INTO calendar_objects(account_id,calendar_id,provider_id,object_json,status) VALUES (?1,?2,'event-one',?3,'confirmed')",rusqlite::params![a,cal,body]).unwrap();
    assert!(c.execute("INSERT INTO calendar_occurrences(account_id,calendar_id,provider_id,object_json,status) VALUES (?1,?2,'event-one',?3,'confirmed')",rusqlite::params![b,cal,body]).is_err());
    c.execute("INSERT INTO calendar_occurrences(account_id,calendar_id,provider_id,object_json,status) VALUES (?1,?2,'event-one',?3,'confirmed')",rusqlite::params![a,cal,body]).unwrap();
    c.close().unwrap();
    assert!(!std::fs::read(temp.path().join("store.db"))
        .unwrap()
        .windows(26)
        .any(|w| w == b"encrypted-calendar-content"));
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(store.status().await.unwrap().schema_version, 23);
    store.close().await.unwrap();
}
