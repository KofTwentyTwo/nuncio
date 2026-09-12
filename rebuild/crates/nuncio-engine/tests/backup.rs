#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        drafts::{DraftContent, Recipient},
        prepare::PreparedAttachment,
        submission::{freeze, SubmissionTransport},
    },
    store::{
        inspect_backup, AccountRecord, AttemptKind, AttemptOutcome, BackupInspection,
        DraftUploadInput, EnqueueSend, SaveDraft, Store, StoreError,
    },
};
use rusqlite::{types::Value, Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
use zeroize::Zeroizing;

fn key() -> Zeroizing<String> {
    Zeroizing::new("synthetic recovery passphrase for backup tests".into())
}
fn opened(path: &Path, key: &str) -> Connection {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.pragma_update(None, "key", key).unwrap();
    c
}
fn contents(c: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
    let mut tables=c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name!='nuncio_backup_metadata' ORDER BY name").unwrap();
    let names = tables
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut result = BTreeMap::new();
    for name in names {
        let mut statement = c
            .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
            .unwrap();
        let n = statement.column_count();
        let mut rows = statement
            .query_map([], |r| {
                (0..n).map(|i| r.get(i)).collect::<Result<Vec<Value>, _>>()
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows.sort_by_cached_key(|r| format!("{r:?}"));
        result.insert(name, rows);
    }
    result
}
#[tokio::test]
async fn encrypted_snapshot_preserves_committed_wal_tables_and_frozen_operations_under_an_independent_key(
) {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::open(temporary.path(), Zeroizing::new(vec![0x41; 32]))
        .await
        .unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    store
        .add_account(AccountRecord {
            id: account.clone(),
            provider: "google".into(),
            address: "alpha@example.test".into(),
        })
        .await
        .unwrap();
    let content = DraftContent {
        to: vec![Recipient {
            address: "beta@example.test".into(),
            name: None,
        }],
        subject: "Encrypted backup canary".into(),
        text: Some("Durable draft and frozen body".into()),
        ..Default::default()
    };
    let draft = store
        .save_draft(
            SaveDraft {
                account_id: account.clone(),
                id: None,
                expected_version: None,
                content: content.clone(),
            },
            1000,
        )
        .await
        .unwrap();
    let binary = (0..320_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let upload = store
        .begin_draft_upload(
            DraftUploadInput {
                account_id: account.clone(),
                draft_id: draft.id.clone(),
                expected_version: 1,
                filename: Some("durable.bin".into()),
                mime_type: "application/octet-stream".into(),
                parameters: Default::default(),
                content_id: None,
                disposition: "attachment".into(),
                byte_length: binary.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&binary)),
            },
            1000,
        )
        .await
        .unwrap();
    for (i, chunk) in binary.chunks(262144).enumerate() {
        store
            .append_draft_upload(&upload, (i * 262144) as u64, chunk.to_vec())
            .await
            .unwrap();
    }
    let draft = store.finish_draft_upload(upload, 1000).await.unwrap();
    let attachments = [PreparedAttachment {
        filename: Some("durable.bin".into()),
        mime_type: "application/octet-stream".into(),
        parameters: Default::default(),
        content_id: None,
        disposition: "attachment".into(),
        bytes: binary,
    }];
    let queued = store
        .enqueue_send(
            EnqueueSend {
                account_id: account.clone(),
                request_id: uuid::Uuid::new_v4().to_string(),
                draft_id: draft.id,
                snapshot_version: 2,
                expected_version: Some(2),
                sender: "alpha@example.test".into(),
                frozen: freeze(
                    "alpha@example.test",
                    &content,
                    None,
                    &attachments,
                    "backup@nuncio.invalid",
                    1001,
                    SubmissionTransport::Gmail,
                )
                .unwrap(),
            },
            1001,
        )
        .await
        .unwrap();
    store
        .begin_operation_attempt(
            account.clone(),
            queued.id.clone(),
            AttemptKind::Dispatch,
            1002,
        )
        .await
        .unwrap();
    store
        .finish_operation_attempt(
            account.clone(),
            queued.id,
            1,
            AttemptOutcome::Uncertain {
                code: "lost_acceptance".into(),
            },
            1003,
        )
        .await
        .unwrap();
    assert!(
        temporary
            .path()
            .join("store.db-wal")
            .metadata()
            .unwrap()
            .len()
            > 32
    );
    let source = opened(
        &temporary.path().join("store.db"),
        &format!("x'{}'", hex::encode([0x41; 32])),
    );
    let before = contents(&source);
    let before_status = store.status().await.unwrap();
    let backup = store.create_backup(key(), 2000).await.unwrap();
    let info = backup.inspection();
    assert_eq!(info.format_version, 1);
    assert_eq!(info.schema_version, 23);
    assert_eq!(info.created_at_ms, 2000);
    assert_eq!(info.revision, before_status.revision);
    assert_eq!(info.accounts, 1);
    assert_eq!(info.drafts, 1);
    assert_eq!(info.operations, 1);
    let bytes = std::fs::read(backup.path()).unwrap();
    assert_eq!(info.byte_length, bytes.len() as u64);
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert!(!bytes
        .windows(b"Encrypted backup canary".len())
        .any(|w| w == b"Encrypted backup canary"));
    assert_eq!(inspect_backup(backup.path(), key()).unwrap(), *info);
    let copied = opened(backup.path(), &key());
    let after = contents(&copied);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    for (name, rows) in &before {
        assert!(rows == &after[name], "backup differs in table {name}");
    }
    assert_eq!(contents(&source), before);
    assert_eq!(
        store.status().await.unwrap().revision,
        before_status.revision
    );
    store
        .save_draft(
            SaveDraft {
                account_id: account,
                id: None,
                expected_version: None,
                content,
            },
            3000,
        )
        .await
        .unwrap();
    assert_eq!(inspect_backup(backup.path(), key()).unwrap().drafts, 1);
    assert!(matches!(
        inspect_backup(
            backup.path(),
            Zeroizing::new("a different synthetic passphrase".into())
        ),
        Err(StoreError::KeyOrCorrupt)
    ));
    assert!(
        opened(backup.path(), &format!("x'{}'", hex::encode([0x41; 32])))
            .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r
                .get::<_, i64>(0))
            .is_err()
    );
    drop(copied);
    drop(source);
    let path = backup.path().to_owned();
    drop(backup);
    assert!(!path.exists());
    store.close().await.unwrap();
}

#[tokio::test]
async fn backup_inspection_rejects_wrong_format_corruption_truncation_and_future_schema_without_changes(
) {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::open(temporary.path(), Zeroizing::new(vec![0x42; 32]))
        .await
        .unwrap();
    for phrase in ["", "            ", "too short", "contains nul\0phrase"] {
        assert!(matches!(
            store.create_backup(Zeroizing::new(phrase.into()), 1).await,
            Err(StoreError::InvalidInput)
        ));
    }
    for prefix in ["x", "X"] {
        for n in [64, 96, 160] {
            let phrase = Zeroizing::new(format!("{prefix}'{}'", "1".repeat(n)));
            assert!(
                matches!(
                    store.create_backup(phrase, 1).await,
                    Err(StoreError::InvalidInput)
                ),
                "raw keys must not bypass passphrase derivation"
            );
        }
    }
    let backup = store.create_backup(key(), 10).await.unwrap();
    let original = std::fs::read(backup.path()).unwrap();
    for name in ["corrupt", "truncated", "future", "wrong-format"] {
        let file = temporary.path().join(name);
        std::fs::write(&file, &original).unwrap();
        match name {
            "corrupt" => {
                let mut bytes = original.clone();
                let n = bytes.len();
                bytes[n - 30] ^= 1;
                std::fs::write(&file, bytes).unwrap();
            }
            "truncated" => {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&file)
                    .unwrap()
                    .set_len(128)
                    .unwrap();
            }
            _ => {
                let c = Connection::open(&file).unwrap();
                c.pragma_update(None, "key", key().as_str()).unwrap();
                c.pragma_update(
                    None,
                    if name == "future" {
                        "user_version"
                    } else {
                        "application_id"
                    },
                    if name == "future" { 999 } else { 0 },
                )
                .unwrap();
            }
        }
        let before = std::fs::read(&file).unwrap();
        let result: Result<BackupInspection, StoreError> = inspect_backup(&file, key());
        assert!(result.is_err(), "accepted {name}");
        if name == "future" {
            assert!(matches!(result, Err(StoreError::FutureSchema)));
        }
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert_eq!(std::fs::read(backup.path()).unwrap(), original);
    }
    assert_eq!(store.status().await.unwrap().schema_version, 23);
    store.close().await.unwrap();
}
