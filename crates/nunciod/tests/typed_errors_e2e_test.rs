//! Offline end-to-end test that typed `ErrorInfo` details survive the gRPC
//! wire.
//!
//! Boots a real `nunciod` daemon gRPC server
//! (`nunciod::grpc::serve_on_listener_with_overrides`) on an ephemeral
//! loopback port, backed by a real temp file-backed [`DatabaseEngine`] and a
//! [`SecretManager::mock`] vault (never the real OS keyring), with every
//! engine override left at its default (no injected backends).
//!
//! Driving the authenticated `nuncio-proto` clients, it triggers several
//! known error conditions and proves a client can machine-read each failure:
//! the returned `tonic::Status` carries the correct canonical gRPC code AND a
//! prost-encoded [`nuncio_proto::v1::ErrorInfo`] in the
//! `grpc-status-details-bin` trailer whose `reason` (and metadata) decode
//! back exactly, so a client never has to string-match the English message.
//!
//! Fully offline: no real network, no real OS keyring, no live server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::errors;
use nuncio_proto::v1::{
    CalendarSyncRequest, CreateRuleRequest, ErrorReason, GetMessageRequest, RemoveAccountRequest,
};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::{
    serve_on_listener_with_overrides, AccountsEngineOverrides, CalendarEngineOverrides,
    ContactsEngineOverrides, MailEngineOverrides,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tonic::Code;

/// Boots the real daemon gRPC server on an ephemeral loopback port with every
/// override at its default. Returns the server address and minted bearer
/// token; the `TempDir` guard is returned so the caller controls its lifetime
/// for the whole test (the spawned server keeps using the db).
async fn boot_daemon() -> (std::net::SocketAddr, String, tempfile::TempDir) {
    let secrets = Arc::new(SecretManager::mock());
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join("typed_errors_e2e.db");
    let db = Arc::new(
        DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .expect("open temp file-backed database"),
    );

    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("mint gRPC bearer token from mock vault"),
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    let event_bus = Arc::new(EventBus::new());
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    let server_secrets = secrets.clone();
    let server_token = token.clone();
    tokio::spawn(async move {
        let _ = serve_on_listener_with_overrides(
            listener,
            event_bus,
            db,
            filter_engine,
            server_secrets,
            server_token,
            MailEngineOverrides::default(),
            CalendarEngineOverrides::default(),
            ContactsEngineOverrides::default(),
            AccountsEngineOverrides::default(),
        )
        .await;
    });

    (addr, token, dir)
}

#[tokio::test]
async fn typed_error_reasons_decode_from_status_details_over_the_wire() {
    let (addr, token, _db_dir) = boot_daemon().await;
    let addr_str = addr.to_string();

    // ---- account-not-found: NotFound + ACCOUNT_NOT_FOUND + account_id ----
    let mut accounts = nuncio_proto::client::connect_accounts(&addr_str, &token)
        .await
        .expect("accounts client connects");
    let status = accounts
        .remove_account(RemoveAccountRequest {
            id: "no-such-account".to_string(),
        })
        .await
        .expect_err("removing a nonexistent account must fail");
    assert_eq!(status.code(), Code::NotFound);
    let info = errors::error_info(&status).expect("typed ErrorInfo present in details trailer");
    assert_eq!(info.reason(), ErrorReason::AccountNotFound);
    assert_eq!(
        info.metadata.get("account_id").map(String::as_str),
        Some("no-such-account"),
        "the missing id is carried as machine-readable metadata"
    );
    assert_eq!(info.message, status.message());
    assert_eq!(
        errors::error_reason(&status),
        Some(ErrorReason::AccountNotFound)
    );

    // ---- message-not-found: NotFound + MESSAGE_NOT_FOUND + message_id ----
    let mut mail = nuncio_proto::client::connect_mail(&addr_str, &token)
        .await
        .expect("mail client connects");
    let status = mail
        .get_message(GetMessageRequest {
            message_id: "no-such-message".to_string(),
        })
        .await
        .expect_err("fetching a nonexistent message must fail");
    assert_eq!(status.code(), Code::NotFound);
    assert_eq!(
        errors::error_reason(&status),
        Some(ErrorReason::MessageNotFound)
    );
    let info = errors::error_info(&status).expect("typed ErrorInfo present");
    assert_eq!(
        info.metadata.get("message_id").map(String::as_str),
        Some("no-such-message")
    );

    // ---- not-configured: FailedPrecondition + NOT_CONFIGURED ----
    let mut calendar = nuncio_proto::client::connect_calendar(&addr_str, &token)
        .await
        .expect("calendar client connects");
    let status = calendar
        .sync(CalendarSyncRequest {
            account_id: String::new(),
            calendar_id: "personal".to_string(),
            start_window: Some(nuncio_proto::time::timestamp_from_unix_secs(0)),
            end_window: Some(nuncio_proto::time::timestamp_from_unix_secs(0)),
        })
        .await
        .expect_err("syncing with no CalDAV account configured must fail");
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert_eq!(
        errors::error_reason(&status),
        Some(ErrorReason::NotConfigured)
    );

    // ---- validation-failed: InvalidArgument + VALIDATION_FAILED ----
    let mut filters = nuncio_proto::client::connect_filters(&addr_str, &token)
        .await
        .expect("filters client connects");
    let status = filters
        .create_rule(CreateRuleRequest {
            name: "bad rule".to_string(),
            nsql: "this is not valid nsql".to_string(),
            priority: 0,
        })
        .await
        .expect_err("creating a rule from unparseable NSQL must fail");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(
        errors::error_reason(&status),
        Some(ErrorReason::ValidationFailed)
    );
}
