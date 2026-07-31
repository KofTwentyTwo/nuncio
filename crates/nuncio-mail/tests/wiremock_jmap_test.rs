//! WireMock integration test suite for JMAP (RFC 8620 / 8621) protocol engine.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
    // The JMAP object id is preserved as the protocol-native `remote_id`, while
    // the persisted id is an opaque 64-char surrogate hash (never the raw id).
    assert_eq!(emails[0].remote_id, "msg-wm-100");
    assert_ne!(emails[0].id, "msg-wm-100");
    assert_eq!(emails[0].id.len(), 64);
    assert_eq!(emails[0].subject, "WireMock JMAP E2E Test");
    assert_eq!(emails[0].sender, "sender@nuncio.mx");
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
    assert_eq!(emails[0].remote_id, "msg-wm-200");
    assert_ne!(emails[0].id, "msg-wm-200");
    assert_eq!(emails[0].id.len(), 64);
    assert_eq!(emails[0].subject, "Trait-Level JMAP Sync");
    assert_eq!(emails[0].sender, "sender@nuncio.mx");
    assert_eq!(emails[0].received_at, 1700002000);
    assert!(!emails[0].read);
    assert_eq!(
        emails[0].body_plain,
        Some("Fetched through the MailBackend trait.".to_string())
    );
    assert_eq!(new_state, "sync-state-900");
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
