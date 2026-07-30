//! Offline end-to-end test for the account lifecycle over gRPC.
//!
//! Boots a real `nunciod` daemon gRPC server
//! (`nunciod::grpc::serve_on_listener_with_overrides`) on an ephemeral
//! loopback port, backed by a real temp file-backed [`DatabaseEngine`] and a
//! [`SecretManager::mock`] vault (never the real OS keyring), with an injected
//! stand-in [`AccountConnectionTester`] standing in for real IMAP/SMTP network
//! I/O (via the [`AccountsEngineOverrides`] injection seam).
//!
//! Driving ONLY the authenticated `nuncio-proto` Accounts client with the
//! shared bearer token -- never reaching into daemon internals -- this proves
//! the whole lifecycle end to end:
//!   a. `AddAccount` persists an account whose IMAP mode is `StartTls` and
//!      SMTP mode is `Plain`.
//!   b. `ListAccounts` reflects it AND round-trips both distinct TLS modes
//!      (the previously silently-discarded fields).
//!   c. `UpdateAccount` genuinely mutates the persisted config (confirmed via
//!      a second `ListAccounts`).
//!   d. `TestAccountConnection` reports the injected tester's GENUINE
//!      per-protocol result (IMAP ok, SMTP failed with real error detail) --
//!      never a fabricated OK.
//!   e. `RemoveAccount` genuinely deletes it (a final `ListAccounts` is
//!      empty).
//!
//! Fully offline: no real network, no real OS keyring, no live server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::{
    AccountConfig, AccountProtocol, AddAccountRequest, ListAccountsRequest, RemoveAccountRequest,
    TestAccountConnectionRequest, TlsMode, UpdateAccountRequest,
};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::{
    serve_on_listener_with_overrides, AccountConnectionReport, AccountConnectionTester,
    AccountsEngineOverrides, CalendarEngineOverrides, ContactsEngineOverrides, MailEngineOverrides,
    ProtocolProbe,
};
use std::sync::Arc;
use tokio::net::TcpListener;

const ACCOUNT_ID: &str = "acct-lifecycle-e2e-1";
const ACCOUNT_EMAIL: &str = "lifecycle-e2e@nuncio.mx";
const KEYRING_KEY: &str = "nuncio/acct-lifecycle-e2e-1";

/// Injected stand-in [`AccountConnectionTester`]: returns a fixed but honest
/// per-protocol report (IMAP succeeds, SMTP fails with real error detail) so
/// `TestAccountConnection` can be exercised end to end WITHOUT any live
/// server. The daemon's production tester performs a genuine bounded network
/// probe; this stub stands in for that real world exactly as the mock
/// mail/calendar/contacts backends do for their protocols.
struct StubConnectionTester;

#[tonic::async_trait]
impl AccountConnectionTester for StubConnectionTester {
    async fn probe(
        &self,
        _config: &nuncio_core::AccountConfig,
        _password: &str,
    ) -> AccountConnectionReport {
        AccountConnectionReport {
            imap: ProtocolProbe {
                ok: true,
                error: None,
            },
            smtp: ProtocolProbe {
                ok: false,
                error: Some("SMTP relay refused the connection".to_string()),
            },
        }
    }
}

fn sample_account_config() -> AccountConfig {
    AccountConfig {
        id: ACCOUNT_ID.to_string(),
        name: "Lifecycle E2E Account".to_string(),
        email_address: ACCOUNT_EMAIL.to_string(),
        protocol: AccountProtocol::ImapSmtp.into(),
        server_host: "imap.nuncio.mx".to_string(),
        server_port: 143,
        use_tls: false,
        imap_tls_mode: TlsMode::StartTls.into(),
        smtp_tls_mode: TlsMode::Plain.into(),
        keyring_secret_key: KEYRING_KEY.to_string(),
        sync_interval_secs: 60,
        smtp_host: "smtp.nuncio.mx".to_string(),
        smtp_port: 25,
    }
}

/// Boots the real daemon gRPC server on an ephemeral loopback port with an
/// injected [`AccountsEngineOverrides`]. Returns the server address and the
/// minted token; the `TempDir` guard is returned so the caller controls its
/// lifetime for the whole test.
async fn boot_daemon(
    secrets: Arc<SecretManager>,
    accounts_overrides: AccountsEngineOverrides,
) -> (std::net::SocketAddr, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join("accounts_lifecycle_e2e.db");
    let db = Arc::new(
        DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .expect("open temp file-backed database"),
    );

    let token_bytes = secrets
        .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
        .expect("mint gRPC bearer token from mock vault");
    let token = hex::encode(token_bytes);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    let event_bus = Arc::new(EventBus::new());
    let server_secrets = secrets.clone();
    let server_token = token.clone();
    let server_filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
    tokio::spawn(async move {
        let _ = serve_on_listener_with_overrides(
            listener,
            event_bus,
            db,
            server_filter_engine,
            server_secrets,
            server_token,
            MailEngineOverrides::default(),
            CalendarEngineOverrides::default(),
            ContactsEngineOverrides::default(),
            accounts_overrides,
        )
        .await;
    });

    (addr, token, dir)
}

#[tokio::test]
async fn account_lifecycle_round_trips_over_authenticated_grpc_offline() {
    let secrets = Arc::new(SecretManager::mock());
    let accounts_overrides = AccountsEngineOverrides {
        connection_tester: Some(Arc::new(StubConnectionTester)),
    };
    let (addr, token, _db_dir) = boot_daemon(secrets, accounts_overrides).await;
    let addr_str = addr.to_string();

    let mut accounts_client = nuncio_proto::client::connect_accounts(&addr_str, &token)
        .await
        .expect("accounts client connects");

    // ---- (a) AddAccount persists a StartTls/Plain account ----
    let add_response = accounts_client
        .add_account(AddAccountRequest {
            config: Some(sample_account_config()),
            password: "lifecycle-secret-password".to_string(),
        })
        .await
        .expect("add_account succeeds")
        .into_inner();
    assert_eq!(add_response.id, ACCOUNT_ID);

    // ---- (b) ListAccounts round-trips BOTH distinct TLS modes ----
    let listed = accounts_client
        .list_accounts(ListAccountsRequest {})
        .await
        .expect("list_accounts succeeds")
        .into_inner()
        .accounts;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].imap_tls_mode(), TlsMode::StartTls);
    assert_eq!(listed[0].smtp_tls_mode(), TlsMode::Plain);

    // ---- (c) UpdateAccount genuinely mutates the persisted config ----
    let mut updated = sample_account_config();
    updated.name = "Renamed Lifecycle Account".to_string();
    updated.imap_tls_mode = TlsMode::ImplicitTls.into();
    accounts_client
        .update_account(UpdateAccountRequest {
            config: Some(updated),
            password: None,
        })
        .await
        .expect("update_account succeeds");

    let relisted = accounts_client
        .list_accounts(ListAccountsRequest {})
        .await
        .expect("list_accounts succeeds")
        .into_inner()
        .accounts;
    assert_eq!(relisted.len(), 1);
    assert_eq!(relisted[0].name, "Renamed Lifecycle Account");
    assert_eq!(relisted[0].imap_tls_mode(), TlsMode::ImplicitTls);
    // Untouched fields survive the update.
    assert_eq!(relisted[0].smtp_tls_mode(), TlsMode::Plain);

    // ---- (d) TestAccountConnection reports the GENUINE per-protocol result ----
    let report = accounts_client
        .test_account_connection(TestAccountConnectionRequest {
            id: ACCOUNT_ID.to_string(),
        })
        .await
        .expect("test_account_connection succeeds")
        .into_inner();
    assert!(report.imap_ok);
    assert!(!report.smtp_ok);
    assert_eq!(report.imap_error, None);
    assert_eq!(
        report.smtp_error.as_deref(),
        Some("SMTP relay refused the connection")
    );

    // ---- (e) RemoveAccount genuinely deletes it ----
    accounts_client
        .remove_account(RemoveAccountRequest {
            id: ACCOUNT_ID.to_string(),
        })
        .await
        .expect("remove_account succeeds");

    let after_remove = accounts_client
        .list_accounts(ListAccountsRequest {})
        .await
        .expect("list_accounts succeeds")
        .into_inner()
        .accounts;
    assert!(after_remove.is_empty());

    // A test against the now-removed account is an honest not-found, never a
    // fabricated success.
    let err = accounts_client
        .test_account_connection(TestAccountConnectionRequest {
            id: ACCOUNT_ID.to_string(),
        })
        .await
        .expect_err("testing a removed account must fail");
    assert_eq!(err.code(), tonic::Code::NotFound);
}
