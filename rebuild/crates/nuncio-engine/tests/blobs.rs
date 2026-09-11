#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::mail::BLOB_CHUNK_BYTES,
    store::{AccountRecord, Store},
};
use zeroize::Zeroizing;

#[tokio::test]
async fn blobs_are_immutable_chunked_account_scoped_and_durable() {
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x62; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    for id in [&a, &b] {
        store
            .add_account(AccountRecord {
                id: id.clone(),
                provider: "google".into(),
                address: "blob@example.test".into(),
            })
            .await
            .unwrap();
    }
    let raw: Vec<u8> = (0..BLOB_CHUNK_BYTES * 2 + 13)
        .map(|i| (i % 251) as u8)
        .collect();
    let first = store.put_blob(a.clone(), raw.clone()).await.unwrap();
    assert_eq!(first.byte_length, raw.len() as u64);
    assert_eq!(
        store.put_blob(a.clone(), raw.clone()).await.unwrap().id,
        first.id
    );
    let other = store.put_blob(b.clone(), raw.clone()).await.unwrap();
    assert_ne!(other.id, first.id);
    assert!(store
        .blob_chunk(b.clone(), first.id.clone(), 0)
        .await
        .is_err());
    store.close().await.unwrap();
    let store = Store::open(temp.path(), key).await.unwrap();
    let mut received = Vec::new();
    for ordinal in 0..3 {
        let chunk = store
            .blob_chunk(a.clone(), first.id.clone(), ordinal)
            .await
            .unwrap();
        assert!(chunk.len() <= BLOB_CHUNK_BYTES);
        received.extend_from_slice(&chunk);
    }
    assert_eq!(received, raw);
    assert!(store
        .blob_chunk(a.clone(), first.id.clone(), 3)
        .await
        .is_err());
    let empty = store.put_blob(a.clone(), Vec::new()).await.unwrap();
    assert_eq!(empty.byte_length, 0);
    assert!(store.blob_chunk(a, empty.id, 0).await.is_err());
    store.close().await.unwrap();
}
