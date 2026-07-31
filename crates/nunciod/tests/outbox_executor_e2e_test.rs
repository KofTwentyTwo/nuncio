//! Offline end-to-end test for the outbox remote-mutation executor.
//!
//! Proves the full `sync -> filter -> outbox -> execute` vertical without any
//! live network: a matched filter rule enqueues a real
//! `PendingRemoteMutation`, and `execute_pending_mutations` drains it against
//! injected mocks -- a `MockMailBackend` that records each applied mutation, a
//! `MockMessageSender` for `FORWARD`, and a loopback-allowed webhook dispatcher
//! hitting a wiremock server. The safety-critical property under test is that a
//! mutation is marked `completed` ONLY when the real op genuinely succeeded: a
//! failing backend never records an op and never completes the item.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use async_trait::async_trait;
use nuncio_core::model::Email;
use nuncio_filter::{FilterEngine, NsqlParser, ValidationOptions, WebhookDispatcher, WebhookError};
use nuncio_mail::{
    MailBackend, MessageSender, MockMailBackend, MockMessageSender, RemoteMutationKind,
};
use nuncio_store::db::DatabaseEngine;
use nunciod::outbox::{execute_pending_mutations, RemoteExecutionEnv};
use nunciod::sync::apply_filter_actions;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCOUNT_ID: &str = "acct-outbox-e2e";
const FOLDER_ID: &str = "INBOX";
const CHECKPOINT: &str = "1:105";

/// Test [`RemoteExecutionEnv`] injecting recording mocks and a webhook
/// dispatcher whose SSRF egress policy is relaxed so it can reach a loopback
/// wiremock server (production keeps the secure default).
struct MockEnv {
    backend: MockMailBackend,
    sender: MockMessageSender,
    webhook: WebhookDispatcher,
}

impl MockEnv {
    fn new() -> Self {
        Self {
            backend: MockMailBackend::new(),
            sender: MockMessageSender::new(),
            webhook: WebhookDispatcher::new("test-signing-key"),
        }
    }
}

#[async_trait]
impl RemoteExecutionEnv for MockEnv {
    async fn mail_backend(
        &self,
        _account_id: &str,
    ) -> Result<Box<dyn MailBackend>, nunciod::outbox::OutboxExecuteError> {
        // Clones share the same Arc-backed recording storage, so the returned
        // boxed backend records into the `self.backend` the test inspects.
        Ok(Box::new(self.backend.clone()))
    }

    async fn message_sender(
        &self,
        _account_id: &str,
    ) -> Result<Box<dyn MessageSender>, nunciod::outbox::OutboxExecuteError> {
        Ok(Box::new(self.sender.clone()))
    }

    async fn dispatch_webhook(
        &self,
        url: &str,
        rule_id: &str,
        message_id: &str,
        subject: &str,
        sender: &str,
    ) -> Result<u16, WebhookError> {
        let opts = ValidationOptions {
            block_private_webhooks: false,
            ..Default::default()
        };
        self.webhook
            .dispatch_with_options(url, rule_id, message_id, subject, sender, &opts)
            .await
    }
}

fn sample_account() -> nuncio_core::AccountConfig {
    nuncio_core::AccountConfig {
        id: ACCOUNT_ID.to_string(),
        name: "Outbox E2E Account".to_string(),
        email_address: "owner@nuncio.mx".to_string(),
        protocol: nuncio_core::AccountProtocol::ImapSmtp,
        server_host: "imap.nuncio.mx".to_string(),
        server_port: 993,
        smtp_host: "smtp.nuncio.mx".to_string(),
        smtp_port: 465,
        use_tls: true,
        imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
        smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
        keyring_secret_key: format!("nuncio/{ACCOUNT_ID}"),
        sync_interval_secs: 60,
    }
}

fn sample_email(subject: &str) -> Email {
    Email {
        id: "imap-uid-42".to_string(),
        account_id: ACCOUNT_ID.to_string(),
        folder_id: FOLDER_ID.to_string(),
        subject: subject.to_string(),
        sender: "alice@nuncio.mx".to_string(),
        recipient: "owner@nuncio.mx".to_string(),
        received_at: 1_700_000_000,
        read: false,
        body_plain: Some("original body".to_string()),
        body_html: None,
        attachments: Vec::new(),
    }
}

/// Seed the account, source-folder checkpoint, and a matching message, then run
/// the given rule's actions through `apply_filter_actions` so the outbox is
/// populated exactly as a real sync would populate it.
async fn seed_and_enqueue(db: &DatabaseEngine, rule_nsql: &str, subject: &str) {
    db.save_account(&sample_account())
        .await
        .expect("save account");
    db.save_folder_sync_state(ACCOUNT_ID, FOLDER_ID, CHECKPOINT)
        .await
        .expect("save folder checkpoint");

    let email = sample_email(subject);
    db.save_email(&email).await.expect("save email");

    let rule = NsqlParser::parse_rule("Outbox Rule", 1, rule_nsql).expect("parse rule");
    let engine = FilterEngine::new(vec![rule]).expect("compile rule");
    apply_filter_actions(db, &engine, &email).await;

    let pending = db.list_pending_mutations(10).await.expect("list pending");
    assert_eq!(
        pending.len(),
        1,
        "the rule must enqueue exactly one mutation"
    );
}

#[tokio::test]
async fn move_rule_executes_against_backend_and_completes_with_uidvalidity_checkpoint() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    seed_and_enqueue(
        &db,
        "WHERE subject CONTAINS 'Archive' ACTION MOVE TO 'Archive'",
        "Please Archive",
    )
    .await;

    let env = MockEnv::new();
    let summary = execute_pending_mutations(&db, &env, 50).await;

    assert_eq!(summary.completed, 1);
    assert_eq!(summary.failed, 0);

    let applied = env.backend.applied_mutations();
    assert_eq!(applied.len(), 1, "the backend op must genuinely run");
    assert_eq!(applied[0].message_id, "imap-uid-42");
    assert_eq!(applied[0].folder_id, FOLDER_ID);
    // The source folder's stored checkpoint (carrying UIDVALIDITY) is threaded
    // through so the backend can enforce its guard.
    assert_eq!(applied[0].folder_checkpoint.as_deref(), Some(CHECKPOINT));
    assert_eq!(
        applied[0].kind,
        RemoteMutationKind::Move {
            to_folder: "Archive".to_string()
        }
    );

    // A completed mutation is no longer pending.
    assert!(db.list_pending_mutations(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn flag_rule_executes_as_set_flagged() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    seed_and_enqueue(
        &db,
        "WHERE subject CONTAINS 'Star' ACTION FLAG",
        "Star this",
    )
    .await;

    let env = MockEnv::new();
    let summary = execute_pending_mutations(&db, &env, 50).await;

    assert_eq!(summary.completed, 1);
    let applied = env.backend.applied_mutations();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        applied[0].kind,
        RemoteMutationKind::SetFlagged { value: true }
    );
}

#[tokio::test]
async fn delete_rule_executes_as_delete() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    seed_and_enqueue(
        &db,
        "WHERE subject CONTAINS 'Trash' ACTION DELETE",
        "Trash me",
    )
    .await;

    let env = MockEnv::new();
    let summary = execute_pending_mutations(&db, &env, 50).await;

    assert_eq!(summary.completed, 1);
    let applied = env.backend.applied_mutations();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].kind, RemoteMutationKind::Delete);
}

#[tokio::test]
async fn a_failing_backend_op_is_never_marked_completed() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    seed_and_enqueue(
        &db,
        "WHERE subject CONTAINS 'Archive' ACTION MOVE TO 'Archive'",
        "Please Archive",
    )
    .await;

    let env = MockEnv::new();
    env.backend.set_should_fail(true);
    let summary = execute_pending_mutations(&db, &env, 50).await;

    // The op failed: nothing recorded, nothing completed, and the item is kept
    // for retry (still pending, with a bumped retry count) -- never faked.
    assert_eq!(summary.completed, 0);
    assert_eq!(summary.retried, 1);
    assert!(env.backend.applied_mutations().is_empty());

    let pending = db.list_pending_mutations(10).await.unwrap();
    assert_eq!(pending.len(), 1, "a failed op must remain pending");
    assert_eq!(pending[0].retry_count, 1, "the retry count must be bumped");
}

#[tokio::test]
async fn forward_rule_sends_via_the_message_sender() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    seed_and_enqueue(
        &db,
        "WHERE subject CONTAINS 'Forward' ACTION FORWARD TO 'boss@nuncio.mx'",
        "Please Forward",
    )
    .await;

    let env = MockEnv::new();
    let summary = execute_pending_mutations(&db, &env, 50).await;

    assert_eq!(summary.completed, 1);
    let sent = env.sender.sent_messages();
    assert_eq!(sent.len(), 1, "the forward must genuinely be sent");
    assert_eq!(sent[0].to, "boss@nuncio.mx");
    // The From: is the forwarding account's own configured address.
    assert_eq!(sent[0].from, "owner@nuncio.mx");
    assert!(sent[0].subject.contains("Please Forward"));
}

#[tokio::test]
async fn webhook_rule_dispatches_a_signed_post_to_the_receiver() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hooks/nuncio"))
        .and(wiremock::matchers::header(
            "Content-Type",
            "application/json",
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&mock_server)
        .await;

    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    let url = format!("{}/hooks/nuncio", mock_server.uri());
    seed_and_enqueue(
        &db,
        &format!("WHERE subject CONTAINS 'Ping' ACTION CALL WEBHOOK '{url}'"),
        "Ping me",
    )
    .await;

    let env = MockEnv::new();
    let summary = execute_pending_mutations(&db, &env, 50).await;

    // Completion is genuine: wiremock's `.expect(1)` verifies the POST actually
    // arrived when the server drops.
    assert_eq!(summary.completed, 1);
    assert!(db.list_pending_mutations(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_mutation_for_a_vanished_message_fails_permanently() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    db.save_account(&sample_account())
        .await
        .expect("save account");

    // Enqueue a mutation whose target message was never persisted.
    let mutation = nuncio_filter::OutboxManager::create_mutation(
        "rule-x",
        "imap-uid-999",
        "MOVE",
        Some("Archive".to_string()),
    );
    db.save_pending_mutation(&mutation)
        .await
        .expect("save mutation");

    let env = MockEnv::new();
    let summary = execute_pending_mutations(&db, &env, 50).await;

    // Cannot act on a message that no longer exists -- fail honestly, never
    // fabricate success, and never touch the backend.
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.completed, 0);
    assert!(env.backend.applied_mutations().is_empty());
    assert!(
        db.list_pending_mutations(10).await.unwrap().is_empty(),
        "a permanently-failed mutation is no longer pending"
    );
}
