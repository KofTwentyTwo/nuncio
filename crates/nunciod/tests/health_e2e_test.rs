//! Offline E2E for `System.GetHealth`: real gRPC over loopback, real store,
//! mock keyring. No network beyond 127.0.0.1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::GetHealthRequest;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use std::sync::Arc;
use tokio::net::TcpListener;

#[tokio::test]
async fn authenticated_get_health_reports_seeded_queue_depth() {
    let secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );
    let (db, _dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let db = Arc::new(db);

    let account = nuncio_core::AccountConfig {
        id: "acct-e2e".to_string(),
        name: "E2E".to_string(),
        email_address: "e2e@nuncio.mx".to_string(),
        keyring_secret_key: "nuncio/acct-e2e".to_string(),
        sync_interval_secs: 60,
        transport: nuncio_core::Transport::Jmap(nuncio_core::JmapTransport {
            endpoint_host: "jmap.nuncio.mx".to_string(),
        }),
    };
    db.save_account(&account).await.expect("save account");

    db.save_pending_mutation(&nuncio_filter::PendingRemoteMutation {
        id: "mut-e2e".to_string(),
        account_id: "acct-e2e".to_string(),
        rule_id: "rule-1".to_string(),
        message_id: "msg-1".to_string(),
        mutation_type: "MOVE".to_string(),
        payload: "{}".to_string(),
        status: "pending".to_string(),
        retry_count: 0,
        created_at: 0,
    })
    .await
    .expect("save mutation");

    let event_bus = Arc::new(EventBus::new());
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    tokio::spawn(nunciod::grpc::serve_on_listener(
        listener,
        event_bus,
        db,
        filter_engine,
        secrets,
        token.clone(),
    ));

    let mut client = nuncio_proto::client::connect_system(&addr.to_string(), &token)
        .await
        .expect("connect");

    let health = client
        .get_health(GetHealthRequest {})
        .await
        .expect("get_health")
        .into_inner();

    let queue = health
        .account_queues
        .iter()
        .find(|q| q.account_id == "acct-e2e")
        .expect("seeded account must be reported");
    assert_eq!(queue.pending, 1);
    assert_eq!(queue.failed, 0);
}

#[tokio::test]
async fn unauthenticated_get_health_is_rejected() {
    let secrets = Arc::new(SecretManager::mock());
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("token provisioned from mock vault"),
    );
    let (db, _dir) = DatabaseEngine::connect_ephemeral()
        .await
        .expect("connect ephemeral test db");
    let event_bus = Arc::new(EventBus::new());
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    tokio::spawn(nunciod::grpc::serve_on_listener(
        listener,
        event_bus,
        Arc::new(db),
        filter_engine,
        secrets,
        token,
    ));

    let mut client =
        nuncio_proto::v1::system_client::SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("connect");

    let status = client
        .get_health(GetHealthRequest {})
        .await
        .expect_err("must reject unauthenticated GetHealth");

    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}
