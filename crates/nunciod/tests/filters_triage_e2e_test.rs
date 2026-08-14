//! Offline end-to-end test for the `nuncio.v1.Filters.Triage` streaming RPC.
//!
//! Boots a real `nunciod` daemon gRPC server (`nunciod::grpc::
//! serve_on_listener`) on an ephemeral loopback port, backed by a real
//! temp file-backed [`DatabaseEngine`] seeded directly via
//! [`DatabaseEngine::save_email_at`] (standing in for messages that were synced
//! before the active rule set existed) and a live [`FilterEngine`] carrying
//! a real persisted rule.
//!
//! Drives ONLY the authenticated `nuncio-proto` `FiltersClient::triage` call
//! against the live server, proving:
//!   a. the final cumulative `matched_count`/`actions_applied_count` are
//!      correct against a store containing both matching and non-matching
//!      messages;
//!   b. matching messages show the REAL side effects `apply_filter_actions`
//!      produces (the read flag flips for an immediate `MARK READ` action;
//!      an outbox row is queued for a remote `MOVE TO` action) -- not a
//!      fabricated count with no backing state change;
//!   c. a store bigger than one `chunk_size` genuinely streams more than one
//!      non-final `TriageProgress` message, proving this is a real
//!      multi-chunk stream rather than one batch dressed up as a stream.
//!
//! Fully offline and deterministic: no real network, no real OS keyring
//! (`SecretManager::mock`), no fixed sleeps.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::model::{Email, IdentitySource, Placement};
use nuncio_core::EventBus;
use nuncio_filter::{FilterEngine, NsqlParser};
use nuncio_proto::v1::filters_client::FiltersClient;
use nuncio_proto::v1::TriageRequest;
use nuncio_store::db::DatabaseEngine;
use nuncio_store::vault::{SecretManager, GRPC_TOKEN_ACCOUNT};
use nunciod::grpc::serve_on_listener;
use std::sync::Arc;
use tokio::net::TcpListener;

const ACCOUNT_ID: &str = "acct-triage-e2e-1";

fn sample_email(id: &str, subject: &str) -> Email {
    Email {
        id: id.to_string(),
        account_id: ACCOUNT_ID.to_string(),
        subject: subject.to_string(),
        sender: "alice@nuncio.mx".to_string(),
        recipient: "triage-e2e@nuncio.mx".to_string(),
        received_at: 1_700_000_000,
        body_plain: Some("body".to_string()),
        body_html: None,
        attachments: Vec::new(),
        message_id: None,
        content_hash: None,
    }
}

/// The account the seeded messages belong to, opted in as this daemon's
/// filter-execution owner.
fn owning_account() -> nuncio_core::AccountConfig {
    nuncio_core::AccountConfig {
        id: ACCOUNT_ID.to_string(),
        name: "Triage E2E".to_string(),
        email_address: "triage-e2e@nuncio.mx".to_string(),
        keyring_secret_key: format!("nuncio/{ACCOUNT_ID}"),
        sync_interval_secs: 60,
        filters_enabled: true,
        transport: nuncio_core::Transport::ImapSmtp(nuncio_core::ImapSmtpTransport {
            imap_host: "imap.nuncio.mx".to_string(),
            imap_port: 993,
            imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
            smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
        }),
    }
}

/// The INBOX occupancy a seeded message sits in. `id` doubles as the UID, so
/// distinct messages occupy distinct rows.
fn sample_placement(id: &str) -> Placement {
    Placement {
        account_id: ACCOUNT_ID.to_string(),
        folder_id: "inbox".to_string(),
        uid_validity: "1".to_string(),
        remote_id: id.to_string(),
        read: false,
    }
}

/// Seed a message and its occupancy the way a sync pass would.
async fn seed(db: &DatabaseEngine, id: &str, subject: &str) {
    db.save_email_at(
        &sample_email(id, subject),
        IdentitySource::Surrogate,
        &sample_placement(id),
    )
    .await
    .unwrap_or_else(|e| panic!("seed message '{id}': {e}"));
}

/// The read flag of a seeded message's single occupancy.
async fn read_flag_of(db: &DatabaseEngine, id: &str) -> bool {
    let placements = db.placements_of(id).await.expect("read placements");
    assert_eq!(
        placements.len(),
        1,
        "'{id}' should occupy exactly one folder"
    );
    placements[0].read
}

/// Boots the real daemon gRPC server (bearer-token authenticated) on an
/// ephemeral loopback port, backed by a real temp file-backed
/// `DatabaseEngine` and the given live `FilterEngine`. Returns the server
/// address, the minted token, the `DatabaseEngine` handle (used to seed
/// messages before triggering `Triage`), and the `TempDir` guard, which the
/// caller must hold for as long as it talks to the server.
async fn boot_daemon(
    secrets: Arc<SecretManager>,
    filter_engine: Arc<FilterEngine>,
) -> (
    std::net::SocketAddr,
    String,
    Arc<DatabaseEngine>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join("triage_e2e.db");
    let db = Arc::new(
        DatabaseEngine::connect_file(&db_path, &secrets)
            .await
            .expect("open temp file-backed database"),
    );

    // Triage only acts on accounts this daemon owns filter execution for, so
    // the account must exist and be opted in -- otherwise the RPC honestly
    // refuses rather than scanning and applying nothing.
    db.save_account(&owning_account())
        .await
        .expect("seed the triage account");

    let token_bytes = secrets
        .get_or_create_key_bytes(GRPC_TOKEN_ACCOUNT, 32)
        .expect("mint gRPC bearer token from mock vault");
    let token = hex::encode(token_bytes);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("listener has local addr");

    let event_bus = Arc::new(EventBus::new());
    let server_db = db.clone();
    let server_secrets = secrets.clone();
    let server_token = token.clone();
    tokio::spawn(async move {
        let _ = serve_on_listener(
            listener,
            event_bus,
            server_db,
            filter_engine,
            server_secrets,
            server_token,
        )
        .await;
    });

    (addr, token, db, dir)
}

fn authed_request<T>(token: &str, message: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid ascii metadata value"),
    );
    request
}

#[tokio::test]
async fn triage_applies_live_rule_retroactively_and_reports_real_cumulative_counts() {
    let secrets = Arc::new(SecretManager::mock());

    let mark_read_rule = NsqlParser::parse_rule(
        "Urgent Auto-Read",
        1,
        "WHERE subject CONTAINS 'Urgent' ACTION MARK READ",
    )
    .expect("parse mark-read rule");
    let move_rule = NsqlParser::parse_rule(
        "Archive Rule",
        2,
        "WHERE subject CONTAINS 'Archive Me' ACTION MOVE TO 'Archive'",
    )
    .expect("parse move rule");
    let filter_engine =
        Arc::new(FilterEngine::new(vec![mark_read_rule, move_rule]).expect("compile rules"));

    let (addr, token, db, _dir) = boot_daemon(secrets, filter_engine).await;

    // Seed messages directly into the store, standing in for mail that was
    // synced BEFORE these rules existed -- exactly the scenario `Triage` is
    // for. Two match (one per rule), one does not.
    seed(&db, "m-urgent", "Urgent: server down").await;
    seed(&db, "m-archive", "Please Archive Me").await;
    seed(&db, "m-plain", "Just saying hi").await;

    let mut client = FiltersClient::connect(format!("http://{addr}"))
        .await
        .expect("client connects");

    let mut stream = client
        .triage(authed_request(
            &token,
            TriageRequest {
                rule_id: None,
                chunk_size: 100,
            },
        ))
        .await
        .expect("triage call succeeds")
        .into_inner();

    let mut updates = Vec::new();
    while let Some(progress) = stream.message().await.expect("stream message") {
        updates.push(progress);
    }

    let last = updates.last().expect("at least one progress update");
    assert!(last.done, "final update must be marked done");
    assert_eq!(last.scanned_count, 3);
    assert_eq!(
        last.matched_count, 2,
        "exactly the two messages with a rule match count as matched"
    );
    assert_eq!(
        last.actions_applied_count, 2,
        "one action applied per matching message"
    );

    // Real side effects, not a fabricated count: the MARK READ action
    // actually flipped the stored read flag...
    assert!(
        read_flag_of(&db, "m-urgent").await,
        "MARK READ must genuinely flip the occupancy's read flag"
    );

    // ...and the MOVE TO action actually queued a real outbox mutation.
    let pending = db
        .list_pending_mutations(10)
        .await
        .expect("list pending mutations");
    assert_eq!(
        pending.len(),
        1,
        "the remote MOVE action must enqueue exactly one outbox mutation"
    );
    assert_eq!(pending[0].message_id, "m-archive");

    assert!(
        !read_flag_of(&db, "m-plain").await,
        "a non-matching message must be left untouched"
    );
}

#[tokio::test]
async fn triage_streams_multiple_chunks_for_a_store_bigger_than_one_chunk() {
    let secrets = Arc::new(SecretManager::mock());
    let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));

    let (addr, token, db, _dir) = boot_daemon(secrets, filter_engine).await;

    // Five messages against a chunk size of two forces three chunks
    // (2 + 2 + 1), so the stream must carry at least two non-final progress
    // updates before the final `done: true` -- proof this is a real
    // multi-chunk stream, not one batch disguised as one.
    for i in 0..5 {
        seed(&db, &format!("m-{i:02}"), &format!("Message number {i}")).await;
    }

    let mut client = FiltersClient::connect(format!("http://{addr}"))
        .await
        .expect("client connects");

    let mut stream = client
        .triage(authed_request(
            &token,
            TriageRequest {
                rule_id: None,
                chunk_size: 2,
            },
        ))
        .await
        .expect("triage call succeeds")
        .into_inner();

    let mut updates = Vec::new();
    while let Some(progress) = stream.message().await.expect("stream message") {
        updates.push(progress);
    }

    let non_final: Vec<_> = updates.iter().filter(|p| !p.done).collect();
    assert!(
        non_final.len() > 1,
        "expected more than one non-final progress update for a multi-chunk scan, got {}",
        non_final.len()
    );

    let last = updates.last().expect("at least one progress update");
    assert!(last.done);
    assert_eq!(last.scanned_count, 5);

    // Cumulative counts must monotonically increase across the stream --
    // never reset or double-count between chunks.
    let mut prev_scanned = 0u64;
    for update in &updates {
        assert!(update.scanned_count >= prev_scanned);
        prev_scanned = update.scanned_count;
    }
}

/// A daemon that owns filter execution for no account must refuse `Triage`
/// outright. Streaming a scan that reports progress and applies nothing would
/// be indistinguishable, from the caller's side, from "no rules matched" --
/// and the whole point of single-owner execution is that the non-owner stays
/// visibly inert rather than quietly so.
#[tokio::test]
async fn triage_refuses_when_this_daemon_owns_no_account() {
    let secrets = Arc::new(SecretManager::mock());
    let rule = NsqlParser::parse_rule(
        "Urgent Auto-Read",
        1,
        "WHERE subject CONTAINS 'Urgent' ACTION MARK READ",
    )
    .expect("parse rule");
    let filter_engine = Arc::new(FilterEngine::new(vec![rule]).expect("compile rule"));

    let (addr, token, db, _dir) = boot_daemon(secrets, filter_engine).await;

    // Take ownership away: the account exists, but this daemon is not its
    // filter owner.
    let mut account = owning_account();
    account.filters_enabled = false;
    db.save_account(&account)
        .await
        .expect("opt the account out");

    seed(&db, "m-1", "Urgent: server down").await;

    let mut client = FiltersClient::connect(format!("http://{addr}"))
        .await
        .expect("connect to the daemon");
    let status = client
        .triage(authed_request(&token, TriageRequest::default()))
        .await
        .expect_err("triage must refuse without an owning account");

    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert!(
        status.message().contains("filter execution"),
        "the refusal must say why, got: {}",
        status.message()
    );

    // And it must genuinely have applied nothing: the occupancy the rule
    // would have marked read is still unread.
    assert!(
        !read_flag_of(&db, "m-1").await,
        "a refused triage must not apply any action"
    );
}
