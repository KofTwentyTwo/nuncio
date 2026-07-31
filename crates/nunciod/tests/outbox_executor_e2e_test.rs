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
use nuncio_core::EventBus;
use nuncio_filter::{FilterEngine, NsqlParser, ValidationOptions, WebhookDispatcher, WebhookError};
use nuncio_mail::{
    MailBackend, MessageSender, MockMailBackend, MockMessageSender, RemoteMutationKind,
};
use nuncio_store::db::DatabaseEngine;
use nunciod::lifecycle::{ShutdownController, ShutdownSignal};
use nunciod::outbox::{execute_pending_mutations, RemoteExecutionEnv};
use nunciod::sync::apply_filter_actions;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A shutdown handle for tests that never trigger it. Holding the returned
/// `ShutdownController` alive for the duration of the test matters: dropping
/// it closes the underlying `watch` channel, which would make `wait()`
/// resolve immediately and be mistaken for a real shutdown request.
fn no_shutdown() -> (ShutdownController, ShutdownSignal) {
    ShutdownController::new(Arc::new(EventBus::new()))
}

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
        collection_url: String::new(),
    }
}

fn sample_email(subject: &str) -> Email {
    Email {
        id: "surrogate-inbox-42".to_string(),
        account_id: ACCOUNT_ID.to_string(),
        folder_id: FOLDER_ID.to_string(),
        remote_id: "42".to_string(),
        uid_validity: "1".to_string(),
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

/// Like [`seed_and_enqueue`], but for a message with its own surrogate id so
/// several calls in the same test enqueue mutations against DISTINCT
/// messages instead of colliding on the shared fixed id `sample_email`
/// otherwise defaults to.
async fn seed_and_enqueue_distinct(
    db: &DatabaseEngine,
    message_id: &str,
    rule_nsql: &str,
    subject: &str,
) {
    db.save_account(&sample_account())
        .await
        .expect("save account");
    db.save_folder_sync_state(ACCOUNT_ID, FOLDER_ID, CHECKPOINT)
        .await
        .expect("save folder checkpoint");

    // `remote_id`/`uid_validity` (not just `id`) are part of the store's
    // `messages` identity UNIQUE index -- leaving them at `sample_email`'s
    // shared default would make this INSERT OR REPLACE the OTHER seeded
    // message sharing that same remote identity out from under it.
    let mut email = sample_email(subject);
    email.id = message_id.to_string();
    email.remote_id = message_id.to_string();
    db.save_email(&email).await.expect("save email");

    let before = db
        .list_pending_mutations(100)
        .await
        .expect("list pending")
        .len();

    let rule = NsqlParser::parse_rule("Outbox Rule", 1, rule_nsql).expect("parse rule");
    let engine = FilterEngine::new(vec![rule]).expect("compile rule");
    apply_filter_actions(db, &engine, &email).await;

    let after = db
        .list_pending_mutations(100)
        .await
        .expect("list pending")
        .len();
    assert_eq!(
        after,
        before + 1,
        "the rule must enqueue exactly one new mutation"
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
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let summary = execute_pending_mutations(&db, &env, 50, &mut shutdown).await;

    assert_eq!(summary.completed, 1);
    assert_eq!(summary.failed, 0);

    let applied = env.backend.applied_mutations();
    assert_eq!(applied.len(), 1, "the backend op must genuinely run");
    // Addressing is recovered from the stored row's columns, not by parsing the
    // opaque surrogate id: the protocol-native UID and the UIDVALIDITY scope it
    // was captured under, so the backend can enforce its guard.
    assert_eq!(applied[0].message_id, "surrogate-inbox-42");
    assert_eq!(applied[0].remote_id, "42");
    assert_eq!(applied[0].folder_id, FOLDER_ID);
    assert_eq!(applied[0].uid_validity, "1");
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
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let summary = execute_pending_mutations(&db, &env, 50, &mut shutdown).await;

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
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let summary = execute_pending_mutations(&db, &env, 50, &mut shutdown).await;

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
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let summary = execute_pending_mutations(&db, &env, 50, &mut shutdown).await;

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
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let summary = execute_pending_mutations(&db, &env, 50, &mut shutdown).await;

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
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let summary = execute_pending_mutations(&db, &env, 50, &mut shutdown).await;

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
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let summary = execute_pending_mutations(&db, &env, 50, &mut shutdown).await;

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

/// A [`RemoteExecutionEnv`] whose `mail_backend` call hangs (a `tokio::time`
/// sleep far longer than any timeout under test, standing in for a stuck
/// IMAP/JMAP op) for the first `hang_calls` invocations, then delegates to a
/// normal [`MockEnv`]. Used to prove the per-item execution timeout fires.
struct HangEnv {
    inner: MockEnv,
    calls: AtomicUsize,
    hang_calls: usize,
}

impl HangEnv {
    fn new(hang_calls: usize) -> Self {
        Self {
            inner: MockEnv::new(),
            calls: AtomicUsize::new(0),
            hang_calls,
        }
    }
}

#[async_trait]
impl RemoteExecutionEnv for HangEnv {
    async fn mail_backend(
        &self,
        account_id: &str,
    ) -> Result<Box<dyn MailBackend>, nunciod::outbox::OutboxExecuteError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call < self.hang_calls {
            // A `tokio::time`-tracked sleep far longer than any per-item
            // timeout under test -- NOT `std::future::pending`, which never
            // registers with the time driver and so behaves unpredictably
            // once `tokio::time::pause`'s auto-advance-on-idle interacts with
            // real (non-timer) I/O elsewhere in the same test, like the
            // SQLite pool's own connection bookkeeping. A real (virtual)
            // sleep is what the timeout under test is meant to race against.
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
        self.inner.mail_backend(account_id).await
    }

    async fn message_sender(
        &self,
        account_id: &str,
    ) -> Result<Box<dyn MessageSender>, nunciod::outbox::OutboxExecuteError> {
        self.inner.message_sender(account_id).await
    }

    async fn dispatch_webhook(
        &self,
        url: &str,
        rule_id: &str,
        message_id: &str,
        subject: &str,
        sender: &str,
    ) -> Result<u16, WebhookError> {
        self.inner
            .dispatch_webhook(url, rule_id, message_id, subject, sender)
            .await
    }
}

/// Seed two mutations so the second proves the drain pass actually moves on
/// after the first times out, rather than the whole pass having stalled.
async fn seed_two_move_mutations(db: &DatabaseEngine) {
    seed_and_enqueue_distinct(
        db,
        "surrogate-hang-1",
        "WHERE subject CONTAINS 'Archive' ACTION MOVE TO 'Archive'",
        "Please Archive this one",
    )
    .await;
    seed_and_enqueue_distinct(
        db,
        "surrogate-hang-2",
        "WHERE subject CONTAINS 'Trash' ACTION DELETE",
        "Please Trash this one",
    )
    .await;
}

#[tokio::test]
async fn a_hung_mutation_times_out_and_is_retried_never_completed() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    seed_and_enqueue(
        &db,
        "WHERE subject CONTAINS 'Archive' ACTION MOVE TO 'Archive'",
        "Please Archive",
    )
    .await;

    // Paused only AFTER setup, and this pass touches the database exactly
    // ONCE (a single `get_message`) before the hang -- mixing
    // `tokio::time::pause`'s auto-advance-on-idle with real (non-timer)
    // background I/O is only safe when there is a single, already-in-flight
    // real operation for the auto-advance to race past; see the sibling
    // two-pass test below for why a SECOND real DB operation in the same
    // paused pass is deliberately avoided.
    tokio::time::pause();
    let env = HangEnv::new(usize::MAX);
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let summary = execute_pending_mutations(&db, &env, 10, &mut shutdown).await;
    // Resume real time before any further DB access: `tokio::time::pause`'s
    // auto-advance-on-idle only plays safely with the ONE real DB operation
    // this pass performs (the `get_message` before the hang); a second real
    // query made while still paused is what made the sibling two-pass test
    // flaky before it was split across a resume point.
    tokio::time::resume();

    // The hung item is NEVER marked completed -- it is a retryable timeout,
    // exactly like any other transient remote failure.
    assert_eq!(summary.completed, 0);
    assert_eq!(
        summary.retried, 1,
        "the hung item must be retried, not lost"
    );
    assert_eq!(summary.failed, 0);

    let pending = db.list_pending_mutations(10).await.unwrap();
    assert_eq!(pending.len(), 1, "the hung item stays pending for a retry");
    assert_eq!(
        pending[0].retry_count, 1,
        "the timeout bumps the retry count exactly like any transient failure"
    );
    assert!(
        env.inner.backend.applied_mutations().is_empty(),
        "the hung item never genuinely reached the backend"
    );
}

#[tokio::test]
async fn a_timed_out_item_does_not_block_a_later_item_in_a_subsequent_pass() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    seed_two_move_mutations(&db).await;

    // First pass: bound to the FIRST (created-earliest) mutation only, so
    // this pass's real DB work is a single `get_message` call before the
    // hang, same as the sibling single-item timeout test. Real threaded
    // SQLite I/O racing `tokio::time::pause`'s auto-advance is only
    // exercised once per paused pass here -- see that test's comment for
    // why a second real DB call in the SAME paused pass is unreliable.
    tokio::time::pause();
    let env = HangEnv::new(usize::MAX);
    let (_shutdown_ctrl, mut shutdown) = no_shutdown();
    let first_pass = execute_pending_mutations(&db, &env, 1, &mut shutdown).await;
    assert_eq!(first_pass.completed, 0);
    assert_eq!(first_pass.retried, 1, "the hung item must be retried");

    // Second pass runs under ordinary real time against a normal
    // (non-hanging) env, standing in for the outbox worker's next 5-second
    // tick: the item that timed out in the first pass did not jam the queue
    // -- both it (now against a healthy backend) and the item queued behind
    // it are processed and genuinely complete.
    tokio::time::resume();
    let normal_env = MockEnv::new();
    let (_shutdown_ctrl2, mut shutdown2) = no_shutdown();
    let second_pass = execute_pending_mutations(&db, &normal_env, 10, &mut shutdown2).await;
    assert_eq!(
        second_pass.completed, 2,
        "both the previously-timed-out item and the one behind it complete"
    );
    assert_eq!(second_pass.failed, 0);

    let applied = normal_env.backend.applied_mutations();
    assert_eq!(
        applied.len(),
        2,
        "both mutations genuinely reached the backend"
    );

    // Neither pass ever fabricated a completion: the queue is now empty
    // because both items genuinely succeeded, not because either was
    // dropped or silently marked done while still hung.
    assert!(db.list_pending_mutations(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn shutdown_signal_interrupts_a_hung_item_instead_of_waiting_out_the_timeout() {
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.expect("db");
    seed_and_enqueue(
        &db,
        "WHERE subject CONTAINS 'Archive' ACTION MOVE TO 'Archive'",
        "Please Archive",
    )
    .await;

    // Paused only after setup -- see the comment on the sibling timeout test
    // for why pausing from the start would spuriously fail the DB pool.
    tokio::time::pause();

    // Hangs on every call -- the shutdown signal, not the timeout, must be
    // what unblocks this drain pass.
    let env = HangEnv::new(usize::MAX);
    let (controller, mut shutdown) = ShutdownController::new(Arc::new(EventBus::new()));
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        controller.trigger();
    });

    let summary = execute_pending_mutations(&db, &env, 10, &mut shutdown).await;
    tokio::time::resume();

    // The drain pass stopped on shutdown, well before the per-item timeout
    // would have elapsed -- the hung item is untouched: not completed, not
    // even retried, since its true disposition is still unknown.
    assert_eq!(summary.completed, 0);
    assert_eq!(summary.retried, 0);
    assert_eq!(summary.failed, 0);

    let pending = db.list_pending_mutations(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].retry_count, 0,
        "shutdown must leave the interrupted item's disposition untouched"
    );
}
