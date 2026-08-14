//! Offline end-to-end tests for the hand-driven mail mutation surface: the
//! placement-addressed `MoveMessage`/`DeleteMessage`/`FlagMessage` RPCs, the
//! durable conflict ledger behind `ListConflicts`/`ResolveConflict`, the
//! complete placement set on `Message`, and the per-account filter switch.
//!
//! Each test boots a real `nunciod` daemon gRPC server
//! (`nunciod::grpc::serve_on_listener_with_overrides`) on an ephemeral loopback
//! port, backed by a real temp file-backed [`DatabaseEngine`] and a
//! [`SecretManager::mock`] vault (never the real OS keyring), with every engine
//! override left at its default. The store handle is shared with the server, so
//! the assertions can seed state a sync pass would have written and read back
//! the durable rows the RPCs produced -- which is the only thing enqueuing can
//! truthfully promise, since nothing here reaches a remote server.
//!
//! Fully offline: no real network, no real OS keyring, no live server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::model::{Email, IdentitySource, MutationConflict, Placement, PlacementKey};
use nuncio_core::EventBus;
use nuncio_filter::{FilterEngine, MutationPayload};
use nuncio_proto::errors;
use nuncio_proto::v1::{
    AccountConfig, AddAccountRequest, DeleteMessageRequest, ErrorReason, FlagMessageRequest,
    GetMessageRequest, ListAccountsRequest, ListConflictsRequest, ListMessagesRequest,
    MoveMessageRequest, PlacementRef, ResolveConflictRequest, UpdateAccountRequest,
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

const ACCOUNT: &str = "acct-mutation-e2e";
/// One UIDVALIDITY generation for every seeded occupancy: these tests are about
/// which mailbox a mutation lands in, not about generation bumps.
const UIDVALIDITY: &str = "7";

/// Boots the real daemon gRPC server on an ephemeral loopback port with every
/// override at its default.
///
/// Returns the dialable address, the minted bearer token, and the SAME store
/// handle the server is using, so a test can seed rows and inspect what an RPC
/// durably wrote. The `TempDir` guard is returned so the caller controls its
/// lifetime for the whole test (the spawned server keeps using the db).
async fn boot_daemon(db_name: &str) -> (String, String, Arc<DatabaseEngine>, tempfile::TempDir) {
    let secrets = Arc::new(SecretManager::mock());
    let dir = tempfile::tempdir().expect("create temp dir for file-backed db");
    let db_path = dir.path().join(db_name);
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
    let server_db = db.clone();
    let server_token = token.clone();
    tokio::spawn(async move {
        let _ = serve_on_listener_with_overrides(
            listener,
            event_bus,
            server_db,
            filter_engine,
            secrets,
            server_token,
            MailEngineOverrides::default(),
            CalendarEngineOverrides::default(),
            ContactsEngineOverrides::default(),
            AccountsEngineOverrides::default(),
        )
        .await;
    });

    (addr.to_string(), token, db, dir)
}

/// One seeded mailbox occupancy of the test account.
fn placement(folder_id: &str, uid: &str) -> Placement {
    Placement {
        account_id: ACCOUNT.to_string(),
        folder_id: folder_id.to_string(),
        uid_validity: UIDVALIDITY.to_string(),
        remote_id: uid.to_string(),
        read: false,
    }
}

/// The wire form a client uses to name that same occupancy.
fn placement_ref(folder_id: &str, uid: &str) -> PlacementRef {
    PlacementRef {
        account_id: ACCOUNT.to_string(),
        folder_id: folder_id.to_string(),
        uidvalidity: UIDVALIDITY.to_string(),
        uid: uid.to_string(),
    }
}

/// The store-side key the queued outbox row is expected to carry.
fn placement_key(folder_id: &str, uid: &str) -> PlacementKey {
    placement(folder_id, uid).key()
}

/// The account row the read RPCs merge over. `Mail` reads carry no account
/// field, so mail belonging to no configured account is unreachable through
/// them -- seeding one keeps these tests on the production path.
async fn seed_account(db: &DatabaseEngine) {
    db.save_account(&nuncio_core::AccountConfig {
        id: ACCOUNT.to_string(),
        name: "Mutation E2E".to_string(),
        email_address: "mutation-e2e@nuncio.mx".to_string(),
        keyring_secret_key: format!("nuncio/{ACCOUNT}"),
        sync_interval_secs: 60,
        filters_enabled: false,
        transport: nuncio_core::Transport::Jmap(nuncio_core::JmapTransport {
            endpoint_host: "jmap.nuncio.mx".to_string(),
        }),
    })
    .await
    .expect("save the owning account");
}

/// Persist one message into each of the given occupancies, the way a sync pass
/// over several mailboxes would.
async fn seed_message(db: &DatabaseEngine, id: &str, placements: &[Placement]) {
    let email = Email {
        id: id.to_string(),
        account_id: ACCOUNT.to_string(),
        subject: format!("subject of {id}"),
        sender: "alice@nuncio.mx".to_string(),
        recipient: "owner@nuncio.mx".to_string(),
        received_at: 1_700_000_000,
        body_plain: Some(format!("body of {id}")),
        body_html: None,
        attachments: Vec::new(),
        message_id: None,
        content_hash: None,
    };
    for p in placements {
        db.save_email_at(&email, IdentitySource::Surrogate, p)
            .await
            .expect("seed the occupancy");
    }
}

/// The queued outbox row with this id, decoded into its operation and payload.
async fn queued_mutation(db: &DatabaseEngine, mutation_id: &str) -> (String, MutationPayload) {
    let pending = db
        .list_pending_mutations(50)
        .await
        .expect("read the outbox queue");
    let row = pending
        .into_iter()
        .find(|m| m.id == mutation_id)
        .unwrap_or_else(|| panic!("no queued outbox row '{mutation_id}'"));
    let payload =
        serde_json::from_str::<MutationPayload>(&row.payload).expect("decode the queued payload");
    assert_eq!(
        row.rule_id, "manual",
        "a hand-driven mutation is attributed to a person, not to a rule"
    );
    (row.mutation_type, payload)
}

/// A message in two mailboxes cannot be moved by naming only the message: one
/// of the copies would be picked for the caller, and a misaimed MOVE is
/// destructive with no undo. Naming the occupancy makes the same call succeed,
/// and the queued row records which copy the intent was formed against.
#[tokio::test]
async fn moving_a_multi_placement_message_requires_the_occupancy_to_be_named() {
    let (addr, token, db, _dir) = boot_daemon("mutation_move.db").await;
    seed_account(&db).await;
    seed_message(
        &db,
        "msg-multi",
        &[placement("INBOX", "101"), placement("Archive", "202")],
    )
    .await;

    let mut mail = nuncio_proto::client::connect_mail(&addr, &token)
        .await
        .expect("mail client connects");

    // ---- unnamed: refused, and the refusal names the candidates ----
    let status = mail
        .move_message(MoveMessageRequest {
            message_id: "msg-multi".to_string(),
            placement: None,
            destination_folder_id: "Trash".to_string(),
        })
        .await
        .expect_err("an ambiguously-addressed move must be refused");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(
        errors::error_reason(&status),
        Some(ErrorReason::ValidationFailed)
    );
    assert!(
        status.message().contains("INBOX") && status.message().contains("Archive"),
        "the refusal must name the candidate mailboxes: {}",
        status.message()
    );
    assert!(
        db.list_pending_mutations(50)
            .await
            .expect("read the outbox queue")
            .is_empty(),
        "a refused move must not leave a queued intent behind"
    );

    // ---- named: accepted, and durable ----
    let mutation_id = mail
        .move_message(MoveMessageRequest {
            message_id: "msg-multi".to_string(),
            placement: Some(placement_ref("INBOX", "101")),
            destination_folder_id: "Trash".to_string(),
        })
        .await
        .expect("naming the occupancy makes the move well-defined")
        .into_inner()
        .mutation_id;
    assert!(
        !mutation_id.is_empty(),
        "the caller needs an id to correlate the queued intent with a conflict"
    );

    let (mutation_type, payload) = queued_mutation(&db, &mutation_id).await;
    assert_eq!(mutation_type, "MOVE");
    assert_eq!(payload.action_type, "MOVE");
    assert_eq!(payload.target.as_deref(), Some("Trash"));
    assert_eq!(
        payload.placement,
        Some(placement_key("INBOX", "101")),
        "the queued row must remember the occupancy the intent was formed against"
    );
}

/// Delete, flag and unflag queue the operation they were asked for, addressed
/// at a specific occupancy: named explicitly when there are several to choose
/// between, and inferred only when the message occupies exactly one mailbox.
#[tokio::test]
async fn delete_and_flag_queue_placement_addressed_rows() {
    let (addr, token, db, _dir) = boot_daemon("mutation_delete_flag.db").await;
    seed_account(&db).await;
    seed_message(
        &db,
        "msg-multi",
        &[placement("INBOX", "101"), placement("Archive", "202")],
    )
    .await;
    seed_message(&db, "msg-solo", &[placement("INBOX", "303")]).await;

    let mut mail = nuncio_proto::client::connect_mail(&addr, &token)
        .await
        .expect("mail client connects");

    let delete_id = mail
        .delete_message(DeleteMessageRequest {
            message_id: "msg-multi".to_string(),
            placement: Some(placement_ref("Archive", "202")),
        })
        .await
        .expect("delete of a named occupancy is well-defined")
        .into_inner()
        .mutation_id;
    let (mutation_type, payload) = queued_mutation(&db, &delete_id).await;
    assert_eq!(mutation_type, "DELETE");
    assert_eq!(payload.action_type, "DELETE");
    assert_eq!(payload.target, None);
    assert_eq!(payload.placement, Some(placement_key("Archive", "202")));

    let flag_id = mail
        .flag_message(FlagMessageRequest {
            message_id: "msg-multi".to_string(),
            placement: Some(placement_ref("INBOX", "101")),
            flagged: true,
        })
        .await
        .expect("flag of a named occupancy is well-defined")
        .into_inner()
        .mutation_id;
    let (mutation_type, payload) = queued_mutation(&db, &flag_id).await;
    assert_eq!(mutation_type, "FLAG");
    assert_eq!(payload.action_type, "FLAG");
    assert_eq!(payload.placement, Some(placement_key("INBOX", "101")));

    // `flagged: false` is a distinct operation, not a FLAG with a parameter --
    // the executor maps the two onto opposite remote calls.
    let unflag_id = mail
        .flag_message(FlagMessageRequest {
            message_id: "msg-solo".to_string(),
            placement: None,
            flagged: false,
        })
        .await
        .expect("a single-occupancy message needs no placement to be unambiguous")
        .into_inner()
        .mutation_id;
    let (mutation_type, payload) = queued_mutation(&db, &unflag_id).await;
    assert_eq!(mutation_type, "UNFLAG");
    assert_eq!(payload.action_type, "UNFLAG");
    assert_eq!(
        payload.placement,
        Some(placement_key("INBOX", "303")),
        "the inferred occupancy is recorded too, so the executor never re-infers it"
    );
}

/// Naming an occupancy the message does not hold is a not-found error, never a
/// silent re-aim at whichever copy does exist: the caller formed its intent
/// against a mailbox that is no longer true, and acting on a different one
/// would touch mail it never selected.
#[tokio::test]
async fn naming_an_occupancy_the_message_does_not_hold_is_refused() {
    let (addr, token, db, _dir) = boot_daemon("mutation_stale_placement.db").await;
    seed_account(&db).await;
    seed_message(&db, "msg-solo", &[placement("INBOX", "303")]).await;

    let mut mail = nuncio_proto::client::connect_mail(&addr, &token)
        .await
        .expect("mail client connects");

    let status = mail
        .move_message(MoveMessageRequest {
            message_id: "msg-solo".to_string(),
            placement: Some(placement_ref("Spam", "999")),
            destination_folder_id: "Trash".to_string(),
        })
        .await
        .expect_err("a stale occupancy must not be honoured");
    assert_eq!(status.code(), Code::NotFound);
    assert_eq!(
        errors::error_reason(&status),
        Some(ErrorReason::MessageNotFound)
    );
    assert!(
        status.message().contains("Spam"),
        "the error must name the occupancy that was asked for: {}",
        status.message()
    );

    // A delete aimed at the wrong uid within a real folder is equally refused:
    // the uid is part of the address, not a hint.
    let status = mail
        .delete_message(DeleteMessageRequest {
            message_id: "msg-solo".to_string(),
            placement: Some(placement_ref("INBOX", "304")),
        })
        .await
        .expect_err("a wrong uid must not be rounded to the right one");
    assert_eq!(
        errors::error_reason(&status),
        Some(ErrorReason::MessageNotFound)
    );

    assert!(
        db.list_pending_mutations(50)
            .await
            .expect("read the outbox queue")
            .is_empty(),
        "a refused mutation must leave nothing queued"
    );
}

/// Both read RPCs report the message's complete occupancy set, folder id
/// ascending, while `folder_id` keeps its older meaning: the occupancy the
/// message was reached through. A client that only ever saw `folder_id` cannot
/// tell a one-mailbox message from a two-mailbox one.
#[tokio::test]
async fn read_rpcs_report_every_occupancy_of_a_message() {
    let (addr, token, db, _dir) = boot_daemon("mutation_placements_read.db").await;
    seed_account(&db).await;
    seed_message(
        &db,
        "msg-multi",
        &[placement("INBOX", "101"), placement("Archive", "202")],
    )
    .await;

    let mut mail = nuncio_proto::client::connect_mail(&addr, &token)
        .await
        .expect("mail client connects");

    let fetched = mail
        .get_message(GetMessageRequest {
            message_id: "msg-multi".to_string(),
        })
        .await
        .expect("get_message succeeds")
        .into_inner()
        .message
        .expect("a found message is always populated");
    let folders: Vec<&str> = fetched
        .placements
        .iter()
        .map(|p| p.folder_id.as_str())
        .collect();
    assert_eq!(folders, vec!["Archive", "INBOX"]);
    let uids: Vec<&str> = fetched.placements.iter().map(|p| p.uid.as_str()).collect();
    assert_eq!(uids, vec!["202", "101"]);
    assert!(fetched
        .placements
        .iter()
        .all(|p| p.account_id == ACCOUNT && p.uidvalidity == UIDVALIDITY));
    assert_eq!(
        fetched.folder_id, "Archive",
        "folder_id still names the single occupancy this read reached the message through"
    );

    // Reached through INBOX this time: the same complete set, a different
    // `folder_id`.
    let listed = mail
        .list_messages(ListMessagesRequest {
            folder_id: "INBOX".to_string(),
            page_size: 10,
            page_token: String::new(),
        })
        .await
        .expect("list_messages succeeds")
        .into_inner()
        .messages;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].folder_id, "INBOX");
    let folders: Vec<&str> = listed[0]
        .placements
        .iter()
        .map(|p| p.folder_id.as_str())
        .collect();
    assert_eq!(folders, vec!["Archive", "INBOX"]);
}

/// A conflict is durable state, not a passing notification: it survives in a
/// queryable table, resolving it is recorded once and only once, and resolved
/// rows stay as the audit trail of what was decided.
#[tokio::test]
async fn conflicts_round_trip_from_record_through_resolution() {
    let (addr, token, db, _dir) = boot_daemon("mutation_conflicts.db").await;
    seed_account(&db).await;
    seed_message(&db, "msg-solo", &[placement("INBOX", "303")]).await;

    db.record_conflict(&MutationConflict {
        id: "conflict-1".to_string(),
        mutation_id: "mut-conflicted".to_string(),
        message_id: "msg-solo".to_string(),
        mutation_type: "MOVE".to_string(),
        placement: Some(placement_key("INBOX", "303")),
        observed: "another client moved the message to Archive first".to_string(),
        detected_at: 1_700_000_500,
        resolved_at: None,
        resolution: None,
    })
    .await
    .expect("record the conflict");

    let mut mail = nuncio_proto::client::connect_mail(&addr, &token)
        .await
        .expect("mail client connects");

    let conflicts = mail
        .list_conflicts(ListConflictsRequest {
            include_resolved: false,
            limit: 10,
        })
        .await
        .expect("list_conflicts succeeds")
        .into_inner()
        .conflicts;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].id, "conflict-1");
    assert_eq!(conflicts[0].mutation_id, "mut-conflicted");
    assert_eq!(conflicts[0].mutation_type, "MOVE");
    assert_eq!(
        conflicts[0].observed,
        "another client moved the message to Archive first"
    );
    assert_eq!(
        conflicts[0].placement,
        Some(placement_ref("INBOX", "303")),
        "the conflict names the occupancy the refused mutation was aimed at"
    );
    assert_eq!(conflicts[0].resolved_at, None);

    mail.resolve_conflict(ResolveConflictRequest {
        conflict_id: "conflict-1".to_string(),
        resolution: "kept the other client's move".to_string(),
    })
    .await
    .expect("resolve_conflict succeeds");

    // Resolving it again is an honest error: reporting success would tell the
    // caller their decision was recorded when the earlier one already stands.
    let status = mail
        .resolve_conflict(ResolveConflictRequest {
            conflict_id: "conflict-1".to_string(),
            resolution: "changed my mind".to_string(),
        })
        .await
        .expect_err("a conflict can only be resolved once");
    assert_eq!(status.code(), Code::NotFound);
    assert_eq!(
        errors::error_reason(&status),
        Some(ErrorReason::MessageNotFound)
    );

    let unresolved = mail
        .list_conflicts(ListConflictsRequest {
            include_resolved: false,
            limit: 10,
        })
        .await
        .expect("list_conflicts succeeds")
        .into_inner()
        .conflicts;
    assert!(
        unresolved.is_empty(),
        "a handled conflict is no longer outstanding work"
    );

    let all = mail
        .list_conflicts(ListConflictsRequest {
            include_resolved: true,
            limit: 10,
        })
        .await
        .expect("list_conflicts succeeds")
        .into_inner()
        .conflicts;
    assert_eq!(all.len(), 1, "the audit trail survives the resolution");
    assert_eq!(
        all[0].resolution.as_deref(),
        Some("kept the other client's move")
    );
    assert!(
        all[0].resolved_at.is_some(),
        "a resolved conflict records when the decision was made"
    );
}

/// `filters_enabled` is settable over the wire rather than only by editing the
/// accounts row by hand. It still defaults to off, because filters take
/// irreversible remote actions and an account should not acquire them merely by
/// being added.
#[tokio::test]
async fn filters_enabled_round_trips_over_the_wire() {
    let (addr, token, _db, _dir) = boot_daemon("mutation_filters_flag.db").await;

    let mut accounts = nuncio_proto::client::connect_accounts(&addr, &token)
        .await
        .expect("accounts client connects");

    let config = AccountConfig {
        id: ACCOUNT.to_string(),
        name: "Mutation E2E".to_string(),
        email_address: "mutation-e2e@nuncio.mx".to_string(),
        keyring_secret_key: format!("nuncio/{ACCOUNT}"),
        sync_interval: Some(nuncio_proto::time::duration_from_secs(60)),
        filters_enabled: false,
        transport: Some(nuncio_proto::v1::account_config::Transport::Jmap(
            nuncio_proto::v1::JmapTransport {
                endpoint_host: "jmap.nuncio.mx".to_string(),
            },
        )),
    };
    accounts
        .add_account(AddAccountRequest {
            config: Some(config.clone()),
            password: "mutation-e2e-secret".to_string(),
        })
        .await
        .expect("add_account succeeds");

    let listed = accounts
        .list_accounts(ListAccountsRequest {})
        .await
        .expect("list_accounts succeeds")
        .into_inner()
        .accounts;
    assert_eq!(listed.len(), 1);
    assert!(
        !listed[0].filters_enabled,
        "an account does not get filters by being added, only by being told to"
    );

    accounts
        .update_account(UpdateAccountRequest {
            config: Some(AccountConfig {
                filters_enabled: true,
                ..config
            }),
            password: None,
        })
        .await
        .expect("update_account succeeds");

    let relisted = accounts
        .list_accounts(ListAccountsRequest {})
        .await
        .expect("list_accounts succeeds")
        .into_inner()
        .accounts;
    assert_eq!(relisted.len(), 1);
    assert!(
        relisted[0].filters_enabled,
        "the flip must be persisted and reported back, not silently discarded"
    );
}
