#![allow(clippy::unwrap_used, clippy::expect_used)]
use nuncio_engine::{
    engine::{Engine, EngineConfig},
    secrets::{SecretError, SecretStore},
};
use nuncio_proto::v2::{system_client::SystemClient, GetStatusRequest, ShutdownRequest};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Default)]
struct MemorySecrets(Mutex<BTreeMap<String, Zeroizing<Vec<u8>>>>);
impl SecretStore for MemorySecrets {
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

#[tokio::test]
async fn real_server_authenticates_status_and_shutdown_and_releases_profile() {
    let directory = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let engine = Engine::open(EngineConfig {
        directory: directory.path().into(),
        secrets: secrets.clone(),
    })
    .await
    .unwrap();
    let auth = engine.authorization();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(nunciod::serve(engine, listener));
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        SystemClient::connect(format!("http://{address}")),
    )
    .await
    .unwrap();
    assert!(result.is_ok(), "daemon must serve its bound listener");
    let mut client = result.unwrap();
    assert_eq!(
        client
            .get_status(GetStatusRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    assert_eq!(
        client
            .shutdown(ShutdownRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let mut request = tonic::Request::new(GetStatusRequest {});
    request
        .metadata_mut()
        .insert("authorization", auth.parse().unwrap());
    let status = client.get_status(request).await.unwrap().into_inner();
    assert_eq!(status.api_version, "nuncio.v2");
    assert_eq!(status.storage.unwrap().account_count, 0);
    let mut request = tonic::Request::new(ShutdownRequest {});
    request
        .metadata_mut()
        .insert("authorization", auth.parse().unwrap());
    client.shutdown(request).await.unwrap();
    drop(client);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    Engine::open(EngineConfig {
        directory: directory.path().into(),
        secrets,
    })
    .await
    .unwrap()
    .shutdown()
    .await
    .unwrap();
}
