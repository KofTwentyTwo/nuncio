//! WireMock integration test suite for JMAP (RFC 8620 / 8621) protocol engine.

use nuncio_mail::JmapEngine;
use serde_json::json;
use wiremock::matchers::{method, path};
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
    assert_eq!(emails[0].id, "msg-wm-100");
    assert_eq!(emails[0].subject, "WireMock JMAP E2E Test");
    assert_eq!(emails[0].sender, "sender@nuncio.mx");
    assert_eq!(new_state, "sync-state-500");
}
