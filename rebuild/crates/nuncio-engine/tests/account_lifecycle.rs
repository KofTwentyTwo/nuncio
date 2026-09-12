#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{AccountLifecycle, ConnectedAccount, Store, StoreError};
use zeroize::Zeroizing;

#[tokio::test]
async fn backup_restore_preserves_names_pause_archive_and_saved_drafts() {
    use nuncio_engine::store::{stage_restore, SaveDraft};
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("source"), Zeroizing::new(vec![0x65; 32]))
        .await
        .unwrap();
    let paused = uuid::Uuid::new_v4().to_string();
    let archived = uuid::Uuid::new_v4().to_string();
    connect(&store, &paused, "alpha").await;
    connect(&store, &archived, "beta").await;
    store
        .edit_account_name(archived.clone(), 1, "Retained archive".into())
        .await
        .unwrap();
    let draft = store
        .save_draft(
            SaveDraft {
                account_id: archived.clone(),
                id: None,
                expected_version: None,
                content: Default::default(),
            },
            1,
        )
        .await
        .unwrap();
    store
        .change_account_lifecycle(paused.clone(), AccountLifecycle::Pause)
        .await
        .unwrap();
    let saved = store
        .change_account_lifecycle(archived.clone(), AccountLifecycle::Archive)
        .await
        .unwrap();
    let phrase = Zeroizing::new("synthetic account recovery passphrase".to_string());
    let backup = store.create_backup(phrase.clone(), 2).await.unwrap();
    let key = Zeroizing::new(vec![0x66; 32]);
    let staged = stage_restore(backup.path(), phrase, key.clone(), temp.path(), 3).unwrap();
    let target = temp.path().join("restored");
    staged.activate(&target).unwrap();
    let restored = Store::open(&target, key).await.unwrap();
    let row = restored.account(archived.clone()).await.unwrap().unwrap();
    assert_eq!(row.display_name, saved.display_name);
    assert_eq!(row.version, saved.version);
    assert_eq!(row.state, "archived");
    assert!(row.credential_ref.is_none());
    let row = restored.account(paused).await.unwrap().unwrap();
    assert_eq!(row.state, "paused");
    assert_eq!(row.auth_state, "disconnected");
    assert!(row.credential_ref.is_none());
    assert_eq!(
        restored
            .get_draft(archived, draft.id)
            .await
            .unwrap()
            .version,
        1
    );
    restored.close().await.unwrap();
    store.close().await.unwrap();
}

#[tokio::test]
async fn failed_purge_rolls_back_deleted_content_and_change_redaction() {
    use nuncio_engine::store::SaveDraft;
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x67; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    connect(&store, &id, "alpha").await;
    let draft = store
        .save_draft(
            SaveDraft {
                account_id: id.clone(),
                id: None,
                expected_version: None,
                content: Default::default(),
            },
            1,
        )
        .await
        .unwrap();
    store
        .change_account_lifecycle(id.clone(), AccountLifecycle::Archive)
        .await
        .unwrap();
    store.close().await.unwrap();
    let c = rusqlite::Connection::open(temp.path().join("store.db")).unwrap();
    c.pragma_update(None, "key", format!("x'{}'", hex::encode(&key)))
        .unwrap();
    c.execute_batch("CREATE TRIGGER refuse_purge BEFORE DELETE ON accounts BEGIN SELECT RAISE(ABORT,'synthetic purge failure'); END;").unwrap();
    c.close().unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let before = store.changes_after(0, 1000).await.unwrap();
    let preview = store.preview_account_purge(id.clone()).await.unwrap();
    assert!(store
        .purge_account(id.clone(), preview.version, preview.revision)
        .await
        .is_err());
    assert_eq!(
        store.get_draft(id.clone(), draft.id).await.unwrap().version,
        1
    );
    assert_eq!(store.account(id).await.unwrap().unwrap().state, "archived");
    let after = store.changes_after(0, 1000).await.unwrap();
    assert_eq!(
        serde_json::to_value(after.changes).unwrap(),
        serde_json::to_value(before.changes).unwrap()
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn archived_drafts_remain_readable_and_cannot_publish_an_earlier_upload() {
    use nuncio_engine::store::{DraftUploadInput, SaveDraft};
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x64; 32]))
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    connect(&store, &id, "alpha").await;
    let input = SaveDraft {
        account_id: id.clone(),
        id: None,
        expected_version: None,
        content: Default::default(),
    };
    let draft = store.save_draft(input.clone(), 1).await.unwrap();
    let upload = store
        .begin_draft_upload(
            DraftUploadInput {
                account_id: id.clone(),
                draft_id: draft.id.clone(),
                expected_version: 1,
                filename: Some("empty.txt".into()),
                mime_type: "text/plain".into(),
                parameters: Default::default(),
                content_id: None,
                disposition: "attachment".into(),
                byte_length: 0,
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            },
            2,
        )
        .await
        .unwrap();
    store
        .change_account_lifecycle(id.clone(), AccountLifecycle::Archive)
        .await
        .unwrap();
    assert!(matches!(
        store.finish_draft_upload(upload, 3).await,
        Err(StoreError::AccountLifecycle)
    ));
    assert!(matches!(
        store.save_draft(input, 4).await,
        Err(StoreError::AccountLifecycle)
    ));
    assert!(matches!(
        store.delete_draft(id.clone(), draft.id.clone(), 1).await,
        Err(StoreError::AccountLifecycle)
    ));
    assert!(store
        .get_draft(id.clone(), draft.id)
        .await
        .unwrap()
        .attachments
        .is_empty());
    assert_eq!(store.preview_account_purge(id).await.unwrap().drafts, 1);
    store.close().await.unwrap();
}

async fn connect(store: &Store, id: &str, subject: &str) {
    let reference = format!("profile/account/{id}/secret");
    store.prepare_credential(reference.clone()).await.unwrap();
    store
        .connect_google(ConnectedAccount {
            id: id.into(),
            subject: subject.into(),
            address: format!("{subject}@example.test"),
            credential_ref: reference,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn pause_archive_restore_and_versioned_name_survive_restart() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x62; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    connect(&store, &id, "alpha").await;
    let initial = store.account(id.clone()).await.unwrap().unwrap();
    let named = store
        .edit_account_name(id.clone(), initial.version, "Personal".into())
        .await
        .unwrap();
    assert!(matches!(
        store
            .edit_account_name(id.clone(), initial.version, "Stale".into())
            .await,
        Err(StoreError::VersionConflict)
    ));
    assert!(store
        .edit_account_name(id.clone(), named.version, "bad\nname".into())
        .await
        .is_err());
    store
        .provider_retry_after(id.clone(), "gmail".into(), 9000)
        .await
        .unwrap();
    let paused = store
        .change_account_lifecycle(id.clone(), AccountLifecycle::Pause)
        .await
        .unwrap();
    assert_eq!(paused.state, "paused");
    assert_eq!(paused.auth_state, "connected");
    assert!(paused.credential_ref.is_some());
    assert!(store
        .sync_schedules()
        .await
        .unwrap()
        .iter()
        .all(|s| s.account_state == "paused"));
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(
        store.account(id.clone()).await.unwrap().unwrap().state,
        "paused"
    );
    let resumed = store
        .change_account_lifecycle(id.clone(), AccountLifecycle::Resume)
        .await
        .unwrap();
    assert_eq!(resumed.state, "connected");
    assert_eq!(
        store
            .provider_retry_deadline(id.clone(), "gmail".into())
            .await
            .unwrap(),
        Some(9000)
    );
    let archived = store
        .change_account_lifecycle(id.clone(), AccountLifecycle::Archive)
        .await
        .unwrap();
    assert_eq!(archived.state, "archived");
    assert_eq!(archived.auth_state, "disconnected");
    assert!(archived.credential_ref.is_none());
    assert_eq!(archived.display_name, "Personal");
    assert_eq!(store.credential_cleanup().await.unwrap().len(), 1);
    assert!(store
        .change_account_lifecycle(id.clone(), AccountLifecycle::Resume)
        .await
        .is_err());
    let restored = store
        .change_account_lifecycle(id.clone(), AccountLifecycle::Restore)
        .await
        .unwrap();
    assert_eq!(restored.state, "disconnected");
    assert_eq!(restored.display_name, "Personal");
    store.close().await.unwrap();
}

#[tokio::test]
async fn purge_requires_archive_fresh_preview_and_preserves_other_account_and_replay() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x63; 32]))
        .await
        .unwrap();
    let alpha = uuid::Uuid::new_v4().to_string();
    let beta = uuid::Uuid::new_v4().to_string();
    connect(&store, &alpha, "alpha").await;
    connect(&store, &beta, "beta").await;
    let preview = store.preview_account_purge(alpha.clone()).await.unwrap();
    assert!(store
        .purge_account(alpha.clone(), preview.version, preview.revision)
        .await
        .is_err());
    store
        .change_account_lifecycle(alpha.clone(), AccountLifecycle::Archive)
        .await
        .unwrap();
    let stale = store.preview_account_purge(alpha.clone()).await.unwrap();
    store
        .edit_account_name(alpha.clone(), stale.version, "Archive".into())
        .await
        .unwrap();
    assert!(matches!(
        store
            .purge_account(alpha.clone(), stale.version, stale.revision)
            .await,
        Err(StoreError::VersionConflict)
    ));
    let preview = store.preview_account_purge(alpha.clone()).await.unwrap();
    store
        .purge_account(alpha.clone(), preview.version, preview.revision)
        .await
        .unwrap();
    assert!(store.account(alpha.clone()).await.unwrap().is_none());
    assert_eq!(
        store.account(beta.clone()).await.unwrap().unwrap().state,
        "connected"
    );
    assert_eq!(
        store.credential_cleanup().await.unwrap(),
        vec![format!("profile/account/{alpha}/secret")]
    );
    let changes = store.changes_after(0, 1000).await.unwrap();
    assert_eq!(changes.changes.len() as u64, changes.current_revision);
    assert!(changes
        .changes
        .iter()
        .all(|c| c.account_id.as_deref() != Some(&alpha)));
    assert_eq!(changes.changes.last().unwrap().kind, "account_purged");
    assert!(changes
        .changes
        .iter()
        .any(|c| c.account_id.as_deref() == Some(&beta)));
    store.close().await.unwrap();
}
