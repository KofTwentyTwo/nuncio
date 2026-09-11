#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    engine::{Engine, EngineConfig},
    secrets::{SecretError, SecretStore},
};
use nuncio_proto::{
    client::TokenInjector,
    v2::{self, maintenance_client::MaintenanceClient, system_client::SystemClient},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Default)]
struct Secrets(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);
impl SecretStore for Secrets {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        self.0
            .lock()
            .unwrap()
            .insert(name.into(), Zeroizing::new(value.to_vec()));
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}
fn secret() -> v2::RecoverySecret {
    v2::RecoverySecret {
        passphrase: "synthetic authenticated maintenance phrase".into(),
    }
}
fn upload(
    bytes: &[u8],
    name: Option<&str>,
) -> impl futures_util::Stream<Item = v2::BackupUploadChunk> + Send + 'static {
    futures_util::stream::iter(frames(bytes, name))
}
fn frames(bytes: &[u8], name: Option<&str>) -> Vec<v2::BackupUploadChunk> {
    let mut parts = vec![v2::BackupUploadChunk {
        content: Some(v2::backup_upload_chunk::Content::Header(
            v2::BackupUploadHeader {
                secret: Some(secret()),
                byte_length: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
                new_profile: name.map(str::to_owned),
            },
        )),
    }];
    for (i, part) in bytes.chunks(65537).enumerate() {
        parts.push(v2::BackupUploadChunk {
            content: Some(v2::backup_upload_chunk::Content::Data(v2::BackupData {
                offset: (i * 65537) as u64,
                data: part.to_vec(),
            })),
        });
    }
    parts
}

#[tokio::test]
async fn authenticated_maintenance_streams_verified_ciphertext_and_restores_a_new_profile() {
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(Secrets::default());
    let directory = temp.path().join("source");
    let engine = Engine::open(EngineConfig {
        directory: directory.clone(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let auth = engine.authorization();
    let id = engine.status().await.unwrap().profile_id;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(nunciod::serve(engine, listener));
    let channel = nuncio_proto::client::connect(&format!("http://{address}"))
        .await
        .unwrap();
    let mut anonymous = MaintenanceClient::new(channel.clone());
    assert_eq!(
        anonymous
            .create_backup(v2::CreateBackupRequest {
                secret: Some(secret())
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .inspect_backup(upload(b"invalid", None))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        anonymous
            .restore_backup(upload(b"invalid", Some("denied")))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let injector = TokenInjector::new(auth).unwrap();
    let mut client = MaintenanceClient::with_interceptor(channel.clone(), injector.clone());
    let mut stream = client
        .create_backup(v2::CreateBackupRequest {
            secret: Some(secret()),
        })
        .await
        .unwrap()
        .into_inner();
    let Some(v2::backup_chunk::Content::Info(info)) =
        stream.message().await.unwrap().unwrap().content
    else {
        unreachable!()
    };
    assert_eq!(info.schema_version, 22);
    assert_eq!(info.accounts, 0);
    let mut bytes = Vec::new();
    let mut complete = false;
    while let Some(frame) = stream.message().await.unwrap() {
        assert!(!complete);
        match frame.content.unwrap() {
            v2::backup_chunk::Content::Data(data) => {
                assert_eq!(data.offset, bytes.len() as u64);
                assert!(!data.data.is_empty());
                assert!(data.data.len() <= 262144);
                bytes.extend(data.data);
            }
            v2::backup_chunk::Content::Complete(digest) => {
                assert_eq!(digest.byte_length, bytes.len() as u64);
                assert_eq!(digest.sha256, format!("{:x}", Sha256::digest(&bytes)));
                complete = true;
            }
            _ => unreachable!(),
        }
    }
    assert!(complete);
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert_eq!(info.byte_length, bytes.len() as u64);
    assert_eq!(info.sha256, format!("{:x}", Sha256::digest(&bytes)));
    let inspected = client
        .inspect_backup(upload(&bytes, None))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(inspected, info);
    for case in [
        "empty-stream",
        "missing-header",
        "repeated-header",
        "empty-data",
        "offset",
        "oversized-chunk",
        "short",
        "hash",
        "wrong-key",
    ] {
        use v2::backup_upload_chunk::Content;
        let mut parts = frames(&bytes, None);
        match case {
            "empty-stream" => parts.clear(),
            "missing-header" => {
                parts.remove(0);
            }
            "repeated-header" => parts.insert(1, parts[0].clone()),
            "empty-data" => {
                parts[1].content = Some(Content::Data(v2::BackupData {
                    offset: 0,
                    data: Vec::new(),
                }))
            }
            "offset" => {
                let Some(Content::Data(data)) = &mut parts[1].content else {
                    unreachable!()
                };
                data.offset = 1;
            }
            "oversized-chunk" => {
                parts[1].content = Some(Content::Data(v2::BackupData {
                    offset: 0,
                    data: vec![0; 262145],
                }))
            }
            "short" => {
                parts.pop();
            }
            _ => {
                let Some(Content::Header(header)) = &mut parts[0].content else {
                    unreachable!()
                };
                if case == "hash" {
                    header.sha256 = "0".repeat(64);
                } else {
                    header.secret = Some(v2::RecoverySecret {
                        passphrase: "incorrect synthetic recovery key".into(),
                    });
                }
            }
        }
        let failure = client
            .inspect_backup(futures_util::stream::iter(parts))
            .await
            .unwrap_err();
        assert_eq!(
            failure.code(),
            tonic::Code::InvalidArgument,
            "wrong failure for {case}"
        );
        assert!(
            !std::fs::read_dir(&directory).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".maintenance-")),
            "leaked upload after {case}"
        );
    }
    assert_eq!(
        client
            .inspect_backup(upload(&bytes, None))
            .await
            .unwrap()
            .into_inner(),
        info
    );
    assert_eq!(
        client
            .inspect_backup(upload(&bytes, Some("unexpected")))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(
        client
            .restore_backup(upload(&bytes, None))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    let result = client
        .restore_backup(upload(&bytes, Some("restored")))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        result.directory,
        std::fs::canonicalize(temp.path())
            .unwrap()
            .join("restored")
            .to_str()
            .unwrap()
    );
    assert_ne!(result.profile_id, id);
    assert_eq!(result.backup, Some(info));
    assert_eq!(result.held_operations, 0);
    let restored = Engine::open(EngineConfig {
        directory: temp.path().join("restored"),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    assert_eq!(
        restored.status().await.unwrap().profile_id,
        result.profile_id
    );
    restored.shutdown().await.unwrap();
    assert_eq!(
        client
            .restore_backup(upload(&bytes, Some("restored")))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::AlreadyExists
    );
    let mut system = SystemClient::with_interceptor(channel, injector);
    system.shutdown(v2::ShutdownRequest {}).await.unwrap();
    drop(client);
    drop(anonymous);
    drop(system);
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    Engine::open(EngineConfig { directory, secrets })
        .await
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}
