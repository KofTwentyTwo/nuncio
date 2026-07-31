//! Full-daemon offline end-to-end test for the contacts vertical.
//!
//! Boots a real `nunciod` daemon gRPC server
//! (`nunciod::grpc::serve_on_listener_with_overrides`) on an ephemeral
//! loopback port, backed by a real temp file-backed [`DatabaseEngine`] and a
//! [`SecretManager::mock`] vault (never the real OS keyring), with an
//! injected [`nuncio_contacts::MockContactsBackend`] standing in for real
//! network I/O (via the [`nunciod::grpc::ContactsEngineOverrides`] injection
//! seam).
//!
//! Driving ONLY the authenticated `nuncio-proto` gRPC client
//! (`connect_contacts`) with the shared bearer token -- never reaching into
//! daemon internals -- this proves:
//!   a. `Contacts/Sync` fetches from the injected mock backend and persists
//!      the result, awaiting full completion before returning, so the
//!      synced contacts are immediately visible to `ListContacts`/
//!      `GetContact`.
//!   b. `Contacts/CreateContact` persists a locally-authored contact
//!      directly to the daemon's real store, and it is visible via a
//!      SECOND, SEPARATE client connection against the same daemon --
//!      proving real cross-process-style persistence, not just
//!      within-process state.
//!   c. An unauthenticated call to `Contacts` is rejected with
//!      `Code::Unauthenticated`, proving it is mounted behind its own
//!      `BearerAuthInterceptor` exactly like every other service.
//!
//! Deterministic and fully offline: no fixed sleeps, no real network, no
//! real OS keyring.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_contacts::{Contact, MockContactsBackend};
use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_proto::v1::{
    ContactEmail as ContactEmailProto, ContactsSyncRequest, CreateContactRequest,
    GetContactRequest, ListContactsRequest,
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

const ACCOUNT_ID: &str = "acct-contacts-e2e-1";

fn mock_contact(id: &str, display_name: &str) -> Contact {
    let mut contact = Contact::new(display_name, format!("{display_name}@nuncio.mx"));
    contact.id = id.to_string();
    contact.account_id = Some(ACCOUNT_ID.to_string());
    contact
}

/// Boots the real daemon gRPC server, injecting `contacts_overrides`, on an
/// ephemeral loopback port backed by a real temp file-backed
/// [`DatabaseEngine`]. Returns the server address and the minted bearer
/// token; the `TempDir` guard is returned too so the caller controls its
/// lifetime for the whole test.
async fn boot_daemon(
    secrets: Arc<SecretManager>,
    contacts_overrides: ContactsEngineOverrides,
) -> (std::net::SocketAddr, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join("contacts_e2e.db");
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
            contacts_overrides,
            AccountsEngineOverrides::default(),
        )
        .await;
    });

    (addr, token, dir)
}

/// The full offline, deterministic end-to-end proof of the contacts
/// vertical: `Sync` against an injected mock backend, then `ListContacts`/
/// `GetContact` immediately reflect the persisted result; `CreateContact`
/// persists a locally-authored contact that a SECOND, independent client
/// connection can then read back -- ALL over the authenticated
/// `nuncio.v1.Contacts` gRPC API against a live in-process daemon, with
/// protocol I/O fully mocked.
#[tokio::test]
async fn contacts_sync_create_and_list_round_trip_over_authenticated_grpc() {
    let secrets = Arc::new(SecretManager::mock());

    let mock_backend = MockContactsBackend::new();
    mock_backend.add_contact(mock_contact("ct-e2e-1", "E2E Contact One"));
    mock_backend.add_contact(mock_contact("ct-e2e-2", "E2E Contact Two"));

    let contacts_overrides = ContactsEngineOverrides {
        contacts_backend: Some(Arc::new(mock_backend)),
    };

    let (addr, token, _db_dir) = boot_daemon(secrets, contacts_overrides).await;
    let addr_str = addr.to_string();

    let mut contacts_client = nuncio_proto::client::connect_contacts(&addr_str, &token)
        .await
        .expect("contacts client connects");

    // ---- (a) Sync fetches from the injected mock backend and persists ----
    let sync_response = contacts_client
        .sync(ContactsSyncRequest {
            account_id: ACCOUNT_ID.to_string(),
        })
        .await
        .expect("sync succeeds")
        .into_inner();
    assert_eq!(sync_response.synced_count, 2);

    // Immediately visible, no fixed sleep: `Sync` only returns once the
    // fetch-and-persist work has fully completed.
    let list_response = contacts_client
        .list_contacts(ListContactsRequest {
            account_id: ACCOUNT_ID.to_string(),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect("list_contacts succeeds")
        .into_inner();
    assert_eq!(list_response.contacts.len(), 2);
    let synced_ids: Vec<&str> = list_response
        .contacts
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert!(synced_ids.contains(&"ct-e2e-1"));
    assert!(synced_ids.contains(&"ct-e2e-2"));

    let get_response = contacts_client
        .get_contact(GetContactRequest {
            contact_id: "ct-e2e-1".to_string(),
        })
        .await
        .expect("get_contact succeeds")
        .into_inner();
    let contact = get_response.contact.expect("contact present in response");
    assert_eq!(contact.display_name, "E2E Contact One");

    // ---- (b) CreateContact persists real local state, visible via a
    // SECOND, independent client connection ----
    let create_response = contacts_client
        .create_contact(CreateContactRequest {
            account_id: ACCOUNT_ID.to_string(),
            display_name: "CLI Added Contact".to_string(),
            organization: Some("Kof22".to_string()),
            emails: vec![ContactEmailProto {
                email: "cli.added@kof22.com".to_string(),
                label: "work".to_string(),
                is_primary: true,
            }],
            phones: vec![],
        })
        .await
        .expect("create_contact succeeds")
        .into_inner();
    let created_id = create_response
        .contact
        .expect("created contact present in response")
        .id;

    // A brand-new client connection (simulating a second, separate CLI
    // invocation against the same running daemon) sees the created contact
    // -- proving real persistence through the daemon's own store, not
    // process-local state.
    let mut second_client = nuncio_proto::client::connect_contacts(&addr_str, &token)
        .await
        .expect("second contacts client connects");
    let second_list = second_client
        .list_contacts(ListContactsRequest {
            account_id: ACCOUNT_ID.to_string(),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect("list_contacts succeeds on second connection")
        .into_inner();
    assert_eq!(second_list.contacts.len(), 3);
    let second_ids: Vec<&str> = second_list.contacts.iter().map(|c| c.id.as_str()).collect();
    assert!(second_ids.contains(&created_id.as_str()));

    let second_get = second_client
        .get_contact(GetContactRequest {
            contact_id: created_id,
        })
        .await
        .expect("get_contact succeeds on second connection")
        .into_inner();
    let created_contact = second_get.contact.expect("contact present in response");
    assert_eq!(created_contact.display_name, "CLI Added Contact");
    assert_eq!(created_contact.organization.as_deref(), Some("Kof22"));

    // ---- (c) An unauthenticated call is rejected ----
    // Uses the raw generated client (bypassing `connect_contacts`'s
    // bearer-token interceptor) to prove `Contacts` is mounted behind its
    // own `BearerAuthInterceptor`, exactly like every other service.
    let mut unauthenticated_client =
        nuncio_proto::v1::contacts_client::ContactsClient::connect(format!("http://{addr_str}"))
            .await
            .expect("client connects at the transport level without any auth");
    let err = unauthenticated_client
        .list_contacts(ListContactsRequest {
            account_id: ACCOUNT_ID.to_string(),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect_err("missing bearer token must be rejected");
    assert_eq!(err.code(), Code::Unauthenticated);
}
