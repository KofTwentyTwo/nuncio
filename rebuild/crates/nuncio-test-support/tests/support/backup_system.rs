use crate::support::system::SystemHarness;
use nuncio_proto::v2::*;
use sha2::{Digest, Sha256};

fn secret() -> RecoverySecret {
    RecoverySecret {
        passphrase: "synthetic reconciliation system backup".into(),
    }
}
pub(crate) async fn backup(h: &SystemHarness) -> Vec<u8> {
    let mut stream = h
        .maintenance()
        .create_backup(CreateBackupRequest {
            secret: Some(secret()),
        })
        .await
        .unwrap()
        .into_inner();
    let mut bytes = Vec::new();
    let mut info = false;
    let mut complete = false;
    while let Some(chunk) = stream.message().await.unwrap() {
        assert!(!complete);
        match chunk.content.unwrap() {
            backup_chunk::Content::Info(_) => {
                assert!(!info && bytes.is_empty());
                info = true;
            }
            backup_chunk::Content::Data(data) => {
                assert!(info);
                assert_eq!(data.offset, bytes.len() as u64);
                bytes.extend(data.data);
                assert!(bytes.len() < 16 * 1024 * 1024);
            }
            backup_chunk::Content::Complete(digest) => {
                assert_eq!(digest.byte_length, bytes.len() as u64);
                assert_eq!(digest.sha256, format!("{:x}", Sha256::digest(&bytes)));
                complete = true;
            }
        }
    }
    assert!(info && complete);
    bytes
}
pub(crate) fn upload(
    bytes: &[u8],
) -> impl futures_util::Stream<Item = BackupUploadChunk> + Send + 'static {
    let mut frames = vec![BackupUploadChunk {
        content: Some(backup_upload_chunk::Content::Header(BackupUploadHeader {
            secret: Some(secret()),
            byte_length: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            new_profile: Some("restored".into()),
        })),
    }];
    for (i, data) in bytes.chunks(65536).enumerate() {
        frames.push(BackupUploadChunk {
            content: Some(backup_upload_chunk::Content::Data(BackupData {
                offset: (i * 65536) as u64,
                data: data.to_vec(),
            })),
        });
    }
    futures_util::stream::iter(frames)
}
