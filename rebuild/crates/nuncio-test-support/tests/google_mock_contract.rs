#![allow(clippy::unwrap_used, clippy::expect_used)]

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use nuncio_test_support::google::{MockGoogle, Seed};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;
use url::Url;

#[tokio::test]
async fn reply_forward_fixture_preserves_non_utf8_and_inline_mime_through_raw_http() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let raw = include_str!("../fixtures/mime/reply-forward.eml")
        .replace('\n', "\r\n")
        .into_bytes();
    mock.control()
        .add_message(
            "alpha@example.test",
            "prepare-001",
            "prepare-thread",
            raw.clone(),
            ["INBOX".into()].into(),
        )
        .await
        .unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let response = get(
        &mock,
        grant["access_token"].as_str().unwrap(),
        "/gmail/v1/users/me/messages/prepare-001?format=raw",
    )
    .await;
    assert_eq!(response.status(), 200);
    let wire: Value = response.json().await.unwrap();
    let received = URL_SAFE_NO_PAD
        .decode(wire["raw"].as_str().unwrap())
        .unwrap();
    assert_eq!(received, raw);
    let parsed = mailparse::parse_mail(&received).unwrap();
    assert_eq!(parsed.subparts.len(), 3);
    let text_attachment = &parsed.subparts[1];
    assert_eq!(text_attachment.get_body_raw().unwrap(), b"caf\xe9");
    assert_eq!(text_attachment.ctype.params["charset"], "windows-1252");
    assert_eq!(text_attachment.ctype.params["format"], "fixed");
    assert_eq!(
        parsed.subparts[2].get_body_raw().unwrap(),
        b"\x89PNG\r\n\x1a\n"
    );
    use mailparse::MailHeaderMap;
    assert_eq!(
        parsed.subparts[2]
            .headers
            .get_first_value("Content-ID")
            .as_deref(),
        Some("<logo@example.test>")
    );
    assert!(mock
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_observation_attributes_oauth_and_revoked_credentials_without_authorizing_them() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let alpha = login(&mock, "alpha@example.test").await;
    let beta = login(&mock, "beta@example.test").await;
    let refresh = |token: String| {
        client()
            .post(format!("{}/token", mock.base_url()))
            .form(&[
                ("client_id", "nuncio-test-client"),
                ("grant_type", "refresh_token"),
                ("refresh_token", token.as_str()),
            ])
            .send()
    };
    mock.control()
        .inject(Fault {
            method: "POST".into(),
            path: "/token".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 503,
                retry_after_secs: Some(6),
            },
        })
        .await;
    assert_eq!(
        refresh(beta["refresh_token"].as_str().unwrap().into())
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        refresh(alpha["refresh_token"].as_str().unwrap().into())
            .await
            .unwrap()
            .status(),
        503
    );
    assert_eq!(
        refresh(alpha["refresh_token"].as_str().unwrap().into())
            .await
            .unwrap()
            .status(),
        200
    );
    mock.control().revoke("alpha@example.test").await;
    assert_eq!(
        get(
            &mock,
            alpha["access_token"].as_str().unwrap(),
            "/gmail/v1/users/me/profile"
        )
        .await
        .status(),
        401
    );
    assert_eq!(
        refresh(alpha["refresh_token"].as_str().unwrap().into())
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["error"],
        "invalid_grant"
    );
    let snapshot = mock.control().snapshot().await;
    assert_eq!(
        snapshot
            .requests
            .iter()
            .find(|r| r.account.as_deref() == Some("alpha@example.test")
                && r.path == "/gmail/v1/users/me/profile")
            .unwrap()
            .count,
        1
    );
    assert_eq!(
        snapshot
            .requests
            .iter()
            .find(|r| r.account.as_deref() == Some("alpha@example.test") && r.path == "/token")
            .unwrap()
            .count,
        4
    );
    let recorded = serde_json::to_string(&snapshot).unwrap();
    for secret in ["access_token", "refresh_token"] {
        assert!(!recorded.contains(alpha[secret].as_str().unwrap()));
    }
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn retry_after_date_is_preserved_on_the_wire_and_fault_consumes_one_call() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let path = "/gmail/v1/users/me/profile";
    mock.control()
        .inject(Fault {
            method: "GET".into(),
            path: path.into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::Before,
            action: FaultAction::StatusWithRetryDate {
                code: 503,
                retry_after: "Sat, 07 Mar 2026 15:00:06 GMT".into(),
            },
        })
        .await;
    let response = get(&mock, token, path).await;
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.headers()["retry-after"],
        "Sat, 07 Mar 2026 15:00:06 GMT"
    );
    assert_eq!(get(&mock, token, path).await.status(), 200);
    assert!(mock
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn userinfo_has_stable_scoped_identity_and_refresh_rotation_is_real() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let invalid = client()
        .post(format!("{}/token", mock.base_url()))
        .form(&[
            ("client_id", "nuncio-test-client"),
            ("client_secret", "wrong-registration-secret"),
            ("grant_type", "refresh_token"),
            ("refresh_token", "deliberately-invalid"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);
    assert_eq!(
        invalid.json::<Value>().await.unwrap()["error"],
        "invalid_client"
    );
    let response = client()
        .get(format!("{}/v1/userinfo", mock.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let grant = login(&mock, "alpha@example.test").await;
    // Existing login requests Gmail/Calendar only: they do not grant identity access.
    assert_eq!(
        get(
            &mock,
            grant["access_token"].as_str().unwrap(),
            "/v1/userinfo"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let response = client()
        .get(format!("{}/o/oauth2/v2/auth", mock.base_url()))
        .query(&[
            ("client_id", "nuncio-test-client"),
            ("redirect_uri", "http://127.0.0.1:8089/callback"),
            ("response_type", "code"),
            ("state", "identity-state"),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER)).as_str(),
            ),
            ("scope", "openid email"),
            ("access_type", "offline"),
            ("login_hint", "alpha@example.test"),
        ])
        .send()
        .await
        .unwrap();
    let redirect = Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
    let code = redirect
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let grant = json_ok(exchange(&mock, &code, VERIFIER).await).await;
    assert_eq!(
        grant["scope"],
        "openid https://www.googleapis.com/auth/userinfo.email"
    );
    let identity = json_ok(
        get(
            &mock,
            grant["access_token"].as_str().unwrap(),
            "/v1/userinfo",
        )
        .await,
    )
    .await;
    assert_eq!(
        identity,
        serde_json::json!({"sub":"100000000000000000001","email":"alpha@example.test","email_verified":true})
    );
    assert_eq!(
        get(
            &mock,
            grant["access_token"].as_str().unwrap(),
            "/v1/userinfo?typo=1"
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    mock.control().rotate_refresh_tokens(true).await;
    let refresh = grant["refresh_token"].as_str().unwrap();
    let rotated = json_ok(
        client()
            .post(format!("{}/token", mock.base_url()))
            .form(&[
                ("client_id", "nuncio-test-client"),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
            ])
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_ne!(rotated["refresh_token"], grant["refresh_token"]);
    assert_eq!(
        json_ok(
            get(
                &mock,
                rotated["access_token"].as_str().unwrap(),
                "/v1/userinfo"
            )
            .await
        )
        .await,
        identity
    );
    let stale = client()
        .post(format!("{}/token", mock.base_url()))
        .form(&[
            ("client_id", "nuncio-test-client"),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::BAD_REQUEST);
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_timezone_freebusy_and_external_guest_classification() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    mock.control().set_page_cap(100).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let response=json_ok(get(&mock,token,"/calendar/v3/calendars/primary/events?singleEvents=true&timeZone=UTC&timeMin=2026-03-01T00%3A00%3A00Z&timeMax=2026-03-02T00%3A00%3A00Z").await).await;
    assert_eq!(response["timeZone"], "UTC");
    assert_eq!(
        response["items"][0]["start"]["dateTime"],
        "2026-03-01T15:00:00+00:00"
    );
    json_ok(
        post(
            &mock,
            token,
            "/calendar/v3/calendars/primary/events?sendUpdates=externalOnly",
            serde_json::json!({
                "id":"guesttest001","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"},
                "attendees":[{"email":"guest@example.test"},{"email":"visitor@another.test"}]
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        mock.control().snapshot().await.calendars["alpha@example.test"]["alpha@example.test"]
            .notifications[0]
            .recipients,
        vec!["visitor@another.test"]
    );
    let free = serde_json::json!({"timeMin":"2026-10-01T00:00:00Z","timeMax":"2026-10-03T00:00:00Z","timeZone":"America/Chicago","items":[{"id":"primary"}]});
    let busy = json_ok(post(&mock, token, "/calendar/v3/freeBusy", free.clone()).await).await;
    assert_eq!(
        busy["calendars"]["primary"]["busy"][0]["start"],
        "2026-10-01T00:00:00-05:00"
    );
    let mut bad = free;
    bad["timeZone"] = serde_json::json!("Not/A_Zone");
    assert_eq!(
        post(&mock, token, "/calendar/v3/freeBusy", bad)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn out_of_band_seed_edit_delete_advance_real_delta_state() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    mock.control().set_page_cap(100).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let profile = json_ok(get(&mock, token, "/gmail/v1/users/me/profile").await).await;
    let cursor = profile["historyId"].as_str().unwrap();
    mock.control().add_message("alpha@example.test","external001","external-thread",
        b"From: remote@example.test\r\nTo: alpha@example.test\r\nSubject: Remote arrival\r\nMessage-ID: <arrival@example.test>\r\n\r\nRemote bytes\r\n".to_vec(),["INBOX".into()].into()).await.unwrap();
    mock.control()
        .change_labels(
            "alpha@example.test",
            "external001",
            &["STARRED".into()],
            &["INBOX".into()],
        )
        .await
        .unwrap();
    let history = json_ok(
        get(
            &mock,
            token,
            &format!("/gmail/v1/users/me/history?startHistoryId={cursor}"),
        )
        .await,
    )
    .await;
    assert_eq!(history["history"].as_array().unwrap().len(), 2);
    assert_eq!(
        history["history"][0]["messagesAdded"][0]["message"]["id"],
        "external001"
    );
    assert_eq!(
        history["history"][1]["labelsRemoved"][0]["labelIds"],
        serde_json::json!(["INBOX"])
    );
    assert_eq!(
        get(
            &mock,
            token,
            &format!(
                "/gmail/v1/users/me/history?startHistoryId={}",
                cursor.parse::<u64>().unwrap() + 1
            )
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    mock.control()
        .delete_event("alpha@example.test", "primary", "alldate01")
        .await
        .unwrap();
    let deleted = json_ok(
        get(
            &mock,
            token,
            "/calendar/v3/calendars/primary/events/alldate01",
        )
        .await,
    )
    .await;
    assert_eq!(deleted["status"], "cancelled");
    assert!(deleted.get("start").is_none());
    assert!(
        mock.control().snapshot().await.calendars["beta@example.test"]["beta@example.test"].events
            ["alldate01"]
            .get("start")
            .is_some()
    );
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn newly_created_recurring_series_supports_instance_get_edit_and_cancel() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    mock.control().set_page_cap(100).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let base = "/calendar/v3/calendars/primary/events";
    json_ok(post(&mock,token,&format!("{base}?sendUpdates=none"),serde_json::json!({
        "id":"series001","summary":"New weekly","start":{"dateTime":"2026-10-25T09:00:00-05:00","timeZone":"America/Chicago"},
        "end":{"dateTime":"2026-10-25T10:00:00-05:00","timeZone":"America/Chicago"},"recurrence":["RRULE:FREQ=WEEKLY;COUNT=3"]
    })).await).await;
    let master = json_ok(get(&mock, token, &format!("{base}/series001")).await).await;
    let updated = client()
        .patch(format!(
            "{}{base}/series001?sendUpdates=none",
            mock.base_url()
        ))
        .bearer_auth(token)
        .header("if-match", master["etag"].as_str().unwrap())
        .json(&serde_json::json!({"summary":"Whole series"}))
        .send()
        .await
        .unwrap();
    assert_eq!(json_ok(updated).await["summary"], "Whole series");
    let instances = json_ok(get(&mock, token, &format!("{base}/series001/instances")).await).await;
    assert!(instances["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["summary"] == "Whole series"));
    assert_eq!(instances["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        instances["items"][1]["start"]["dateTime"],
        "2026-11-01T09:00:00-06:00"
    );
    let instance = &instances["items"][1];
    let path = format!("{base}/{}", instance["id"].as_str().unwrap());
    assert_eq!(
        json_ok(get(&mock, token, &path).await).await,
        instance.clone()
    );
    let patch = client()
        .patch(format!("{}{path}?sendUpdates=none", mock.base_url()))
        .bearer_auth(token)
        .header("if-match", instance["etag"].as_str().unwrap())
        .json(&serde_json::json!({"summary":"One occurrence"}))
        .send()
        .await
        .unwrap();
    let patch = json_ok(patch).await;
    assert_eq!(patch["recurringEventId"], "series001");
    assert_eq!(patch["originalStartTime"], instance["start"]);
    assert_eq!(
        json_ok(get(&mock, token, &format!("{base}/series001")).await).await["summary"],
        "Whole series"
    );
    let deleted = client()
        .delete(format!("{}{path}?sendUpdates=none", mock.base_url()))
        .bearer_auth(token)
        .header("if-match", patch["etag"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let instances = json_ok(
        get(
            &mock,
            token,
            &format!("{base}/series001/instances?showDeleted=false"),
        )
        .await,
    )
    .await;
    assert_eq!(instances["items"].as_array().unwrap().len(), 2);
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn oauth_rejects_unknown_scope_and_metadata_cannot_read_raw() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    for (scope, expected) in [
        (
            "https://www.googleapis.com/auth/does-not-exist",
            StatusCode::BAD_REQUEST,
        ),
        (
            "https://www.googleapis.com/auth/gmail.metadata",
            StatusCode::FOUND,
        ),
    ] {
        let response = client()
            .get(format!("{}/o/oauth2/v2/auth", mock.base_url()))
            .query(&[
                ("client_id", "nuncio-test-client"),
                ("redirect_uri", "http://127.0.0.1:8089/callback"),
                ("response_type", "code"),
                ("state", "fixture-state"),
                ("code_challenge_method", "S256"),
                (
                    "code_challenge",
                    &URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER)),
                ),
                ("scope", scope),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == StatusCode::FOUND {
            let redirect = Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
            let code = redirect
                .query_pairs()
                .find(|(key, _)| key == "code")
                .unwrap()
                .1
                .into_owned();
            let grant = json_ok(exchange(&mock, &code, VERIFIER).await).await;
            let token = grant["access_token"].as_str().unwrap();
            assert_eq!(
                get(
                    &mock,
                    token,
                    "/gmail/v1/users/me/messages/m-001?format=metadata"
                )
                .await
                .status(),
                StatusCode::OK
            );
            assert_eq!(
                get(&mock, token, "/gmail/v1/users/me/messages/m-001?format=raw")
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                get(
                    &mock,
                    token,
                    "/gmail/v1/users/me/messages?q=rfc822msgid%3Amultipart%40example.test"
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
    }
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn truncation_delivers_a_prefix_then_fails_and_shutdown_cancels_withholding() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let path = "/gmail/v1/users/me/profile";
    mock.control()
        .inject(Fault {
            method: "GET".into(),
            path: path.into(),
            account: None,
            call: None,
            phase: Phase::Before,
            action: FaultAction::TruncatedBody,
        })
        .await;
    let mut response = client()
        .get(format!("{}{path}", mock.base_url()))
        .bearer_auth(token)
        .send()
        .await
        .expect("prefix headers must arrive");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.chunk().await.unwrap().unwrap().as_ref(),
        b"{\"partial\":"
    );
    assert!(response.chunk().await.is_err());
    mock.control()
        .inject(Fault {
            method: "GET".into(),
            path: path.into(),
            account: None,
            call: None,
            phase: Phase::After,
            action: FaultAction::Withhold {
                barrier: "stop-test".into(),
            },
        })
        .await;
    let request = client()
        .get(format!("{}{path}", mock.base_url()))
        .bearer_auth(token);
    let task = tokio::spawn(async move { request.send().await });
    mock.control().wait_for_barrier("stop-test").await.unwrap();
    mock.shutdown().await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    if let Ok(response) = response {
        assert!(response.bytes().await.is_err());
    }
}

#[tokio::test]
async fn mock_rejects_nested_typos_and_missing_send_recipients() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let good = serde_json::json!({"id":"strict001","start":{"date":"2026-11-01"},"end":{"date":"2026-11-02"}});
    for extra in [
        serde_json::json!({"sumary":null}),
        serde_json::json!({"reminders":{"useDefalt":true}}),
        serde_json::json!({"reminders":{"useDefault":"true"}}),
        serde_json::json!({"reminders":{"useDefault":false,"overrides":[{"method":"popup","minutes":-1}]}}),
        serde_json::json!({"attendees":[{"email":"guest@example.test","optional":3}]}),
        serde_json::json!({"extendedProperties":{"privte":{"key":"value"}}}),
        serde_json::json!({"extendedProperties":{"private":{"key":42}}}),
    ] {
        let mut body = good.clone();
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            post(&mock, token, "/calendar/v3/calendars/primary/events", body)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let missing = b"From: alpha@example.test\r\nSubject: No recipient\r\n\r\nBody\r\n";
    assert_eq!(
        post(
            &mock,
            token,
            "/gmail/v1/users/me/messages/send",
            serde_json::json!({"raw":URL_SAFE_NO_PAD.encode(missing)})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(mock
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn standalone_mock_announces_readiness_and_resets_through_stdin_only() {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let temp = tempfile::tempdir().unwrap();
    let ready = temp.path().join("google-ready.json");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mock-google"))
        .env_clear()
        .args(["--ready-file", ready.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stdin = child.stdin.take().unwrap();
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .expect("ready announcement");
    let announced: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(announced["event"], "ready");
    let saved: Value = serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
    assert_eq!(announced, saved);
    let endpoint = saved["base_url"].as_str().unwrap();
    assert!(endpoint.starts_with("http://127.0.0.1:"));
    assert_eq!(
        client()
            .get(format!("{endpoint}/_control/reset"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    for command in [
        r#"{"op":"snapshot"}"#,
        r#"{"op":"reset"}"#,
        r#"{"op":"snapshot"}"#,
    ] {
        stdin
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert!(response.get("error").is_none());
        if command.contains("snapshot") {
            assert_eq!(
                response["result"]["mail"]["alpha@example.test"]["messages"]
                    .as_object()
                    .unwrap()
                    .len(),
                3
            );
        }
    }
    stdin.write_all(b"{\"op\":\"shutdown\"}\n").await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    assert!(client()
        .get(format!("{endpoint}/gmail/v1/users/me/profile"))
        .send()
        .await
        .is_err());
}

#[tokio::test]
async fn calendar_lost_ack_counts_invitations_and_external_edits_conflict() {
    use nuncio_test_support::google::{CursorScope, Fault, FaultAction, Phase};
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    mock.control().set_page_cap(100).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let base = "/calendar/v3/calendars/primary/events";
    let initial = json_ok(
        get(
            &mock,
            token,
            &format!("{base}?singleEvents=false&showDeleted=true"),
        )
        .await,
    )
    .await;
    let event = serde_json::json!({"id":"invite001","summary":"Invitation","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"},
        "attendees":[{"email":"guest@example.test"},{"email":"guest@external.test"}]});
    mock.control()
        .inject(Fault {
            method: "POST".into(),
            path: base.into(),
            account: None,
            call: None,
            phase: Phase::After,
            action: FaultAction::Disconnect,
        })
        .await;
    let response = client()
        .post(format!("{}{base}?sendUpdates=all", mock.base_url()))
        .bearer_auth(token)
        .json(&event)
        .send()
        .await;
    if let Ok(response) = response {
        assert!(response.bytes().await.is_err());
    }
    let remote = mock.control().snapshot().await;
    let calendar = &remote.calendars["alpha@example.test"]["alpha@example.test"];
    assert_eq!(calendar.notifications.len(), 1);
    assert_eq!(
        calendar.notifications[0].recipients,
        vec!["guest@example.test", "guest@external.test"]
    );
    assert!(calendar.events.contains_key("invite001"));
    assert_eq!(
        post(&mock, token, &format!("{base}?sendUpdates=all"), event)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let observed = json_ok(get(&mock, token, &format!("{base}/invite001")).await).await;
    let mut external = observed.clone();
    external["summary"] = serde_json::json!("Changed elsewhere");
    external["futureProviderField"] = serde_json::json!({"opaque":true});
    mock.control()
        .put_event("alpha@example.test", "primary", external)
        .await
        .unwrap();
    let conflict = client()
        .patch(format!(
            "{}{base}/invite001?sendUpdates=all",
            mock.base_url()
        ))
        .bearer_auth(token)
        .header("if-match", observed["etag"].as_str().unwrap())
        .json(&serde_json::json!({"summary":"Overwrite"}))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::PRECONDITION_FAILED);
    let fresh = json_ok(get(&mock, token, &format!("{base}/invite001")).await).await;
    let response = client()
        .patch(format!(
            "{}{base}/invite001?sendUpdates=externalOnly",
            mock.base_url()
        ))
        .bearer_auth(token)
        .header("if-match", fresh["etag"].as_str().unwrap())
        .json(&serde_json::json!({"summary":"Confirmed edit"}))
        .send()
        .await
        .unwrap();
    let patched = json_ok(response).await;
    assert_eq!(
        patched["futureProviderField"],
        serde_json::json!({"opaque":true})
    );
    let remote = mock.control().snapshot().await;
    let effects = &remote.calendars["alpha@example.test"]["alpha@example.test"].notifications;
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[1].recipients, vec!["guest@external.test"]);
    let delta = format!(
        "{base}?singleEvents=false&showDeleted=true&syncToken={}",
        initial["nextSyncToken"].as_str().unwrap()
    );
    assert_eq!(
        json_ok(get(&mock, token, &delta).await).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    mock.control()
        .expire_cursor(
            "alpha@example.test",
            CursorScope::Calendar("primary".into()),
        )
        .await
        .unwrap();
    assert_eq!(get(&mock, token, &delta).await.status(), StatusCode::GONE);
    assert_eq!(
        get(
            &mock,
            token,
            "/calendar/v3/calendars/alpha%40example.test/events/invite001"
        )
        .await
        .status(),
        StatusCode::OK
    );
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn overlap_reordering_latency_selection_and_reset_are_controlled() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    mock.control().set_page_overlap(true).await;
    let first = json_ok(get(&mock, token, "/gmail/v1/users/me/messages").await).await;
    let next = first["nextPageToken"].as_str().unwrap();
    let second = json_ok(
        get(
            &mock,
            token,
            &format!("/gmail/v1/users/me/messages?pageToken={next}"),
        )
        .await,
    )
    .await;
    assert_eq!(first["messages"][1], second["messages"][0]);
    assert_eq!(second["messages"][1]["id"], "m-003");
    mock.control().reverse_results(true).await;
    let reversed = json_ok(get(&mock, token, "/gmail/v1/users/me/messages").await).await;
    assert_eq!(reversed["messages"][0]["id"], "m-003");
    let path = "/gmail/v1/users/me/profile";
    mock.control()
        .inject(Fault {
            method: "GET".into(),
            path: path.into(),
            account: Some("beta@example.test".into()),
            call: Some(1),
            phase: Phase::Before,
            action: FaultAction::Status {
                code: 503,
                retry_after_secs: None,
            },
        })
        .await;
    assert_eq!(get(&mock, token, path).await.status(), StatusCode::OK);
    let beta = login(&mock, "beta@example.test").await;
    assert_eq!(
        get(&mock, beta["access_token"].as_str().unwrap(), path)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    mock.control()
        .inject(Fault {
            method: "GET".into(),
            path: path.into(),
            account: None,
            call: None,
            phase: Phase::Before,
            action: FaultAction::Delay { millis: 30 },
        })
        .await;
    let before = std::time::Instant::now();
    assert_eq!(get(&mock, token, path).await.status(), StatusCode::OK);
    assert!(before.elapsed() >= Duration::from_millis(30));
    mock.control().reset(Seed::TwoAccounts).await.unwrap();
    assert_eq!(
        get(&mock, token, path).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        mock.control().snapshot().await.mail["alpha@example.test"]
            .messages
            .len(),
        3
    );
    let grant = login(&mock, "alpha@example.test").await;
    assert_eq!(
        get(
            &mock,
            grant["access_token"].as_str().unwrap(),
            &format!("/gmail/v1/users/me/messages?pageToken={next}")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_canonical_pages_are_distinct_from_dst_occurrences() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let calendars = json_ok(get(&mock, token, "/calendar/v3/users/me/calendarList").await).await;
    assert_eq!(calendars["items"].as_array().unwrap().len(), 2);
    assert!(calendars["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["primary"] == true));
    let base = "/calendar/v3/calendars/primary/events?singleEvents=false&showDeleted=true";
    let first = json_ok(get(&mock, token, base).await).await;
    assert!(first.get("nextSyncToken").is_none());
    let next = first["nextPageToken"].as_str().unwrap();
    let page_path = format!("{base}&pageToken={next}");
    let last = json_ok(get(&mock, token, &page_path).await).await;
    assert!(last.get("nextPageToken").is_none());
    assert!(last["nextSyncToken"].is_string());
    assert_eq!(json_ok(get(&mock, token, &page_path).await).await, last);
    let mut all = first["items"].as_array().unwrap().clone();
    all.extend(last["items"].as_array().unwrap().clone());
    assert!(all.iter().any(|e| e.get("recurrence").is_some()));
    let cancelled = all.iter().find(|e| e["status"] == "cancelled").unwrap();
    assert!(
        cancelled.get("start").is_none(),
        "sparse cancelled exceptions must not become invented timed events"
    );
    let day = all.iter().find(|e| e["id"] == "alldate01").unwrap();
    assert_eq!(day["start"], serde_json::json!({"date":"2026-03-09"}));
    assert_eq!(day["end"], serde_json::json!({"date":"2026-03-10"}));
    let delta = format!(
        "{base}&syncToken={}",
        last["nextSyncToken"].as_str().unwrap()
    );
    assert!(json_ok(get(&mock, token, &delta).await).await["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        get(
            &mock,
            token,
            &format!("{delta}&timeMin=2026-03-01T00%3A00%3A00Z")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&mock, token, &format!("{base}&syncToken={next}"))
            .await
            .status(),
        StatusCode::GONE
    );
    let expanded = "/calendar/v3/calendars/primary/events?singleEvents=true&showDeleted=true&timeMin=2026-03-01T00%3A00%3A00Z&timeMax=2026-03-17T00%3A00%3A00Z&orderBy=startTime";
    let mut path = expanded.to_string();
    let mut occurrences = Vec::new();
    loop {
        let page = json_ok(get(&mock, token, &path).await).await;
        occurrences.extend(page["items"].as_array().unwrap().clone());
        match page["nextPageToken"].as_str() {
            Some(next) => path = format!("{expanded}&pageToken={next}"),
            None => break,
        }
    }
    assert!(occurrences.iter().all(|e| e.get("recurrence").is_none()));
    let first = occurrences
        .iter()
        .find(|e| e["id"] == "dstseries_20260301T150000Z")
        .unwrap();
    assert_eq!(first["start"]["dateTime"], "2026-03-01T09:00:00-06:00");
    let moved = occurrences
        .iter()
        .find(|e| e["id"] == "dstseries_20260308T140000Z")
        .unwrap();
    assert_eq!(
        moved["originalStartTime"]["dateTime"],
        "2026-03-08T09:00:00-05:00"
    );
    assert_eq!(moved["start"]["dateTime"], "2026-03-08T10:30:00-05:00");
    let instances=json_ok(get(&mock,token,"/calendar/v3/calendars/primary/events/dstseries/instances?showDeleted=true&maxResults=100").await).await;
    assert_eq!(instances["items"].as_array().unwrap().len(), 2);
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_conditional_writes_preserve_fields_and_reject_invalid_wire() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let path = "/calendar/v3/calendars/primary/events";
    let event = serde_json::json!({"id":"eventtest001","summary":"New event",
        "start":{"dateTime":"2026-03-20T09:00:00-05:00","timeZone":"America/Chicago"},
        "end":{"dateTime":"2026-03-20T10:00:00-05:00","timeZone":"America/Chicago"},
        "attendees":[{"email":"guest@example.test","responseStatus":"needsAction"}],
        "extendedProperties":{"private":{"preserve":"opaque-value"}},"reminders":{"useDefault":false,"overrides":[{"method":"popup","minutes":10}]}});
    let created = json_ok(
        post(
            &mock,
            token,
            &format!("{path}?sendUpdates=all"),
            event.clone(),
        )
        .await,
    )
    .await;
    assert_eq!(
        post(&mock, token, path, event).await.status(),
        StatusCode::CONFLICT
    );
    let id_path = format!("{path}/eventtest001");
    let patched = client()
        .patch(format!("{}{id_path}?sendUpdates=none", mock.base_url()))
        .bearer_auth(token)
        .header("if-match", created["etag"].as_str().unwrap())
        .json(&serde_json::json!({"summary":"Edited"}))
        .send()
        .await
        .unwrap();
    let patched = json_ok(patched).await;
    assert_eq!(patched["summary"], "Edited");
    assert_eq!(
        patched["extendedProperties"]["private"]["preserve"],
        "opaque-value"
    );
    assert!(patched["reminders"]["overrides"].is_array());
    let conflict = client()
        .patch(format!("{}{id_path}?sendUpdates=all", mock.base_url()))
        .bearer_auth(token)
        .header("if-match", created["etag"].as_str().unwrap())
        .json(&serde_json::json!({"summary":"Must not overwrite"}))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        json_ok(get(&mock, token, &id_path).await).await["summary"],
        "Edited"
    );
    for payload in [
        serde_json::json!({"id":"bad-ID","start":{"date":"2026-03-20"},"end":{"date":"2026-03-21"}}),
        serde_json::json!({"id":"valid01","start":{"date":"2026-03-20"},"end":{"date":"2026-03-20"}}),
        serde_json::json!({"id":"valid02","sumary":"typo","start":{"date":"2026-03-20"},"end":{"date":"2026-03-21"}}),
        serde_json::json!({"id":"valid03","summary":42,"start":{"date":"2026-03-20"},"end":{"date":"2026-03-21"}}),
    ] {
        assert_eq!(
            post(&mock, token, path, payload).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    let free=json_ok(post(&mock,token,"/calendar/v3/freeBusy",serde_json::json!({"timeMin":"2026-03-20T00:00:00Z","timeMax":"2026-03-21T00:00:00Z","items":[{"id":"primary"},{"id":"unknown"}]})).await).await;
    assert_eq!(
        free["calendars"]["primary"]["busy"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        free["calendars"]["unknown"]["errors"][0]["reason"],
        "notFound"
    );
    let deleted = client()
        .delete(format!("{}{id_path}?sendUpdates=all", mock.base_url()))
        .bearer_auth(token)
        .header("if-match", patched["etag"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        json_ok(get(&mock, token, &id_path).await).await["status"],
        "cancelled"
    );
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn remote_controls_history_expiry_snapshot_pages_and_withheld_ack() {
    use nuncio_test_support::google::{CursorScope, Fault, FaultAction, Phase};
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let initial = json_ok(get(&mock, token, "/gmail/v1/users/me/profile").await).await;
    mock.control().set_page_cap(1).await.unwrap();
    let page1 = json_ok(get(&mock, token, "/gmail/v1/users/me/messages").await).await;
    assert_eq!(page1["messages"].as_array().unwrap().len(), 1);
    let path = format!(
        "/gmail/v1/users/me/messages?pageToken={}",
        page1["nextPageToken"].as_str().unwrap()
    );
    let page2 = json_ok(get(&mock, token, &path).await).await;
    mock.control()
        .delete_message("alpha@example.test", "m-002")
        .await
        .unwrap();
    assert_eq!(
        get(&mock, token, "/gmail/v1/users/me/messages/m-002")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        json_ok(get(&mock, token, &path).await).await,
        page2,
        "snapshot pages remain reusable despite mutations"
    );
    let history_path = format!(
        "/gmail/v1/users/me/history?startHistoryId={}",
        initial["historyId"].as_str().unwrap()
    );
    let delta = json_ok(get(&mock, token, &history_path).await).await;
    assert_eq!(
        delta["history"][0]["messagesDeleted"][0]["message"]["id"],
        "m-002"
    );
    mock.control()
        .expire_cursor("alpha@example.test", CursorScope::GmailHistory)
        .await
        .unwrap();
    assert_eq!(
        get(&mock, token, &history_path).await.status(),
        StatusCode::NOT_FOUND
    );
    mock.control()
        .expire_cursor("alpha@example.test", CursorScope::Pages)
        .await
        .unwrap();
    assert_eq!(
        get(&mock, token, &path).await.status(),
        StatusCode::BAD_REQUEST
    );
    let send_path = "/gmail/v1/users/me/messages/send";
    mock.control()
        .inject(Fault {
            method: "POST".into(),
            path: send_path.into(),
            account: Some("alpha@example.test".into()),
            call: Some(1),
            phase: Phase::After,
            action: FaultAction::Withhold {
                barrier: "accepted-send".into(),
            },
        })
        .await;
    let http = client();
    let url = format!("{}{send_path}", mock.base_url());
    let payload = serde_json::json!({"raw":URL_SAFE_NO_PAD.encode(b"From: alpha@example.test\r\nTo: recipient@example.test\r\nSubject: Withheld\r\n\r\nBody\r\n")});
    let request = http.post(url).bearer_auth(token).json(&payload);
    let task = tokio::spawn(async move { request.send().await });
    mock.control()
        .wait_for_barrier("accepted-send")
        .await
        .unwrap();
    assert!(!task.is_finished());
    assert_eq!(
        mock.control()
            .accepted_sends("alpha@example.test")
            .await
            .len(),
        1
    );
    mock.control().release_barrier("accepted-send").await;
    assert_eq!(task.await.unwrap().unwrap().status(), StatusCode::OK);
    assert_eq!(
        mock.control()
            .snapshot()
            .await
            .requests
            .iter()
            .find(|r| r.path == send_path)
            .unwrap()
            .count,
        1
    );
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn faults_distinguish_pre_acceptance_from_lost_send_ack_and_report_statuses() {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let path = "/gmail/v1/users/me/messages/send";
    let payload = serde_json::json!({"raw":URL_SAFE_NO_PAD.encode(b"From: alpha@example.test\r\nTo: recipient@example.test\r\nSubject: Fault test\r\nMessage-ID: <fault@example.test>\r\n\r\nPayload\r\n")});
    for (phase, expected) in [(Phase::Before, 0), (Phase::After, 1)] {
        mock.control()
            .inject(Fault {
                method: "POST".into(),
                path: path.into(),
                account: Some("alpha@example.test".into()),
                call: None,
                phase,
                action: FaultAction::Disconnect,
            })
            .await;
        let response = client()
            .post(format!("{}{path}", mock.base_url()))
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await;
        match response {
            Err(_) => {}
            Ok(response) => assert!(
                response.bytes().await.is_err(),
                "must lose the acknowledgement"
            ),
        }
        assert_eq!(
            mock.control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            expected
        );
    }
    for code in [401, 403, 404, 410, 412, 429, 500, 503] {
        mock.control()
            .inject(Fault {
                method: "GET".into(),
                path: "/gmail/v1/users/me/profile".into(),
                account: None,
                call: None,
                phase: Phase::Before,
                action: FaultAction::Status {
                    code,
                    retry_after_secs: Some(7),
                },
            })
            .await;
        let response = get(&mock, token, "/gmail/v1/users/me/profile").await;
        assert_eq!(response.status().as_u16(), code);
        assert_eq!(response.headers()["retry-after"], "7");
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"]["code"],
            code
        );
    }
    for action in [FaultAction::MalformedJson, FaultAction::TruncatedBody] {
        mock.control()
            .inject(Fault {
                method: "GET".into(),
                path: "/gmail/v1/users/me/profile".into(),
                account: None,
                call: None,
                phase: Phase::Before,
                action,
            })
            .await;
        let response = client()
            .get(format!("{}/gmail/v1/users/me/profile", mock.base_url()))
            .bearer_auth(token)
            .send()
            .await;
        if let Ok(response) = response {
            assert!(response.json::<Value>().await.is_err());
        }
    }
    assert_eq!(
        get(&mock, token, "/gmail/v1/users/me/profile")
            .await
            .status(),
        StatusCode::OK,
        "faults are one-shot"
    );
    mock.shutdown().await.unwrap();
}

async fn get(mock: &MockGoogle, token: &str, path: &str) -> reqwest::Response {
    client()
        .get(format!("{}{path}", mock.base_url()))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}
async fn post(mock: &MockGoogle, token: &str, path: &str, body: Value) -> reqwest::Response {
    client()
        .post(format!("{}{path}", mock.base_url()))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
}
async fn json_ok(response: reqwest::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

#[tokio::test]
async fn oauth_expiry_refresh_revocation_denial_and_invalid_redirects() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let a = login(&mock, "alpha@example.test").await;
    let token = a["access_token"].as_str().unwrap();
    mock.control().advance(Duration::from_secs(3601)).await;
    assert_eq!(
        get(&mock, token, "/gmail/v1/users/me/profile")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let refresh = client()
        .post(format!("{}/token", mock.base_url()))
        .form(&[
            ("client_id", "nuncio-test-client"),
            ("grant_type", "refresh_token"),
            ("refresh_token", a["refresh_token"].as_str().unwrap()),
        ])
        .send()
        .await
        .unwrap();
    let refreshed = json_ok(refresh).await;
    assert!(refreshed.get("refresh_token").is_none());
    assert_eq!(
        get(
            &mock,
            refreshed["access_token"].as_str().unwrap(),
            "/gmail/v1/users/me/profile"
        )
        .await
        .status(),
        StatusCode::OK
    );
    mock.control().revoke("alpha@example.test").await;
    let revoked = client()
        .post(format!("{}/token", mock.base_url()))
        .form(&[
            ("client_id", "nuncio-test-client"),
            ("grant_type", "refresh_token"),
            ("refresh_token", a["refresh_token"].as_str().unwrap()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        revoked.json::<Value>().await.unwrap()["error"],
        "invalid_grant"
    );
    let expired_code = authorize(&mock, "beta@example.test").await;
    mock.control().advance(Duration::from_secs(301)).await;
    let code = expired_code
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(
        exchange(&mock, &code, VERIFIER).await.status(),
        StatusCode::BAD_REQUEST
    );
    mock.control().deny_consent(true).await;
    let denial = authorize(&mock, "beta@example.test").await;
    assert_eq!(
        denial.query_pairs().find(|(k, _)| k == "error").unwrap().1,
        "access_denied"
    );
    assert!(denial.query_pairs().all(|(k, _)| k != "code"));
    mock.control().deny_consent(false).await;
    mock.control()
        .deny_scope("https://www.googleapis.com/auth/gmail.modify")
        .await;
    let reduced = login(&mock, "beta@example.test").await;
    assert!(!reduced["scope"].as_str().unwrap().contains("gmail"));
    assert_eq!(
        get(
            &mock,
            reduced["access_token"].as_str().unwrap(),
            "/gmail/v1/users/me/profile"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for redirect in [
        "https://example.com/callback",
        "http://127.0.0.1.evil.test/callback",
        "http://user@127.0.0.1/callback",
    ] {
        let response = client()
            .get(format!("{}/o/oauth2/v2/auth", mock.base_url()))
            .query(&[
                ("client_id", "nuncio-test-client"),
                ("redirect_uri", redirect),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.headers().get("location").is_none());
    }
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn gmail_wire_formats_mime_attachments_and_scoped_reusable_pages() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let a = login(&mock, "alpha@example.test").await;
    let b = login(&mock, "beta@example.test").await;
    let token = a["access_token"].as_str().unwrap();
    let other = b["access_token"].as_str().unwrap();
    let profile = json_ok(get(&mock, token, "/gmail/v1/users/me/profile").await).await;
    assert_eq!(profile["messagesTotal"], 3);
    assert_eq!(profile["threadsTotal"], 2);
    let list = json_ok(get(&mock, token, "/gmail/v1/users/me/messages?maxResults=100").await).await;
    assert_eq!(list["messages"].as_array().unwrap().len(), 2);
    assert!(list["messages"][0].get("payload").is_none());
    let next = list["nextPageToken"].as_str().unwrap();
    let path = format!("/gmail/v1/users/me/messages?maxResults=100&pageToken={next}");
    let page = json_ok(get(&mock, token, &path).await).await;
    assert_eq!(page["messages"].as_array().unwrap().len(), 1);
    assert!(page.get("nextPageToken").is_none());
    assert_eq!(json_ok(get(&mock, token, &path).await).await, page);
    assert_eq!(
        get(&mock, other, &path).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&mock, token, &format!("{path}&labelIds=INBOX"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let raw =
        json_ok(get(&mock, token, "/gmail/v1/users/me/messages/m-001?format=raw").await).await;
    let bytes = URL_SAFE_NO_PAD
        .decode(raw["raw"].as_str().unwrap())
        .unwrap();
    let expected = include_str!("../fixtures/mime/multipart.eml")
        .replace("ACCOUNT", "alpha@example.test")
        .replace('\n', "\r\n")
        .into_bytes();
    assert_eq!(bytes, expected);
    assert!(raw.get("payload").is_none());
    assert!(raw["internalDate"].is_string());
    let full = json_ok(
        get(
            &mock,
            token,
            "/gmail/v1/users/me/messages/m-001?format=full",
        )
        .await,
    )
    .await;
    assert!(full.get("raw").is_none());
    assert_eq!(
        full["labelIds"],
        serde_json::json!(["INBOX", "Label_project", "UNREAD"])
    );
    assert_eq!(full["payload"]["parts"][1]["filename"], "sample.pdf");
    let attachment = full["payload"]["parts"][1]["body"]["attachmentId"]
        .as_str()
        .unwrap();
    let attachment = json_ok(
        get(
            &mock,
            token,
            &format!("/gmail/v1/users/me/messages/m-001/attachments/{attachment}"),
        )
        .await,
    )
    .await;
    assert_eq!(
        URL_SAFE_NO_PAD
            .decode(attachment["data"].as_str().unwrap())
            .unwrap(),
        b"%PDF-1.4\nSynthetic PDF fixture\n%%EOF\n"
    );
    let metadata = json_ok(
        get(
            &mock,
            token,
            "/gmail/v1/users/me/messages/m-001?format=metadata&metadataHeaders=Subject",
        )
        .await,
    )
    .await;
    assert_eq!(metadata["payload"]["headers"].as_array().unwrap().len(), 1);
    assert!(metadata["payload"].get("parts").is_none());
    let thread =
        json_ok(get(&mock, token, "/gmail/v1/users/me/threads/t-001?format=full").await).await;
    assert_eq!(thread["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        get(&mock, token, "/gmail/v1/users/beta@example.test/profile")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&mock, token, "/gmail/v1/users/me/messages?maxresults=1")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&mock, token, "/gmail/v1/users/me/messages?maxResults=zero")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn optional_gmail_wire_fields_can_be_absent_without_changing_other_accounts() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let auth = login(&mock, "alpha@example.test").await;
    let token = auth["access_token"].as_str().unwrap();
    let fields = [
        "raw",
        "threadId",
        "historyId",
        "internalDate",
        "labelIds",
        "payload",
        "sizeEstimate",
    ];
    mock.control()
        .omit_message_fields("alpha@example.test", "m-003", &fields)
        .await
        .unwrap();
    let response =
        json_ok(get(&mock, token, "/gmail/v1/users/me/messages/m-003?format=raw").await).await;
    assert_eq!(response, serde_json::json!({"id":"m-003"}));
    assert!(mock
        .control()
        .omit_message_fields("alpha@example.test", "m-003", &["id"])
        .await
        .is_err());
    let auth = login(&mock, "beta@example.test").await;
    let response = json_ok(
        get(
            &mock,
            auth["access_token"].as_str().unwrap(),
            "/gmail/v1/users/me/messages/m-003?format=raw",
        )
        .await,
    )
    .await;
    assert!(response["raw"].is_string());
    mock.control()
        .omit_message_fields("alpha@example.test", "m-003", &[])
        .await
        .unwrap();
    let response =
        json_ok(get(&mock, token, "/gmail/v1/users/me/messages/m-003?format=raw").await).await;
    assert!(response["raw"].is_string());
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn calendar_removal_and_two_day_fixture_are_independent_remote_state() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let a = login(&mock, "alpha@example.test").await;
    let token = a["access_token"].as_str().unwrap();
    mock.control()
        .omit_calendar_fields("alpha@example.test", "primary", &["timeZone"])
        .await
        .unwrap();
    let catalog = json_ok(get(&mock, token, "/calendar/v3/users/me/calendarList").await).await;
    assert!(catalog["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["primary"] == true)
        .unwrap()
        .get("timeZone")
        .is_none());
    let canonical = json_ok(get(&mock, token, "/calendar/v3/calendars/primary/events").await).await;
    assert_eq!(canonical["timeZone"], "America/Chicago");
    let body: Value =
        serde_json::from_str(include_str!("../fixtures/calendar/all-day-2.json")).unwrap();
    mock.control()
        .put_event("alpha@example.test", "primary", body)
        .await
        .unwrap();
    for (from, to, count) in [
        ("2026-10-31T00:00:00-05:00", "2026-11-01T00:00:00-05:00", 1),
        ("2026-11-01T00:00:00-05:00", "2026-11-02T00:00:00-06:00", 1),
        ("2026-11-02T00:00:00-06:00", "2026-11-03T00:00:00-06:00", 0),
    ] {
        let mut url = url::Url::parse(&format!(
            "{}/calendar/v3/calendars/primary/events",
            mock.base_url()
        ))
        .unwrap();
        url.query_pairs_mut().extend_pairs([
            ("singleEvents", "true"),
            ("showDeleted", "true"),
            ("timeMin", from),
            ("timeMax", to),
        ]);
        let response = client().get(url).bearer_auth(token).send().await.unwrap();
        let values = json_ok(response).await;
        assert_eq!(
            values["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v["id"] == "alldate02")
                .count(),
            count
        );
    }
    mock.control()
        .remove_calendar("alpha@example.test", "team-alpha@example.test")
        .await
        .unwrap();
    let list = json_ok(get(&mock, token, "/calendar/v3/users/me/calendarList").await).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        get(
            &mock,
            token,
            "/calendar/v3/calendars/team-alpha%40example.test/events"
        )
        .await
        .status(),
        404
    );
    let b = login(&mock, "beta@example.test").await;
    let list = json_ok(
        get(
            &mock,
            b["access_token"].as_str().unwrap(),
            "/calendar/v3/users/me/calendarList",
        )
        .await,
    )
    .await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    mock.shutdown().await.unwrap();
}

#[tokio::test]
async fn gmail_label_history_and_send_semantics_are_independent() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let a = login(&mock, "alpha@example.test").await;
    let token = a["access_token"].as_str().unwrap();
    let initial = json_ok(get(&mock, token, "/gmail/v1/users/me/profile").await).await;
    let cursor = initial["historyId"].as_str().unwrap();
    let labels = json_ok(get(&mock, token, "/gmail/v1/users/me/labels").await).await;
    assert!(labels["labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["id"] == "Label_project" && l["type"] == "user"));
    let changed = json_ok(
        post(
            &mock,
            token,
            "/gmail/v1/users/me/messages/m-001/modify",
            serde_json::json!({
                "addLabelIds":["STARRED"],"removeLabelIds":["INBOX","UNREAD"]
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        changed["labelIds"],
        serde_json::json!(["Label_project", "STARRED"])
    );
    for invalid in [
        serde_json::json!({"addLabels":["STARRED"]}),
        serde_json::json!({"addLabelIds":[2]}),
        serde_json::json!({"addLabelIds":["not-a-label"]}),
    ] {
        assert_eq!(
            post(
                &mock,
                token,
                "/gmail/v1/users/me/messages/m-001/modify",
                invalid
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    json_ok(
        post(
            &mock,
            token,
            "/gmail/v1/users/me/messages/m-001/trash",
            serde_json::json!({}),
        )
        .await,
    )
    .await;
    json_ok(
        post(
            &mock,
            token,
            "/gmail/v1/users/me/messages/m-001/untrash",
            serde_json::json!({}),
        )
        .await,
    )
    .await;
    let history = json_ok(
        get(
            &mock,
            token,
            &format!("/gmail/v1/users/me/history?startHistoryId={cursor}&maxResults=500"),
        )
        .await,
    )
    .await;
    assert_eq!(history["history"].as_array().unwrap().len(), 2);
    assert_eq!(
        history["history"][0]["labelsAdded"][0]["labelIds"],
        serde_json::json!(["STARRED"])
    );
    assert!(history["history"][0]["id"].is_string());
    let next = history["nextPageToken"].as_str().unwrap();
    assert_eq!(
        get(
            &mock,
            token,
            &format!("/gmail/v1/users/me/history?startHistoryId={next}")
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let final_page = json_ok(get(&mock,token,&format!("/gmail/v1/users/me/history?startHistoryId={cursor}&maxResults=500&pageToken={next}")).await).await;
    assert!(final_page.get("nextPageToken").is_none());
    assert!(
        final_page["historyId"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > cursor.parse().unwrap()
    );
    let send_bytes = b"From: alpha@example.test\r\nTo: recipient@example.test\r\nSubject: Sent once\r\nMessage-ID: <frozen@example.test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nFrozen content\r\n";
    let payload = serde_json::json!({"raw":URL_SAFE_NO_PAD.encode(send_bytes)});
    let first = json_ok(
        post(
            &mock,
            token,
            "/gmail/v1/users/me/messages/send",
            payload.clone(),
        )
        .await,
    )
    .await;
    let second =
        json_ok(post(&mock, token, "/gmail/v1/users/me/messages/send", payload).await).await;
    assert_ne!(
        first["id"], second["id"],
        "Gmail does not promise Message-ID idempotency"
    );
    let accepted = mock.control().accepted_sends("alpha@example.test").await;
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].raw, send_bytes);
    assert_eq!(
        mock.control().snapshot().await.mail["beta@example.test"]
            .messages
            .len(),
        3
    );
    let search = json_ok(
        get(
            &mock,
            token,
            "/gmail/v1/users/me/messages?q=rfc822msgid%3Afrozen%40example.test",
        )
        .await,
    )
    .await;
    assert_eq!(search["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        post(
            &mock,
            token,
            "/gmail/v1/users/me/messages/send",
            serde_json::json!({"raw":[1,2]})
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&mock, token, "/_control/snapshot").await.status(),
        StatusCode::NOT_FOUND
    );
    mock.shutdown().await.unwrap();
}

const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyz0123456789-._~ABCDE";
const SCOPES: &str =
    "https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/calendar";

fn client() -> Client {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}

async fn authorize(mock: &MockGoogle, account: &str) -> Url {
    let response = client()
        .get(format!("{}/o/oauth2/v2/auth", mock.base_url()))
        .query(&[
            ("client_id", "nuncio-test-client"),
            ("redirect_uri", "http://127.0.0.1:8089/callback"),
            ("response_type", "code"),
            ("state", "state-with-special&characters"),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER)),
            ),
            ("scope", SCOPES),
            ("access_type", "offline"),
            ("login_hint", account),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    Url::parse(response.headers()["location"].to_str().unwrap()).unwrap()
}

async fn exchange(mock: &MockGoogle, code: &str, verifier: &str) -> reqwest::Response {
    client()
        .post(format!("{}/token", mock.base_url()))
        .form(&[
            ("client_id", "nuncio-test-client"),
            ("grant_type", "authorization_code"),
            ("redirect_uri", "http://127.0.0.1:8089/callback"),
            ("code", code),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .unwrap()
}

async fn login(mock: &MockGoogle, account: &str) -> Value {
    let redirect = authorize(mock, account).await;
    assert_eq!(
        redirect
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1,
        "state-with-special&characters"
    );
    let code = redirect
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let response = exchange(mock, &code, VERIFIER).await;
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

#[tokio::test]
async fn oauth_uses_state_pkce_one_use_codes_and_real_account_identity() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let redirect = authorize(&mock, "alpha@example.test").await;
    assert_eq!(
        redirect
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1,
        "state-with-special&characters"
    );
    let code = redirect
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let wrong = exchange(&mock, &code, "wrong").await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        wrong.json::<Value>().await.unwrap()["error"],
        "invalid_grant"
    );
    let redirect = authorize(&mock, "alpha@example.test").await;
    let code = redirect
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let response = exchange(&mock, &code, VERIFIER).await;
    assert_eq!(response.status(), StatusCode::OK);
    let grant: Value = response.json().await.unwrap();
    assert!(grant["expires_in"].is_number());
    assert!(grant["refresh_token"].is_string());
    assert_eq!(
        exchange(&mock, &code, VERIFIER).await.status(),
        StatusCode::BAD_REQUEST
    );
    let profile = client()
        .get(format!("{}/gmail/v1/users/me/profile", mock.base_url()))
        .bearer_auth(grant["access_token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(profile.status(), StatusCode::OK);
    let profile: Value = profile.json().await.unwrap();
    assert_eq!(profile["emailAddress"], "alpha@example.test");
    assert!(
        profile["historyId"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 9_007_199_254_740_992
    );
    let second = login(&mock, "beta@example.test").await;
    assert_ne!(grant["access_token"], second["access_token"]);
    mock.shutdown().await.unwrap();
}
#[path = "support/calendar_permissions_contract.rs"]
mod calendar_permissions_contract;

#[tokio::test]
async fn freebusy_contract_reports_partial_coverage_and_never_changes_provider_state() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let grant = login(&mock, "alpha@example.test").await;
    let token = grant["access_token"].as_str().unwrap();
    let ctl = mock.control();
    for (id, transparency) in [("busy001", "opaque"), ("free001", "transparent")] {
        ctl.put_event("alpha@example.test","alpha@example.test",serde_json::json!({"id":id,"start":{"dateTime":"2026-10-02T09:00:00-05:00"},"end":{"dateTime":"2026-10-02T11:00:00-05:00"},"transparency":transparency})).await.unwrap();
    }
    let before = ctl.snapshot().await;
    let request = serde_json::json!({"timeMin":"2026-10-02T14:30:00Z","timeMax":"2026-10-02T15:30:00Z","items":[{"id":"primary"},{"id":"team-alpha@example.test"},{"id":"missing@example.test"}]});
    let reply = json_ok(post(&mock, token, "/calendar/v3/freeBusy", request.clone()).await).await;
    assert_eq!(
        reply["calendars"]["primary"]["busy"],
        serde_json::json!([{"start":"2026-10-02T14:30:00+00:00","end":"2026-10-02T15:30:00+00:00"}])
    );
    assert_eq!(
        reply["calendars"]["team-alpha@example.test"]["busy"],
        serde_json::json!([])
    );
    assert_eq!(
        reply["calendars"]["missing@example.test"]["errors"],
        serde_json::json!([{"domain":"global","reason":"notFound"}])
    );
    for invalid in [
        serde_json::json!({"timeMin":"bad","timeMax":"2026-10-03T00:00:00Z","items":[]}),
        serde_json::json!({"timeMin":"2026-10-03T00:00:00Z","timeMax":"2026-10-02T00:00:00Z","items":[]}),
        serde_json::json!({"timeMin":"2026-10-02T00:00:00Z","timeMax":"2026-10-03T00:00:00Z","items":[{"id":"primary","typo":true}]}),
        serde_json::json!({"timeMin":"2026-10-02T00:00:00Z","timeMax":"2026-10-03T00:00:00Z","items":(0..51).map(|i|serde_json::json!({"id":format!("id{i}")})).collect::<Vec<_>>()}),
    ] {
        assert_eq!(
            post(&mock, token, "/calendar/v3/freeBusy", invalid)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let after = ctl.snapshot().await;
    for id in ["alpha@example.test", "team-alpha@example.test"] {
        assert_eq!(
            after.calendars["alpha@example.test"][id].events,
            before.calendars["alpha@example.test"][id].events
        );
        assert_eq!(
            after.calendars["alpha@example.test"][id].version,
            before.calendars["alpha@example.test"][id].version
        );
        assert_eq!(
            after.calendars["alpha@example.test"][id]
                .notifications
                .len(),
            before.calendars["alpha@example.test"][id]
                .notifications
                .len()
        );
    }
    mock.shutdown().await.unwrap();
}
