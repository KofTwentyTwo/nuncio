#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::drafts::DraftContent,
    store::{AccountRecord, ProjectionScope, SaveDraft, Store, StoreError},
};
use zeroize::Zeroizing;

fn originals(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    ["store.db", "store.db-wal"]
        .into_iter()
        .filter(|name| path.join(name).exists())
        .map(|name| (name.into(), std::fs::read(path.join(name)).unwrap()))
        .collect()
}

#[tokio::test]
async fn repair_integrity_preflight_rejects_relational_corruption_without_rewriting_any_original() {
    let temporary = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x4b; 32]);
    let store = Store::open(temporary.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    store
        .add_account(AccountRecord {
            id: account.clone(),
            provider: "google".into(),
            address: "synthetic@example.test".into(),
        })
        .await
        .unwrap();
    let draft = store
        .save_draft(
            SaveDraft {
                account_id: account.clone(),
                id: None,
                expected_version: None,
                content: DraftContent {
                    subject: "Preserve through failed repair".into(),
                    text: Some("Durable local body".into()),
                    ..Default::default()
                },
            },
            1000,
        )
        .await
        .unwrap();
    let connection = rusqlite::Connection::open(temporary.path().join("store.db")).unwrap();
    connection
        .pragma_update(None, "key", format!("x'{}'", hex::encode(key.as_slice())))
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    connection.execute("INSERT INTO message_headers(account_id,message_id,ordinal,name,value) VALUES (?1,'absent-message',0,'Subject','Corrupt reference')",[&account]).unwrap();
    let revision = store.status().await.unwrap().revision;
    let before = originals(temporary.path());
    for scope in [ProjectionScope::Mail, ProjectionScope::Calendar] {
        assert!(matches!(
            store.preview_repair(account.clone(), scope).await,
            Err(StoreError::KeyOrCorrupt)
        ));
        assert_eq!(store.status().await.unwrap().revision, revision);
        assert!(
            originals(temporary.path()) == before,
            "integrity rejection must leave original database/WAL bytes intact"
        );
        let retained = store
            .get_draft(account.clone(), draft.id.clone())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(retained).unwrap(),
            serde_json::to_value(&draft).unwrap()
        );
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM message_headers WHERE message_id='absent-message'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM sync_runs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(connection);
    store.close().await.unwrap();
}
