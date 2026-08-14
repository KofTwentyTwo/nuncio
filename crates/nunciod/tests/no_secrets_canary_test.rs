//! Cross-crate "no secrets in logs" canary.
//!
//! This is the regression net for the logging redaction policy documented in
//! `nuncio_core::redact`. It installs a capturing `tracing` subscriber, seeds a
//! set of unmistakable SENTINEL secrets/PII, then drives a representative slice
//! of the now-instrumented code paths across the daemon and the library crates
//! using only mock backends and loopback wiremock servers (no live network):
//!
//!   - gRPC auth reject (a wrong bearer token) and authenticated calls;
//!   - an account add carrying an account password;
//!   - a mock inbound mail sync carrying a sentinel subject/body/sender/recipient;
//!   - a real JMAP protocol sync (wiremock) carrying the same sentinel content
//!     plus sentinel credentials;
//!   - an outbox-affecting mark-read and an outbound send carrying sentinel content;
//!   - a filter rule firing on a sentinel message;
//!   - a webhook dispatch signed with a sentinel HMAC key;
//!   - a CalDAV and a CardDAV sync authenticated with a sentinel token and
//!     carrying sentinel event/contact PII.
//!
//! It then asserts that NONE of the sentinel strings appear anywhere in the
//! captured telemetry, while also proving the log-emitting paths actually ran
//! (so the test can never pass by capturing nothing).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use nuncio_cal::{CalDavAccountConfig, CalDavClient, CalendarBackend};
use nuncio_contacts::{CardDavAccountConfig, CardDavClient, ContactsBackend};
use nuncio_core::model::{Email, Folder, IdentitySource, Placement};
use nuncio_core::EventBus;
use nuncio_filter::{FilterEngine, NsqlParser, ValidationOptions, WebhookDispatcher};
use nuncio_mail::{JmapEngine, MailBackend, MockMailBackend, MockMessageSender, PlacedMessage};
use nuncio_proto::v1::{
    AccountConfig, AddAccountRequest, GetStatusRequest, MarkReadRequest, SendMessageRequest,
    SyncRequest, TlsMode,
};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::{
    serve_on_listener_with_overrides, AccountsEngineOverrides, CalendarEngineOverrides,
    ContactsEngineOverrides, MailEngineOverrides,
};
use tokio::net::TcpListener;
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- Sentinels: none of these may EVER appear in captured telemetry. ----
const SENTINEL_BEARER: &str = "SENTINELbearerTokenAaBbCc001";
const SENTINEL_PASSWORD: &str = "SENTINELaccountPasswordDdEe002";
const SENTINEL_SUBJECT: &str = "SENTINELmessageSubjectFfGg003";
const SENTINEL_BODY: &str = "SENTINELmessageBodyHhIi004";
const SENTINEL_SENDER: &str = "sentinel-sender-jjkk005@example.invalid";
const SENTINEL_RECIPIENT: &str = "sentinel-recipient-llmm006@example.invalid";
const SENTINEL_WEBHOOK_KEY: &str = "SENTINELwebhookHmacKeyNnOo007";
const SENTINEL_DAV_TOKEN: &str = "SENTINELdavAuthTokenPpQq008";
const SENTINEL_EVENT_SUMMARY: &str = "SENTINELeventSummaryRrSs009";
const SENTINEL_CONTACT_NAME: &str = "SENTINELcontactNameTtUu010";

const ACCOUNT_ID: &str = "acct-canary-1";
const ACCOUNT_EMAIL: &str = "canary@nuncio.mx";
const KEYRING_KEY: &str = "nuncio/acct-canary-1";

/// Thread-safe sink recording every rendered field value from every captured
/// span and event, so a single global subscriber can collect telemetry emitted
/// on any daemon/runtime thread.
#[derive(Default)]
struct Sink(Mutex<Vec<String>>);

impl Sink {
    fn push_all(&self, values: Vec<String>) {
        if let Ok(mut guard) = self.0.lock() {
            guard.extend(values);
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.0.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

struct FieldVisitor(Vec<String>);

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push(format!("{}={}", field.name(), value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push(format!("{}={:?}", field.name(), value));
    }
}

struct CaptureLayer(Arc<Sink>);

impl<S> tracing_subscriber::Layer<S> for CaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor(vec![format!("span={}", attrs.metadata().name())]);
        attrs.record(&mut visitor);
        self.0.push_all(visitor.0);
    }

    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor(vec![format!("event={}", event.metadata().target())]);
        event.record(&mut visitor);
        self.0.push_all(visitor.0);
    }
}

fn sample_proto_config() -> AccountConfig {
    AccountConfig {
        id: ACCOUNT_ID.to_string(),
        name: "Canary Account".to_string(),
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

fn sentinel_inbound_email() -> Email {
    Email {
        id: "msg-canary-1".to_string(),
        account_id: ACCOUNT_ID.to_string(),
        subject: SENTINEL_SUBJECT.to_string(),
        sender: SENTINEL_SENDER.to_string(),
        recipient: SENTINEL_RECIPIENT.to_string(),
        received_at: 1_700_000_000,
        body_plain: Some(SENTINEL_BODY.to_string()),
        body_html: None,
        attachments: Vec::new(),
        message_id: None,
        content_hash: None,
    }
}

/// The INBOX occupancy the sentinel message is found in.
fn sentinel_placement() -> Placement {
    Placement {
        account_id: ACCOUNT_ID.to_string(),
        folder_id: "inbox".to_string(),
        uid_validity: "1".to_string(),
        remote_id: "msg-canary-1".to_string(),
        read: false,
    }
}

/// The sentinel message as a backend would surface it.
fn sentinel_placed_message() -> PlacedMessage {
    PlacedMessage {
        email: sentinel_inbound_email(),
        source: IdentitySource::Surrogate,
        placement: sentinel_placement(),
    }
}

/// Boots the real daemon gRPC server (all services, bearer-authenticated) on an
/// ephemeral loopback port, backed by a real temp file DB and the given mail
/// overrides + filter engine.
async fn boot_daemon(
    secrets: Arc<SecretManager>,
    overrides: MailEngineOverrides,
    filter_engine: Arc<FilterEngine>,
) -> (std::net::SocketAddr, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir for db");
    let db_path = dir.path().join("canary.db");
    let db = Arc::new(
        DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .expect("open temp file db"),
    );
    let token = hex::encode(
        secrets
            .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
            .expect("mint bearer token from mock vault"),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has addr");

    let event_bus = Arc::new(EventBus::new());
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
            overrides,
            CalendarEngineOverrides::default(),
            ContactsEngineOverrides::default(),
            AccountsEngineOverrides::default(),
        )
        .await;
    });
    (addr, token, dir)
}

/// Drives the daemon over authenticated gRPC: an auth-reject with a sentinel
/// bearer token, an account add with a sentinel password, an inbound sync
/// carrying sentinel content, a mark-read, and an outbound send.
async fn drive_daemon() {
    let secrets = Arc::new(SecretManager::mock());

    let mock_backend = MockMailBackend::new();
    mock_backend.add_folder(Folder {
        id: "inbox".to_string(),
        name: "Inbox".to_string(),
        total_messages: 1,
        unread_messages: 1,
    });
    mock_backend.add_message(sentinel_placed_message());
    let mock_sender = MockMessageSender::new();
    let overrides = MailEngineOverrides {
        mail_backend: Some(Arc::new(mock_backend)),
        message_sender: Some(Arc::new(mock_sender)),
    };

    // A rule that fires on the sentinel subject, so the daemon's filter path is
    // exercised against sentinel content during any evaluation it performs.
    let rule = NsqlParser::parse_rule(
        "Canary Rule",
        1,
        &format!("WHERE subject CONTAINS '{SENTINEL_SUBJECT}' ACTION MARK READ"),
    )
    .expect("parse canary rule");
    let filter_engine = Arc::new(FilterEngine::new(vec![rule]).expect("compile rule"));

    let (addr, token, _db_dir) = boot_daemon(secrets, overrides, filter_engine).await;
    let addr_str = addr.to_string();

    // (1) Auth reject: a call bearing the WRONG (sentinel) token is rejected,
    // and that token must never be echoed to a log.
    let mut unauth =
        nuncio_proto::v1::system_client::SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("system client connects");
    let mut bad = tonic::Request::new(GetStatusRequest {});
    bad.metadata_mut().insert(
        "authorization",
        format!("Bearer {SENTINEL_BEARER}")
            .parse()
            .expect("ascii metadata"),
    );
    let err = unauth
        .get_status(bad)
        .await
        .expect_err("wrong bearer token must be rejected");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    let mut accounts = nuncio_proto::client::connect_accounts(&addr_str, &token)
        .await
        .expect("accounts client");
    let mut mail = nuncio_proto::client::connect_mail(&addr_str, &token)
        .await
        .expect("mail client");

    // (2) Account add carrying a sentinel password.
    accounts
        .add_account(AddAccountRequest {
            config: Some(sample_proto_config()),
            password: SENTINEL_PASSWORD.to_string(),
        })
        .await
        .expect("add_account succeeds");

    // (3) Inbound sync over the mock backend carrying sentinel content.
    let synced = mail
        .sync(SyncRequest { account_id: None })
        .await
        .expect("sync succeeds")
        .into_inner();
    assert_eq!(synced.synced_count, 1);

    // (4) Outbox-affecting mark-read.
    mail.mark_read(MarkReadRequest {
        message_id: "msg-canary-1".to_string(),
        read: true,
    })
    .await
    .expect("mark_read succeeds");

    // (5) Outbound send carrying sentinel content.
    mail.send_message(SendMessageRequest {
        to: SENTINEL_RECIPIENT.to_string(),
        cc: None,
        subject: SENTINEL_SUBJECT.to_string(),
        body_text: SENTINEL_BODY.to_string(),
        body_html: None,
        attachments: Vec::new(),
        account_id: None,
    })
    .await
    .expect("send_message succeeds");
}

/// Drives the filter engine (a rule fires on the sentinel email) and a webhook
/// dispatch signed with a sentinel HMAC key against a loopback wiremock server.
async fn drive_filter_and_webhook() {
    let rule = NsqlParser::parse_rule(
        "Canary Webhook Rule",
        1,
        &format!("WHERE subject CONTAINS '{SENTINEL_SUBJECT}' ACTION MARK READ"),
    )
    .expect("parse rule");
    let engine = FilterEngine::new(vec![rule]).expect("compile rule");
    let sentinel = sentinel_inbound_email();
    let placement = sentinel_placement();
    let matches = engine.evaluate(nuncio_filter::PlacedEmail {
        email: &sentinel,
        placement: &placement,
    });
    assert_eq!(matches.len(), 1, "rule must fire on the sentinel message");

    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let dispatcher = WebhookDispatcher::new(SENTINEL_WEBHOOK_KEY);
    let opts = ValidationOptions {
        available_folders: None,
        allowed_forward_domains: None,
        block_private_webhooks: false,
    };
    let url = format!("{}/hook", mock_server.uri());
    let status = dispatcher
        .dispatch_with_options(
            &url,
            "rule-canary",
            "msg-canary-1",
            SENTINEL_SUBJECT,
            SENTINEL_SENDER,
            &opts,
        )
        .await
        .expect("webhook dispatch to loopback mock succeeds");
    assert_eq!(status, 200);
}

/// Drives a CalDAV and a CardDAV sync authenticated with the sentinel token and
/// carrying sentinel PII, against loopback wiremock servers.
async fn drive_dav() {
    // CalDAV.
    let cal_server = MockServer::start().await;
    let cal_body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
    <d:response>
        <d:href>/calendars/work/e.ics</d:href>
        <d:propstat><d:prop><c:calendar-data>BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
SUMMARY:{SENTINEL_EVENT_SUMMARY}
DTSTART:20240301T120000Z
DTEND:20240301T130000Z
END:VEVENT
END:VCALENDAR</c:calendar-data></d:prop></d:propstat>
    </d:response>
</d:multistatus>"#
    );
    Mock::given(method("REPORT"))
        .and(path("/calendars/work/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(cal_body))
        .mount(&cal_server)
        .await;
    let cal_client = CalDavClient::new(CalDavAccountConfig {
        account_id: ACCOUNT_ID.to_string(),
        caldav_url: format!("{}/calendars/work/", cal_server.uri()),
        username: ACCOUNT_EMAIL.to_string(),
        auth_token: SENTINEL_DAV_TOKEN.to_string().into(),
    });
    let events =
        CalendarBackend::fetch_events(&cal_client, "cal-work", 1_700_000_000, 1_800_000_000)
            .await
            .expect("caldav fetch succeeds");
    assert_eq!(events.len(), 1);

    // CardDAV.
    let card_server = MockServer::start().await;
    let card_body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <d:response>
        <d:href>/addressbooks/contacts/c.vcf</d:href>
        <d:propstat><d:prop><card:address-data>BEGIN:VCARD
VERSION:4.0
FN:{SENTINEL_CONTACT_NAME}
EMAIL:{SENTINEL_SENDER}
END:VCARD</card:address-data></d:prop></d:propstat>
    </d:response>
</d:multistatus>"#
    );
    Mock::given(method("REPORT"))
        .and(path("/addressbooks/contacts/"))
        .respond_with(ResponseTemplate::new(207).set_body_string(card_body))
        .mount(&card_server)
        .await;
    let card_client = CardDavClient::new(CardDavAccountConfig {
        account_id: ACCOUNT_ID.to_string(),
        carddav_url: format!("{}/addressbooks/contacts/", card_server.uri()),
        username: ACCOUNT_EMAIL.to_string(),
        auth_token: SENTINEL_DAV_TOKEN.to_string().into(),
    });
    let contacts = ContactsBackend::fetch_contacts(&card_client, ACCOUNT_ID)
        .await
        .expect("carddav fetch succeeds");
    assert_eq!(contacts.len(), 1);
}

/// Drives a real JMAP protocol sync (session discovery + Mailbox/get +
/// Email/query + Email/get) with sentinel credentials, carrying sentinel
/// content in the fetched message.
async fn drive_jmap() {
    let server = MockServer::start().await;
    let session_body = serde_json::json!({
        "username": "canary@nuncio.mx",
        "primaryAccounts": { "urn:ietf:params:jmap:mail": "acct-100" },
        "apiUrl": format!("{}/jmap/api", server.uri()),
        "state": "session-state-1"
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&session_body))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(serde_json::json!({
            "methodCalls": [["Mailbox/get", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "methodResponses": [["Mailbox/get", {
                "accountId": "acct-100",
                "list": [{"id": "mb-inbox", "name": "Inbox", "totalEmails": 1, "unreadEmails": 1}]
            }, "c1"]]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(serde_json::json!({
            "methodCalls": [["Email/query", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "methodResponses": [["Email/query", {"ids": ["msg-jmap-1"]}, "c1"]]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(serde_json::json!({
            "methodCalls": [["Email/get", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "methodResponses": [["Email/get", {
                "accountId": "acct-100",
                "state": "sync-state-1",
                "list": [{
                    "id": "msg-jmap-1",
                    "subject": SENTINEL_SUBJECT,
                    "from": [{ "email": SENTINEL_SENDER }],
                    "to": [{ "email": SENTINEL_RECIPIENT }],
                    "receivedAt": 1700002000i64,
                    "isUnread": true,
                    "bodySnippet": SENTINEL_BODY
                }]
            }, "c1"]]
        })))
        .mount(&server)
        .await;

    let engine =
        JmapEngine::with_credentials("acct-100", &server.uri(), ACCOUNT_EMAIL, SENTINEL_PASSWORD);
    let folders = engine.sync_folders().await.expect("jmap sync_folders");
    assert_eq!(folders.len(), 1);
    let (emails, _state) = engine
        .sync_messages("mb-inbox", None)
        .await
        .expect("jmap sync_messages");
    assert_eq!(emails.len(), 1);
}

#[tokio::test]
async fn no_sentinel_secret_appears_in_any_captured_telemetry() {
    let sink = Arc::new(Sink::default());
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(sink.clone()));
    tracing::subscriber::set_global_default(subscriber)
        .expect("install the global capturing subscriber");

    drive_daemon().await;
    drive_filter_and_webhook().await;
    drive_dav().await;
    drive_jmap().await;

    let captured = sink.snapshot();

    // Prove the log-emitting paths actually ran: the test must never pass by
    // capturing nothing.
    assert!(
        !captured.is_empty(),
        "no telemetry captured -- the canary exercised nothing"
    );
    let haystack = captured.join("\n");
    assert!(
        haystack.contains("webhook"),
        "expected the webhook dispatch path to have logged; got:\n{haystack}"
    );
    assert!(
        haystack.contains("rule matched") || haystack.contains("dispatching actions"),
        "expected the filter rule-fire path to have logged; got:\n{haystack}"
    );

    // The core assertion: no sentinel secret or PII may appear anywhere.
    let sentinels = [
        ("bearer token", SENTINEL_BEARER),
        ("account password", SENTINEL_PASSWORD),
        ("message subject", SENTINEL_SUBJECT),
        ("message body", SENTINEL_BODY),
        ("sender address", SENTINEL_SENDER),
        ("recipient address", SENTINEL_RECIPIENT),
        ("webhook signing key", SENTINEL_WEBHOOK_KEY),
        ("DAV auth token", SENTINEL_DAV_TOKEN),
        ("calendar event summary", SENTINEL_EVENT_SUMMARY),
        ("contact name", SENTINEL_CONTACT_NAME),
    ];
    for (label, sentinel) in sentinels {
        let leaked: Vec<&String> = captured
            .iter()
            .filter(|line| line.contains(sentinel))
            .collect();
        assert!(
            leaked.is_empty(),
            "SECRET LEAK: the {label} sentinel appeared in telemetry: {leaked:?}"
        );
    }
}
