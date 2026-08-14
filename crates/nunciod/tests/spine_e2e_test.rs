//! Capstone offline end-to-end test for the mail spine.
//!
//! Boots a real `nunciod` daemon gRPC server (`nunciod::grpc::
//! serve_on_listener_with_overrides`) on an ephemeral loopback port, backed
//! by a real temp file-backed [`DatabaseEngine`] and a [`SecretManager::mock`]
//! vault (never the real OS keyring), with an injected
//! [`nuncio_mail::MockMailBackend`] / [`nuncio_mail::MockMessageSender`]
//! standing in for real network I/O (via the
//! [`nunciod::grpc::MailEngineOverrides`] injection seam).
//!
//! Then, driving ONLY the authenticated `nuncio-proto` gRPC clients
//! (`connect_system`/`connect_accounts`/`connect_mail`) with the shared
//! bearer token -- never reaching into daemon internals -- this proves the
//! complete spine end-to-end:
//!   a. `AddAccount` persists (`ListAccounts` reflects it).
//!   b. `Mail/Sync` triggers a real inbound sync against the injected mock
//!      backend and awaits full completion, so
//!      the synced messages are immediately visible to
//!      `ListMessages`/`GetMessage`.
//!   c. `MarkRead` persists (a second `GetMessage` reflects the flip) and a
//!      `MessageFlagsChanged` event arrives on a live `System/Subscribe`
//!      stream.
//!   d. `SendMessage` sends through the injected `MockMessageSender`, which
//!      captures the EXACT outbound message (recipients/subject/body).
//!
//! Deterministic and fully offline: no fixed sleeps (the `Sync` RPC awaits
//! completion before returning, and the `Subscribe` stream is opened and
//! its registration awaited BEFORE the event-producing call that follows
//! it, exactly as `nunciod::grpc`'s own tests do), no real network, no real
//! OS keyring.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::model::{Email, Folder, IdentitySource, Placement};
use nuncio_core::EventBus;
use nuncio_filter::FilterEngine;
use nuncio_mail::{MockMailBackend, MockMessageSender, PlacedMessage};
use nuncio_proto::v1::event::Kind;
use nuncio_proto::v1::{
    AccountConfig, AddAccountRequest, GetMessageRequest, ListAccountsRequest, ListMessagesRequest,
    MarkReadRequest, SendMessageRequest, SyncRequest, TlsMode,
};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::{
    serve_on_listener_with_overrides, AccountsEngineOverrides, CalendarEngineOverrides,
    ContactsEngineOverrides, MailEngineOverrides,
};
use std::sync::Arc;
use tokio::net::TcpListener;

const ACCOUNT_ID: &str = "acct-spine-e2e-1";
const ACCOUNT_EMAIL: &str = "spine-e2e@nuncio.mx";
const KEYRING_KEY: &str = "nuncio/acct-spine-e2e-1";

fn sample_account_config() -> AccountConfig {
    AccountConfig {
        id: ACCOUNT_ID.to_string(),
        name: "Spine E2E Test Account".to_string(),
        email_address: ACCOUNT_EMAIL.to_string(),
        keyring_secret_key: KEYRING_KEY.to_string(),
        sync_interval: Some(nuncio_proto::time::duration_from_secs(60)),
        transport: Some(nuncio_proto::v1::account_config::Transport::ImapSmtp(
            nuncio_proto::v1::ImapSmtpTransport {
                imap_host: "imap.nuncio.mx".to_string(),
                imap_port: 993,
                imap_tls_mode: TlsMode::ImplicitTls.into(),
                smtp_host: "smtp.nuncio.mx".to_string(),
                smtp_port: 465,
                smtp_tls_mode: TlsMode::ImplicitTls.into(),
            },
        )),
    }
}

/// One inbound message as a backend surfaces it: identity, the tier the key
/// came from, and the INBOX occupancy this pass found it in.
fn mock_inbound_email(id: &str, subject: &str, body: &str) -> PlacedMessage {
    PlacedMessage {
        email: Email {
            id: id.to_string(),
            account_id: ACCOUNT_ID.to_string(),
            subject: subject.to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: ACCOUNT_EMAIL.to_string(),
            received_at: 1_700_000_000,
            body_plain: Some(body.to_string()),
            body_html: None,
            attachments: Vec::new(),
            message_id: None,
            content_hash: None,
        },
        source: IdentitySource::Surrogate,
        placement: Placement {
            account_id: ACCOUNT_ID.to_string(),
            folder_id: "inbox".to_string(),
            uid_validity: "1".to_string(),
            remote_id: id.to_string(),
            read: false,
        },
    }
}

/// Boots the real daemon gRPC server (all three `nuncio.v1` services,
/// authenticated by a bearer token minted from `secrets`) on an ephemeral
/// loopback port, backed by a real temp file-backed [`DatabaseEngine`] and
/// the given [`MailEngineOverrides`]. Returns the server address and the
/// minted token; the `TempDir` guard and `DatabaseEngine` handle are
/// returned too so the caller controls their lifetime for the whole test.
async fn boot_daemon(
    secrets: Arc<SecretManager>,
    overrides: MailEngineOverrides,
) -> (std::net::SocketAddr, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join("spine_e2e.db");
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
            overrides,
            CalendarEngineOverrides::default(),
            ContactsEngineOverrides::default(),
            AccountsEngineOverrides::default(),
        )
        .await;
    });

    (addr, token, dir)
}

/// The full offline, deterministic end-to-end proof of the mail spine: add
/// an account, sync inbound mail, list and read it, mark it read (observing
/// the change stream live), and send an outbound message -- ALL over the
/// authenticated `nuncio.v1` gRPC API against a live in-process daemon,
/// with protocol I/O fully mocked.
#[tokio::test]
async fn full_mail_spine_round_trips_over_authenticated_grpc_with_mocked_protocol_io() {
    // ---- Arrange: one shared mock vault, an injected mock backend/sender ----
    let secrets = Arc::new(SecretManager::mock());

    let mock_backend = MockMailBackend::new();
    mock_backend.add_folder(Folder {
        id: "inbox".to_string(),
        name: "Inbox".to_string(),
        total_messages: 2,
        unread_messages: 2,
    });
    mock_backend.add_message(mock_inbound_email(
        "msg-spine-1",
        "Welcome to Nuncio",
        "This is the first synced message.",
    ));
    mock_backend.add_message(mock_inbound_email(
        "msg-spine-2",
        "Second Message",
        "This is the second synced message.",
    ));

    let mock_sender = MockMessageSender::new();
    // Kept as a separate handle so assertions below can inspect what the
    // daemon sent without going through gRPC (the mock IS the "real world"
    // stand-in here, not something under test).
    let sent_messages_probe = mock_sender.clone();

    let overrides = MailEngineOverrides {
        mail_backend: Some(Arc::new(mock_backend)),
        message_sender: Some(Arc::new(mock_sender)),
    };

    let (addr, token, _db_dir) = boot_daemon(secrets, overrides).await;
    let addr_str = addr.to_string();

    let mut system_client = nuncio_proto::client::connect_system(&addr_str, &token)
        .await
        .expect("system client connects");
    let mut accounts_client = nuncio_proto::client::connect_accounts(&addr_str, &token)
        .await
        .expect("accounts client connects");
    let mut mail_client = nuncio_proto::client::connect_mail(&addr_str, &token)
        .await
        .expect("mail client connects");

    // ---- (a) AddAccount persists, confirmed via ListAccounts ----
    let add_response = accounts_client
        .add_account(AddAccountRequest {
            config: Some(sample_account_config()),
            password: "spine-e2e-secret-password".to_string(),
        })
        .await
        .expect("add_account succeeds")
        .into_inner();
    assert_eq!(
        add_response
            .config
            .expect("response carries the created config")
            .id,
        ACCOUNT_ID
    );

    let list_response = accounts_client
        .list_accounts(ListAccountsRequest {})
        .await
        .expect("list_accounts succeeds")
        .into_inner();
    assert_eq!(list_response.accounts.len(), 1);
    assert_eq!(list_response.accounts[0].id, ACCOUNT_ID);
    assert_eq!(list_response.accounts[0].email_address, ACCOUNT_EMAIL);

    // ---- (b) Mail/Sync triggers a real inbound sync against the mock ----
    // backend and awaits full completion -- no fixed sleep is needed before
    // the ListMessages/GetMessage calls immediately below.
    let sync_response = mail_client
        .sync(SyncRequest { account_id: None })
        .await
        .expect("sync succeeds")
        .into_inner();
    assert_eq!(sync_response.synced_count, 2);

    let list_messages_response = mail_client
        .list_messages(ListMessagesRequest {
            folder_id: "inbox".to_string(),
            page_size: 0,
            page_token: String::new(),
        })
        .await
        .expect("list_messages succeeds")
        .into_inner();
    assert_eq!(list_messages_response.messages.len(), 2);
    let synced_ids: Vec<&str> = list_messages_response
        .messages
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert!(synced_ids.contains(&"msg-spine-1"));
    assert!(synced_ids.contains(&"msg-spine-2"));

    let get_message_response = mail_client
        .get_message(GetMessageRequest {
            message_id: "msg-spine-1".to_string(),
        })
        .await
        .expect("get_message succeeds")
        .into_inner();
    let synced_message = get_message_response
        .message
        .expect("message present in response");
    assert_eq!(synced_message.subject, "Welcome to Nuncio");
    assert_eq!(
        synced_message.body_plain.as_deref(),
        Some("This is the first synced message.")
    );
    assert!(!synced_message.read);

    // ---- (c) MarkRead persists, and streams on System/Subscribe ----
    // Subscribing BEFORE calling MarkRead guarantees (by the server's own
    // documented contract, see `nunciod::grpc::SystemGrpcService::subscribe`)
    // that once this `.subscribe()` call resolves, any event published
    // afterwards -- including the one MarkRead is about to publish -- is
    // observed on this stream. No fixed sleep is needed.
    let mut event_stream = nuncio_proto::client::subscribe_events(&mut system_client)
        .await
        .expect("subscribe succeeds");

    mail_client
        .mark_read(MarkReadRequest {
            message_id: "msg-spine-1".to_string(),
            read: true,
        })
        .await
        .expect("mark_read succeeds");

    let refetched = mail_client
        .get_message(GetMessageRequest {
            message_id: "msg-spine-1".to_string(),
        })
        .await
        .expect("get_message succeeds after mark_read")
        .into_inner()
        .message
        .expect("message present in response");
    assert!(refetched.read);

    let event = event_stream
        .message()
        .await
        .expect("stream yields without a transport error")
        .expect("stream produces an event rather than ending");
    match event.kind {
        Some(Kind::MessageFlagsChanged(changed)) => {
            assert_eq!(changed.message_id, "msg-spine-1");
            assert!(changed.read);
        }
        other => panic!("expected a mapped MessageFlagsChanged event, got {other:?}"),
    }

    // ---- (d) SendMessage sends through the injected MockMessageSender ----
    let send_response = mail_client
        .send_message(SendMessageRequest {
            to: "bob@nuncio.mx".to_string(),
            cc: Some("carol@nuncio.mx".to_string()),
            subject: "Quarterly Roadmap".to_string(),
            body_text: "Let's discuss the roadmap.".to_string(),
            body_html: None,
            attachments: Vec::new(),
            account_id: None,
        })
        .await
        .expect("send_message succeeds")
        .into_inner();
    assert!(send_response.message_id.starts_with("sent-"));

    let sent = sent_messages_probe.sent_messages();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from, ACCOUNT_EMAIL);
    assert_eq!(sent[0].to, "bob@nuncio.mx");
    assert_eq!(sent[0].cc.as_deref(), Some("carol@nuncio.mx"));
    assert_eq!(sent[0].subject, "Quarterly Roadmap");
    assert_eq!(
        sent[0].body_plain.as_deref(),
        Some("Let's discuss the roadmap.")
    );
}
