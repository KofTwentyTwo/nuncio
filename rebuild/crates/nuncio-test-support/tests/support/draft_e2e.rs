use super::{E2eHarness, Seed};
use serde_json::json;

#[tokio::test]
async fn paused_send_queue_is_idempotent_after_daemon_death_and_draft_deletion() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let path = h.artifacts.join("queued-draft.json");
    std::fs::write(&path,br#"{"to":[{"address":"recipient@example.test"}],"subject":"Queued intent","text":"Frozen body"}"#).unwrap();
    let saved = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            path.to_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(saved.status, 0);
    let draft = saved.json().unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        h.cli(&["--json", "account", "disconnect", "--account", &account])
            .await
            .unwrap()
            .status,
        0
    );
    let request_id = "f928d572-0e79-4d65-8a39-dd915dba54af";
    let send = [
        "--json",
        "mail",
        "send",
        "--account",
        &account,
        "--draft",
        &draft,
        "--request-id",
        request_id,
    ];
    let queued = h.cli(&send).await.unwrap();
    assert_eq!(queued.status, 0);
    let operation = queued.json().unwrap()["result"].clone();
    assert_eq!(operation["state"], "queued");
    let id = operation["id"].as_str().unwrap();
    let list = h
        .cli(&[
            "--json",
            "operation",
            "list",
            "--account",
            &account,
            "--page-size",
            "1",
        ])
        .await
        .unwrap();
    assert_eq!(list.status, 0);
    assert_eq!(list.json().unwrap()["result"]["items"][0]["id"], id);
    let attempts = h
        .cli(&[
            "--json",
            "operation",
            "attempts",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap();
    assert_eq!(attempts.status, 0);
    assert!(attempts.json().unwrap()["result"]["items"]
        .as_array()
        .unwrap()
        .is_empty());
    let resolution = h.artifacts.join("resolution.json");
    std::fs::write(
        &resolution,
        br#"{"decision":"abandon","reason":"No longer wanted"}"#,
    )
    .unwrap();
    let resolve = [
        "--json",
        "operation",
        "resolve",
        "--account",
        &account,
        "--operation",
        id,
        "--version",
        "1",
        "--file",
        resolution.to_str().unwrap(),
    ];
    let rejected = h.cli(&resolve).await.unwrap();
    assert_eq!(rejected.status, 5, "queued work must use cancellation");
    std::fs::write(&resolution,br#"{"decision":"resend","request_id":"9a0d3d71-1442-4681-b18d-326aec75decc","reason":"Explicit new submission"}"#).unwrap();
    let rejected = h.cli(&resolve).await.unwrap();
    assert_eq!(rejected.status, 2);
    assert_eq!(
        rejected.json().unwrap()["error"]["code"],
        "duplicate_risk_not_accepted"
    );
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("may duplicate"));
    let mut approved = resolve.to_vec();
    approved.push("--accept-duplicate-risk");
    assert_eq!(
        h.cli(&approved).await.unwrap().status,
        5,
        "risk acknowledgement does not bypass operation state"
    );
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let shown = h
        .cli(&[
            "--json",
            "operation",
            "show",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap();
    assert_eq!(shown.status, 0);
    assert_eq!(shown.json().unwrap()["result"], operation);
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "delete",
            "--account",
            &account,
            "--draft",
            &draft,
            "--version",
            "1"
        ])
        .await
        .unwrap()
        .status,
        0
    );
    let repeated = h.cli(&send).await.unwrap();
    assert_eq!(repeated.status, 0);
    assert_eq!(repeated.json().unwrap()["result"], operation);
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            request_id,
            "--version",
            "1"
        ])
        .await
        .unwrap()
        .status,
        5
    );
    let cancelled = h
        .cli(&[
            "--json",
            "operation",
            "cancel",
            "--account",
            &account,
            "--operation",
            id,
        ])
        .await
        .unwrap();
    assert_eq!(cancelled.status, 0);
    assert_eq!(cancelled.json().unwrap()["result"]["state"], "cancelled");
    let repeated = h.cli(&send).await.unwrap();
    assert_eq!(repeated.status, 0);
    assert_eq!(repeated.json().unwrap()["result"]["state"], "cancelled");
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn reply_all_and_forward_use_offline_originals_and_preserve_mime_context_after_restart() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let raw = include_str!("../../fixtures/mime/reply-forward.eml")
        .replace('\n', "\r\n")
        .into_bytes();
    h.google
        .control()
        .add_message(
            "alpha@example.test",
            "prepare-001",
            "prepare-thread",
            raw,
            ["INBOX".into()].into(),
        )
        .await
        .unwrap();
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await
            .unwrap()
            .status,
        0
    );
    let listed = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let message = listed["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["provider_id"] == "prepare-001")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.google.stop().await.unwrap();
    let before = h.google.control().snapshot().await.requests;
    let body = h.artifacts.join("response.txt");
    std::fs::write(&body, "My response").unwrap();
    let reply = h
        .cli(&[
            "--json",
            "mail",
            "reply-all",
            "--account",
            &account,
            "--message",
            &message,
            "--body-file",
            body.to_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(reply.status, 0);
    let reply = reply.json().unwrap()["result"].clone();
    assert_eq!(
        reply["content"]["to"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["address"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "alice@example.test",
            "bob@example.test",
            "carol@example.test"
        ]
    );
    assert_eq!(reply["content"]["cc"][0]["address"], "david@example.test");
    assert!(reply["content"]["bcc"].as_array().unwrap().is_empty());
    assert_eq!(
        reply["context"]["references"],
        json!([
            "root@example.test",
            "parent@example.test",
            "incoming@example.test"
        ])
    );
    let single = h
        .cli(&[
            "--json",
            "mail",
            "reply",
            "--account",
            &account,
            "--message",
            &message,
            "--body-file",
            body.to_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(single.status, 0);
    assert_eq!(
        single.json().unwrap()["result"]["content"]["to"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let forwarded = h
        .cli(&[
            "--json",
            "mail",
            "forward",
            "--account",
            &account,
            "--message",
            &message,
            "--to",
            "forward@example.test",
            "--to",
            "second@example.test",
        ])
        .await
        .unwrap();
    assert_eq!(forwarded.status, 0);
    let forwarded = forwarded.json().unwrap()["result"].clone();
    assert_eq!(forwarded["content"]["to"].as_array().unwrap().len(), 2);
    assert!(forwarded["content"]["text"]
        .as_str()
        .unwrap()
        .contains("Original plain body."));
    assert!(forwarded["content"]["html"]
        .as_str()
        .unwrap()
        .contains("cid:logo@example.test"));
    assert!(forwarded["context"]["thread_id"].is_null());
    assert_eq!(forwarded["attachments"].as_array().unwrap().len(), 2);
    assert_eq!(
        forwarded["attachments"][0]["parameters"]["charset"],
        "windows-1252"
    );
    use sha2::{Digest, Sha256};
    assert_eq!(
        forwarded["attachments"][0]["sha256"],
        format!("{:x}", Sha256::digest(b"caf\xe9"))
    );
    assert_eq!(
        forwarded["attachments"][1]["content_id"],
        "logo@example.test"
    );
    assert_eq!(forwarded["attachments"][1]["disposition"], "inline");
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    for draft in [&reply, &forwarded] {
        let read = h
            .cli(&[
                "--json",
                "mail",
                "draft",
                "show",
                "--account",
                &account,
                "--draft",
                draft["id"].as_str().unwrap(),
            ])
            .await
            .unwrap();
        assert_eq!(read.status, 0);
        assert_eq!(read.json().unwrap()["result"], *draft);
    }
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await.requests).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_drafts_attachments_and_versions_survive_daemon_death_and_full_provider_reset() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let other = h.connect_google("beta@example.test").await.unwrap();
    let source = h.artifacts.join("draft.json");
    let input = json!({"to":[{"address":"recipient@example.test","name":"Zoë Recipient"},{"address":"second@example.test","name":null}],"cc":[{"address":"copy@example.test"}],"bcc":[{"address":"private@example.test"}],"subject":"Durable draft","text":"Original plain body","html":"<p>Original <strong>HTML</strong> body</p>"});
    std::fs::write(&source, serde_json::to_vec(&input).unwrap()).unwrap();
    let created = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            source.to_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(
        created.status, 0,
        "actual CLI must save a durable local draft"
    );
    let created = created.json().unwrap();
    let draft = created["result"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["result"]["version"], 1);
    assert_eq!(created["result"]["account_id"], account);
    assert_eq!(created["result"]["content"]["to"], input["to"]);
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--draft",
            &draft,
            "--version",
            "9",
            "--file",
            source.to_str().unwrap()
        ])
        .await
        .unwrap()
        .status,
        5,
        "stale draft edit must conflict"
    );
    let attachment = h.artifacts.join("private-attachment.bin");
    let bytes = (0..600_000).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    std::fs::write(&attachment, &bytes).unwrap();
    let attached = h
        .cli(&[
            "--json",
            "mail",
            "draft",
            "attach",
            "--account",
            &account,
            "--draft",
            &draft,
            "--version",
            "1",
            "--file",
            attachment.to_str().unwrap(),
            "--mime-type",
            "application/octet-stream",
        ])
        .await
        .unwrap();
    assert_eq!(attached.status, 0);
    let attached = attached.json().unwrap();
    assert_eq!(attached["result"]["version"], 2);
    assert_eq!(attached["result"]["attachments"][0]["byte_length"], 600_000);
    use sha2::{Digest, Sha256};
    assert_eq!(
        attached["result"]["attachments"][0]["sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    for synchronize in [false, true] {
        if synchronize {
            assert_eq!(
                h.cli(&["--json", "sync", "--account", &account, "--full", "--wait"])
                    .await
                    .unwrap()
                    .status,
                0
            );
        }
        let read = h
            .cli(&[
                "--json",
                "mail",
                "draft",
                "show",
                "--account",
                &account,
                "--draft",
                &draft,
            ])
            .await
            .unwrap();
        assert_eq!(read.status, 0);
        assert_eq!(
            read.json().unwrap()["result"],
            attached["result"],
            "provider projection reset modified the local draft"
        );
    }
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "show",
            "--account",
            &other,
            "--draft",
            &draft
        ])
        .await
        .unwrap()
        .status,
        4
    );
    let list = h
        .cli(&["--json", "mail", "draft", "list", "--account", &account])
        .await
        .unwrap();
    assert_eq!(list.status, 0);
    assert_eq!(list.json().unwrap()["result"]["items"][0]["id"], draft);
    let mut invalid = input.clone();
    invalid["to"][0]["address"] = json!("recipient@example.test\r\nBcc: injected@example.test");
    std::fs::write(&source, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "save",
            "--account",
            &account,
            "--file",
            source.to_str().unwrap()
        ])
        .await
        .unwrap()
        .status,
        2
    );
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "delete",
            "--account",
            &account,
            "--draft",
            &draft,
            "--version",
            "1"
        ])
        .await
        .unwrap()
        .status,
        5
    );
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "delete",
            "--account",
            &account,
            "--draft",
            &draft,
            "--version",
            "2"
        ])
        .await
        .unwrap()
        .status,
        0
    );
    assert_eq!(
        h.cli(&[
            "--json",
            "mail",
            "draft",
            "show",
            "--account",
            &account,
            "--draft",
            &draft
        ])
        .await
        .unwrap()
        .status,
        4
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}
