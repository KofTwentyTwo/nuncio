//! WireMock integration test suite for JMAP (RFC 8620 / 8621) protocol engine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::model::{IdentitySource, PlacementKey};
use nuncio_mail::{JmapEngine, MailBackend, RemoteMutationKind, RemoteMutationSpec};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn wiremock_jmap_session_discovery_and_email_get_sync() {
    let mock_server = MockServer::start().await;

    // 1. Mock JMAP Session Discovery Endpoint (/.well-known/jmap)
    let session_body = json!({
        "username": "james.maes@kof22.com",
        "primaryAccounts": {
            "urn:ietf:params:jmap:mail": "acct-100"
        },
        "apiUrl": format!("{}/jmap/api", mock_server.uri()),
        "downloadUrl": format!("{}/jmap/download", mock_server.uri()),
        "uploadUrl": format!("{}/jmap/upload", mock_server.uri()),
        "state": "session-state-1"
    });

    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&session_body))
        .mount(&mock_server)
        .await;

    // Execute real HTTP GET request against WireMock server for JMAP session discovery
    let session_url = JmapEngine::build_session_url(&mock_server.uri());
    let resp = reqwest::get(&session_url).await.expect("http request");
    let session = JmapEngine::parse_session(&resp.text().await.unwrap()).expect("parse session");

    assert_eq!(session.username, "james.maes@kof22.com");
    assert_eq!(session.state, "session-state-1");

    // 2. Mock JMAP Email/get Sync Endpoint (/jmap/api)
    let email_get_body = json!({
        "methodResponses": [
            [
                "Email/get",
                {
                    "accountId": "acct-100",
                    "state": "sync-state-500",
                    "list": [
                        {
                            "id": "msg-wm-100",
                            "subject": "WireMock JMAP E2E Test",
                            "from": [{ "email": "sender@nuncio.mx" }],
                            "to": [{ "email": "james.maes@kof22.com" }],
                            "receivedAt": 1700001000i64,
                            "isUnread": true,
                            "bodySnippet": "Deterministic JMAP protocol response over WireMock."
                        }
                    ]
                },
                "c1"
            ]
        ]
    });

    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&email_get_body))
        .mount(&mock_server)
        .await;

    // Execute real HTTP POST JMAP Email/get against WireMock server
    let client = reqwest::Client::new();
    let jmap_req = JmapEngine::build_email_get_request("acct-100", None);
    let resp = client
        .post(&session.api_url)
        .json(&jmap_req)
        .send()
        .await
        .expect("http post");

    let engine = JmapEngine::new("acct-100");
    let (emails, new_state) = engine
        .parse_email_get_response(&resp.text().await.unwrap())
        .expect("parse email get");

    assert_eq!(emails.len(), 1);
    // The JMAP object id is preserved as the placement's protocol-native
    // `remote_id`, while the message's own id is an opaque 64-char derived key
    // (never the raw id). The object id is account-stable, so it enters the
    // precedence at the EMAILID tier rather than as a folder-scoped surrogate.
    assert_eq!(emails[0].placement.remote_id, "msg-wm-100");
    assert_eq!(emails[0].source, IdentitySource::EmailId);
    assert_ne!(emails[0].email.id, "msg-wm-100");
    assert_eq!(emails[0].email.id.len(), 64);
    assert_eq!(emails[0].email.subject, "WireMock JMAP E2E Test");
    assert_eq!(emails[0].email.sender, "sender@nuncio.mx");
    assert_eq!(new_state, "sync-state-500");
}

/// Drives `JmapEngine` through the `MailBackend` trait itself (rather than its static parse
/// helpers) against a wiremock server, proving `with_credentials` performs genuine session
/// discovery, `Mailbox/get`, `Email/query`, and `Email/get` HTTP round trips end to end -- the
/// exact seam `nunciod::sync::build_mail_backend` wires into production.
#[tokio::test]
async fn jmap_engine_sync_folders_and_messages_through_mail_backend_trait() {
    let mock_server = MockServer::start().await;

    let session_body = json!({
        "username": "james.maes@kof22.com",
        "primaryAccounts": {
            "urn:ietf:params:jmap:mail": "acct-100"
        },
        "apiUrl": format!("{}/jmap/api", mock_server.uri()),
        "state": "session-state-1"
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&session_body))
        .mount(&mock_server)
        .await;

    let mailbox_get_body = json!({
        "methodResponses": [
            [
                "Mailbox/get",
                {
                    "accountId": "acct-100",
                    "list": [
                        {"id": "mb-inbox", "name": "Inbox", "totalEmails": 3, "unreadEmails": 1}
                    ]
                },
                "c1"
            ]
        ]
    });
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(wiremock::matchers::body_partial_json(json!({
            "methodCalls": [["Mailbox/get", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&mailbox_get_body))
        .mount(&mock_server)
        .await;

    let email_query_body = json!({
        "methodResponses": [
            ["Email/query", {"ids": ["msg-wm-200"]}, "c1"]
        ]
    });
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(wiremock::matchers::body_partial_json(json!({
            "methodCalls": [["Email/query", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&email_query_body))
        .mount(&mock_server)
        .await;

    let email_get_body = json!({
        "methodResponses": [
            [
                "Email/get",
                {
                    "accountId": "acct-100",
                    "state": "sync-state-900",
                    "list": [
                        {
                            "id": "msg-wm-200",
                            "subject": "Trait-Level JMAP Sync",
                            "from": [{ "email": "sender@nuncio.mx" }],
                            "to": [{ "email": "james.maes@kof22.com" }],
                            "receivedAt": 1700002000i64,
                            "isUnread": true,
                            "bodySnippet": "Fetched through the MailBackend trait."
                        }
                    ]
                },
                "c1"
            ]
        ]
    });
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(wiremock::matchers::body_partial_json(json!({
            "methodCalls": [["Email/get", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(&email_get_body))
        .mount(&mock_server)
        .await;

    let engine = JmapEngine::with_credentials(
        "acct-100",
        &mock_server.uri(),
        "james.maes@kof22.com",
        "wiremock-password",
    );

    let folders = engine.sync_folders().await.expect("sync_folders succeeds");
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].id, "mb-inbox");
    assert_eq!(folders[0].name, "Inbox");
    assert_eq!(folders[0].total_messages, 3);
    assert_eq!(folders[0].unread_messages, 1);

    let (emails, new_state) = engine
        .sync_messages("mb-inbox", None)
        .await
        .expect("sync_messages succeeds");
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0].placement.remote_id, "msg-wm-200");
    // The occupancy is labelled with the mailbox that was actually synced.
    // Removals are addressed by the same key, so a placement stored under any
    // other folder id could never be reconciled away again.
    assert_eq!(emails[0].placement.folder_id, "mb-inbox");
    assert_eq!(emails[0].source, IdentitySource::EmailId);
    assert_ne!(emails[0].email.id, "msg-wm-200");
    assert_eq!(emails[0].email.id.len(), 64);
    assert_eq!(emails[0].email.subject, "Trait-Level JMAP Sync");
    assert_eq!(emails[0].email.sender, "sender@nuncio.mx");
    assert_eq!(emails[0].email.received_at, 1700002000);
    // Read state is a property of the occupancy, not of the message.
    assert!(!emails[0].placement.read);
    assert_eq!(
        emails[0].email.body_plain,
        Some("Fetched through the MailBackend trait.".to_string())
    );
    // The checkpoint is scheme-tagged, so a later build can tell what the
    // opaque server token inside it actually means.
    assert_eq!(new_state, "v1:jmapstate:sync-state-900");
}

/// A session-discovery failure (non-2xx status) must surface as a genuine `MailError`, never a
/// silent fallback to canned data, once credentials are present.
#[tokio::test]
async fn jmap_engine_sync_folders_surfaces_session_discovery_failure() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let engine = JmapEngine::with_credentials(
        "acct-100",
        &mock_server.uri(),
        "james.maes@kof22.com",
        "wiremock-password",
    );

    let err = engine
        .sync_folders()
        .await
        .expect_err("session discovery failure must not fall back to canned folders");
    assert!(err.to_string().contains("network I/O error"));
}

/// Mount JMAP session discovery so `apply_mutation` can reach the mock `/jmap/api`.
async fn mount_session(mock_server: &MockServer) {
    let session_body = json!({
        "username": "james.maes@kof22.com",
        "primaryAccounts": { "urn:ietf:params:jmap:mail": "acct-100" },
        "apiUrl": format!("{}/jmap/api", mock_server.uri()),
        "state": "session-state-1"
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/jmap"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&session_body))
        .mount(mock_server)
        .await;
}

fn jmap_engine(mock_server: &MockServer) -> JmapEngine {
    JmapEngine::with_credentials(
        "acct-100",
        &mock_server.uri(),
        "james.maes@kof22.com",
        "wiremock-password",
    )
}

fn spec(kind: RemoteMutationKind) -> RemoteMutationSpec {
    RemoteMutationSpec {
        message_id: "surrogate-jmap-msg-1".to_string(),
        remote_id: "jmap-msg-1".to_string(),
        folder_id: "mb-inbox".to_string(),
        uid_validity: "jmap".to_string(),
        kind,
    }
}

#[tokio::test]
async fn jmap_apply_mutation_flag_issues_keywords_patch_and_confirms_success() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;

    // Assert the exact Email/set update patch reaches the server: a $flagged
    // keyword set to true on the target id.
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(json!({
            "methodCalls": [[
                "Email/set",
                { "update": { "jmap-msg-1": { "keywords/$flagged": true } } },
                "c1"
            ]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/set", {"updated": {"jmap-msg-1": null}}, "c1"]]
        })))
        .mount(&mock_server)
        .await;

    let engine = jmap_engine(&mock_server);
    engine
        .apply_mutation(&spec(RemoteMutationKind::SetFlagged { value: true }))
        .await
        .expect("flag mutation applied");
}

#[tokio::test]
async fn jmap_apply_mutation_move_replaces_mailbox_ids() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(json!({
            "methodCalls": [[
                "Email/set",
                { "update": { "jmap-msg-1": { "mailboxIds": { "mb-archive": true } } } },
                "c1"
            ]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/set", {"updated": {"jmap-msg-1": null}}, "c1"]]
        })))
        .mount(&mock_server)
        .await;

    let engine = jmap_engine(&mock_server);
    engine
        .apply_mutation(&spec(RemoteMutationKind::Move {
            to_folder: "mb-archive".to_string(),
        }))
        .await
        .expect("move mutation applied");
}

#[tokio::test]
async fn jmap_apply_mutation_delete_issues_destroy_and_confirms_success() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(json!({
            "methodCalls": [["Email/set", { "destroy": ["jmap-msg-1"] }, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/set", {"destroyed": ["jmap-msg-1"]}, "c1"]]
        })))
        .mount(&mock_server)
        .await;

    let engine = jmap_engine(&mock_server);
    engine
        .apply_mutation(&spec(RemoteMutationKind::Delete))
        .await
        .expect("delete mutation applied");
}

/// A server that reports the id under `notUpdated` genuinely rejected the
/// mutation; `apply_mutation` MUST surface that as an error so the outbox never
/// marks a rejected op completed.
#[tokio::test]
async fn jmap_apply_mutation_surfaces_server_rejection_as_error() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/set", {
                "updated": {},
                "notUpdated": { "jmap-msg-1": { "type": "notFound" } }
            }, "c1"]]
        })))
        .mount(&mock_server)
        .await;

    let engine = jmap_engine(&mock_server);
    let err = engine
        .apply_mutation(&spec(RemoteMutationKind::SetFlagged { value: false }))
        .await
        .expect_err("a server-rejected mutation must not be reported as success");
    assert!(err.to_string().contains("jmap-msg-1"));
}

// --- Email/changes differential sync (RFC 8621 s4.2 over RFC 8620 s5.2) ---

/// Mount one `Email/changes` page, matched on the exact `sinceState` the engine
/// must send. A pass that resumes from the wrong token matches no mock and the
/// request fails, so the matcher is itself the round-trip assertion.
async fn mount_email_changes_page(
    mock_server: &MockServer,
    since_state: &str,
    body: serde_json::Value,
) {
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(json!({
            "methodCalls": [["Email/changes", { "sinceState": since_state }, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/changes", body, "c1"]]
        })))
        .mount(mock_server)
        .await;
}

/// Mount an `Email/get` answering with `list` verbatim and the given state.
async fn mount_email_get(mock_server: &MockServer, state: &str, list: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(json!({
            "methodCalls": [["Email/get", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/get", {"state": state, "list": list}, "c1"]]
        })))
        .mount(mock_server)
        .await;
}

fn message(id: &str, mailbox_id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "subject": format!("Message {id}"),
        "from": [{ "email": "sender@nuncio.mx" }],
        "to": [{ "email": "james.maes@kof22.com" }],
        "receivedAt": 1700003000i64,
        "isUnread": true,
        "mailboxIds": { mailbox_id: true }
    })
}

fn placement_key(folder_id: &str, remote_id: &str) -> PlacementKey {
    PlacementKey {
        account_id: "acct-100".to_string(),
        folder_id: folder_id.to_string(),
        uid_validity: "jmap".to_string(),
        remote_id: remote_id.to_string(),
    }
}

/// The whole point of the differential pass: ids the server reports as
/// `destroyed` come back as placement removals, addressed exactly as the
/// occupancies this engine stores are -- so the sync path deletes those
/// placements and nothing else. An incremental pass still reports
/// `present: None`, because it never enumerated the folder.
#[tokio::test]
async fn jmap_sync_changes_turns_destroyed_ids_into_placement_removals() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;
    mount_email_changes_page(
        &mock_server,
        "srv-state-1",
        json!({
            "oldState": "srv-state-1",
            "newState": "srv-state-2",
            "created": ["m-new"],
            "updated": [],
            "destroyed": ["m-gone-1", "m-gone-2"],
            "hasMoreChanges": false
        }),
    )
    .await;
    mount_email_get(
        &mock_server,
        "srv-state-2",
        json!([message("m-new", "mb-inbox")]),
    )
    .await;

    let engine = jmap_engine(&mock_server);
    let changes = engine
        .sync_changes("mb-inbox", Some("v1:jmapstate:srv-state-1"))
        .await
        .expect("incremental sync succeeds");

    assert_eq!(changes.upserts.len(), 1);
    assert_eq!(changes.upserts[0].placement.remote_id, "m-new");
    assert_eq!(changes.upserts[0].placement.folder_id, "mb-inbox");
    assert_eq!(changes.upserts[0].source, IdentitySource::EmailId);

    // Exactly the destroyed ids, keyed the same way the stored placement is.
    assert_eq!(
        changes.removals,
        vec![
            placement_key("mb-inbox", "m-gone-1"),
            placement_key("mb-inbox", "m-gone-2"),
        ]
    );
    // A message the pass placed must never also be reconciled away.
    assert!(!changes.removals.iter().any(|key| key.remote_id == "m-new"));

    // An incremental pass cannot speak to absence beyond what it was told.
    assert!(
        changes.present.is_none(),
        "an incremental pass must not claim to have enumerated the folder"
    );
    assert_eq!(changes.next_state, "v1:jmapstate:srv-state-2");
}

/// `hasMoreChanges: true` means the server truncated the page. Stopping there
/// silently drops every change past the first page, so the pass must follow the
/// intermediate `newState` until the server stops paging, and accumulate.
#[tokio::test]
async fn jmap_sync_changes_follows_has_more_changes_to_convergence() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;
    mount_email_changes_page(
        &mock_server,
        "page-1",
        json!({
            "oldState": "page-1",
            "newState": "page-2",
            "created": ["m-page1"],
            "destroyed": ["m-gone-page1"],
            "hasMoreChanges": true
        }),
    )
    .await;
    mount_email_changes_page(
        &mock_server,
        "page-2",
        json!({
            "oldState": "page-2",
            "newState": "page-3",
            "updated": ["m-page2"],
            "destroyed": ["m-gone-page2"],
            "hasMoreChanges": false
        }),
    )
    .await;
    mount_email_get(
        &mock_server,
        "page-3",
        json!([
            message("m-page1", "mb-inbox"),
            message("m-page2", "mb-inbox")
        ]),
    )
    .await;

    let engine = jmap_engine(&mock_server);
    let changes = engine
        .sync_changes("mb-inbox", Some("v1:jmapstate:page-1"))
        .await
        .expect("paged incremental sync succeeds");

    let upserted: Vec<&str> = changes
        .upserts
        .iter()
        .map(|m| m.placement.remote_id.as_str())
        .collect();
    assert_eq!(
        upserted,
        vec!["m-page1", "m-page2"],
        "both pages' changed ids must be fetched, not just the first"
    );
    assert_eq!(
        changes.removals,
        vec![
            placement_key("mb-inbox", "m-gone-page1"),
            placement_key("mb-inbox", "m-gone-page2"),
        ],
        "removals must accumulate across pages"
    );
    // The last page's state is the resume point; an intermediate one would
    // replay the tail of the change log forever.
    assert_eq!(changes.next_state, "v1:jmapstate:page-3");
}

/// `cannotCalculateChanges` (RFC 8620 s5.2) is the server declining to compute
/// a delta. The pass must re-enumerate the folder rather than fail -- and only
/// then may it report `present`, because only then has it genuinely seen
/// everything the folder holds.
#[tokio::test]
async fn jmap_sync_changes_falls_back_to_full_enumeration_and_only_then_reports_presence() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(json!({
            "methodCalls": [["Email/changes", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["error", {"type": "cannotCalculateChanges"}, "c1"]]
        })))
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(json!({
            "methodCalls": [["Email/query", {"filter": {"inMailboxes": ["mb-inbox"]}}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/query", {"ids": ["m-a", "m-b"]}, "c1"]]
        })))
        .mount(&mock_server)
        .await;

    mount_email_get(
        &mock_server,
        "srv-full-9",
        json!([message("m-a", "mb-inbox"), message("m-b", "mb-inbox")]),
    )
    .await;

    let engine = jmap_engine(&mock_server);
    let changes = engine
        .sync_changes("mb-inbox", Some("v1:jmapstate:far-too-old"))
        .await
        .expect("a stale state must re-enumerate, not fail the sync");

    assert_eq!(changes.upserts.len(), 2);
    assert_eq!(
        changes.present,
        Some(vec![
            placement_key("mb-inbox", "m-a"),
            placement_key("mb-inbox", "m-b"),
        ]),
        "a genuine full enumeration is the one pass entitled to report presence"
    );
    assert!(
        changes.removals.is_empty(),
        "a full pass expresses absence through `present`, not through removals"
    );
    assert_eq!(changes.next_state, "v1:jmapstate:srv-full-9");
}

/// The `newState` a pass returns is the token the next pass must resume from.
/// The second pass's `Email/changes` mock only matches that exact state, so a
/// token that failed to round-trip through `next_state` would match nothing.
#[tokio::test]
async fn jmap_sync_changes_resumes_from_the_state_the_previous_pass_returned() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;
    mount_email_changes_page(
        &mock_server,
        "round-1",
        json!({
            "oldState": "round-1",
            "newState": "round-2",
            "destroyed": ["m-gone"],
            "hasMoreChanges": false
        }),
    )
    .await;
    mount_email_changes_page(
        &mock_server,
        "round-2",
        json!({
            "oldState": "round-2",
            "newState": "round-3",
            "destroyed": ["m-gone-later"],
            "hasMoreChanges": false
        }),
    )
    .await;

    let engine = jmap_engine(&mock_server);
    let first = engine
        .sync_changes("mb-inbox", Some("v1:jmapstate:round-1"))
        .await
        .expect("first pass succeeds");
    assert_eq!(first.next_state, "v1:jmapstate:round-2");

    let second = engine
        .sync_changes("mb-inbox", Some(&first.next_state))
        .await
        .expect("the returned checkpoint must be accepted verbatim on the next pass");
    assert_eq!(
        second.removals,
        vec![placement_key("mb-inbox", "m-gone-later")]
    );
    assert_eq!(second.next_state, "v1:jmapstate:round-3");
}

/// A checkpoint carrying no scheme this build recognises -- a bare token
/// written before tagging existed -- must never be handed to the server as a
/// `sinceState`. It resolves to a full enumeration instead: over-fetching is
/// recoverable, resuming from a token the server reads differently is not.
#[tokio::test]
async fn jmap_sync_changes_re_enumerates_when_the_stored_checkpoint_is_untagged() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;

    // No `Email/changes` mock is mounted on purpose: such a call would 404 and
    // fail the sync, which is what proves none was attempted.
    Mock::given(method("POST"))
        .and(path("/jmap/api"))
        .and(body_partial_json(json!({
            "methodCalls": [["Email/query", {}, "c1"]]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methodResponses": [["Email/query", {"ids": ["m-a"]}, "c1"]]
        })))
        .mount(&mock_server)
        .await;
    mount_email_get(
        &mock_server,
        "srv-full-1",
        json!([message("m-a", "mb-inbox")]),
    )
    .await;

    let engine = jmap_engine(&mock_server);
    let changes = engine
        .sync_changes("mb-inbox", Some("sync-state-900"))
        .await
        .expect("an unrecognised checkpoint must re-enumerate");
    assert_eq!(
        changes.present,
        Some(vec![placement_key("mb-inbox", "m-a")])
    );
    assert_eq!(changes.next_state, "v1:jmapstate:srv-full-1");
}

/// `Email/changes` is account-wide (RFC 8621 s4.2 has no mailbox filter), so a
/// folder pass sees ids that now live in other mailboxes. A message another
/// client moved out has left this folder: its occupancy here is reconciled
/// away, and it is NOT placed here on the strength of having merely changed.
#[tokio::test]
async fn jmap_sync_changes_reconciles_a_message_moved_out_of_the_folder() {
    let mock_server = MockServer::start().await;
    mount_session(&mock_server).await;
    mount_email_changes_page(
        &mock_server,
        "srv-state-1",
        json!({
            "oldState": "srv-state-1",
            "newState": "srv-state-2",
            "updated": ["m-moved", "m-stayed"],
            "hasMoreChanges": false
        }),
    )
    .await;
    mount_email_get(
        &mock_server,
        "srv-state-2",
        json!([
            message("m-moved", "mb-archive"),
            message("m-stayed", "mb-inbox")
        ]),
    )
    .await;

    let engine = jmap_engine(&mock_server);
    let changes = engine
        .sync_changes("mb-inbox", Some("v1:jmapstate:srv-state-1"))
        .await
        .expect("incremental sync succeeds");

    let upserted: Vec<&str> = changes
        .upserts
        .iter()
        .map(|m| m.placement.remote_id.as_str())
        .collect();
    assert_eq!(upserted, vec!["m-stayed"]);
    assert_eq!(
        changes.removals,
        vec![placement_key("mb-inbox", "m-moved")],
        "a message whose mailboxes no longer include this folder must lose its occupancy here"
    );
}
