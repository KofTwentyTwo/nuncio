#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::drafts::DraftContent,
    store::{AccountRecord, DraftUploadInput, SaveDraft, Store, StoreError},
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

fn request(account: &str, draft: &str, data: &[u8]) -> DraftUploadInput {
    DraftUploadInput {
        account_id: account.into(),
        draft_id: draft.into(),
        expected_version: 1,
        filename: Some("evidence.bin".into()),
        mime_type: "application/octet-stream".into(),
        parameters: Default::default(),
        content_id: None,
        disposition: "attachment".into(),
        byte_length: data.len() as u64,
        sha256: format!("{:x}", Sha256::digest(data)),
    }
}
#[tokio::test]
async fn upload_staging_is_bounded_atomic_hash_checked_and_cleaned_on_drop_or_restart() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x75; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    for id in [&account, &other] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "upload@example.test".into(),
            })
            .await
            .unwrap();
    }
    let draft = store
        .save_draft(
            SaveDraft {
                account_id: account.clone(),
                id: None,
                expected_version: None,
                content: DraftContent::default(),
            },
            100,
        )
        .await
        .unwrap();
    let bytes = (0..600_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    assert!(matches!(
        store
            .begin_draft_upload(request(&other, &draft.id, &bytes), 101)
            .await,
        Err(StoreError::NotFound)
    ));
    let upload = store
        .begin_draft_upload(request(&account, &draft.id, &bytes), 101)
        .await
        .unwrap();
    assert!(store
        .append_draft_upload(&upload, 0, vec![0; 262145])
        .await
        .is_err());
    assert!(store
        .append_draft_upload(&upload, 1, bytes[..262144].to_vec())
        .await
        .is_err());
    store
        .append_draft_upload(&upload, 0, bytes[..262144].to_vec())
        .await
        .unwrap();
    assert!(store
        .get_draft(account.clone(), draft.id.clone())
        .await
        .unwrap()
        .attachments
        .is_empty());
    assert!(
        store.finish_draft_upload(upload, 102).await.is_err(),
        "incomplete upload must not publish"
    );
    let mut wrong = request(&account, &draft.id, &bytes);
    wrong.sha256 = "0".repeat(64);
    let upload = store.begin_draft_upload(wrong, 102).await.unwrap();
    for (i, chunk) in bytes.chunks(262144).enumerate() {
        store
            .append_draft_upload(&upload, (i * 262144) as u64, chunk.to_vec())
            .await
            .unwrap();
    }
    assert!(
        store.finish_draft_upload(upload, 103).await.is_err(),
        "wrong digest must not publish"
    );
    let unchanged = store
        .get_draft(account.clone(), draft.id.clone())
        .await
        .unwrap();
    assert_eq!(unchanged.version, 1);
    assert!(unchanged.attachments.is_empty());
    let mut pending = Vec::new();
    for _ in 0..4 {
        pending.push(
            store
                .begin_draft_upload(request(&account, &draft.id, &bytes), 104)
                .await
                .unwrap(),
        );
    }
    assert!(
        store
            .begin_draft_upload(request(&account, &draft.id, &bytes), 104)
            .await
            .is_err(),
        "concurrent staging must be bounded"
    );
    drop(pending.pop());
    let replacement = store
        .begin_draft_upload(request(&account, &draft.id, &bytes), 104)
        .await
        .unwrap();
    drop(replacement);
    drop(pending);
    let upload = store
        .begin_draft_upload(request(&account, &draft.id, &bytes), 105)
        .await
        .unwrap();
    for (i, chunk) in bytes.chunks(262144).enumerate() {
        store
            .append_draft_upload(&upload, (i * 262144) as u64, chunk.to_vec())
            .await
            .unwrap();
    }
    let attached = store.finish_draft_upload(upload, 106).await.unwrap();
    assert_eq!(attached.version, 2);
    assert_eq!(attached.attachments.len(), 1);
    let attachment = &attached.attachments[0];
    assert_eq!(attachment.sha256, format!("{:x}", Sha256::digest(&bytes)));
    let mut original = Vec::new();
    for i in 0..3 {
        original.extend(
            store
                .blob_chunk(account.clone(), attachment.blob_id.clone(), i)
                .await
                .unwrap(),
        );
    }
    assert_eq!(original, bytes);
    let mut stale = request(&account, &draft.id, &[]);
    stale.expected_version = 2;
    let raced = store.begin_draft_upload(stale, 107).await.unwrap();
    let edited = store
        .save_draft(
            SaveDraft {
                account_id: account.clone(),
                id: Some(draft.id.clone()),
                expected_version: Some(2),
                content: DraftContent {
                    subject: "Concurrent edit after upload began".into(),
                    ..Default::default()
                },
            },
            107,
        )
        .await
        .unwrap();
    assert_eq!(edited.version, 3);
    assert!(
        matches!(
            store.finish_draft_upload(raced, 108).await,
            Err(StoreError::VersionConflict)
        ),
        "version must be checked again at publication, after a successful begin"
    );
    assert_eq!(
        store
            .get_draft(account.clone(), draft.id.clone())
            .await
            .unwrap()
            .version,
        3
    );
    let mut stale = request(&account, &draft.id, &[]);
    stale.expected_version = 3;
    let unfinished = store.begin_draft_upload(stale, 107).await.unwrap();
    store.close().await.unwrap();
    drop(unfinished);
    let store = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(store.recover_draft_uploads().await.unwrap(), 1);
    assert_eq!(
        store
            .get_draft(account.clone(), draft.id)
            .await
            .unwrap()
            .attachments[0]
            .sha256,
        attachment.sha256
    );
    store.close().await.unwrap();
}
