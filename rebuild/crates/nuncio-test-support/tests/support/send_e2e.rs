use super::{E2eHarness, Seed};
use mailparse::MailHeaderMap;

#[tokio::test]
async fn send_retry_after_seconds_and_date_survive_crash_and_wake_without_blocking_another_account()
{
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for dated in [false, true] {
        let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
        let alpha = h.connect_google("alpha@example.test").await.unwrap();
        let beta = h.connect_google("beta@example.test").await.unwrap();
        let file = h.artifacts.join("backoff-draft.json");
        std::fs::write(&file,br#"{"to":[{"address":"recipient@example.test"}],"subject":"Backoff","text":"One submission"}"#).unwrap();
        let mut drafts = Vec::new();
        for account in [&alpha, &beta] {
            let saved = h
                .cli(&[
                    "--json",
                    "mail",
                    "draft",
                    "save",
                    "--account",
                    account,
                    "--file",
                    file.to_str().unwrap(),
                ])
                .await
                .unwrap();
            assert_eq!(saved.status, 0);
            drafts.push(
                saved.json().unwrap()["result"]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        let action = if dated {
            FaultAction::StatusWithRetryDate {
                code: 429,
                retry_after: chrono::DateTime::from_timestamp_millis(1772899200000)
                    .unwrap()
                    .format("%a, %d %b %Y %H:%M:%S GMT")
                    .to_string(),
            }
        } else {
            FaultAction::Status {
                code: 429,
                retry_after_secs: Some(3600),
            }
        };
        h.google
            .control()
            .inject(Fault {
                method: "POST".into(),
                path: "/gmail/v1/users/me/messages/send".into(),
                account: Some("alpha@example.test".into()),
                call: None,
                phase: Phase::Before,
                action,
            })
            .await;
        let send = [
            "--json",
            "mail",
            "send",
            "--account",
            &alpha,
            "--draft",
            &drafts[0],
            "--request-id",
            "b954d3ec-7f7a-4b31-9fca-e584de17cb52",
        ];
        let queued = h.cli(&send).await.unwrap();
        assert_eq!(queued.status, 0);
        let queued = queued.json().unwrap();
        let id = queued["result"]["id"].as_str().unwrap();
        let show = [
            "--json",
            "operation",
            "show",
            "--account",
            &alpha,
            "--operation",
            id,
        ];
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
        let waiting = loop {
            let shown = h.cli(&show).await.unwrap().json().unwrap();
            if shown["result"]["state"] == "retry_wait" {
                break shown["result"].clone();
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert!(waiting["next_attempt_at_ms"].as_i64().unwrap() >= 1772899200000);
        if dated {
            assert_eq!(waiting["next_attempt_at_ms"], 1772899200000_i64);
        }
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            0
        );
        h.force_kill().await.unwrap();
        h.restart().await.unwrap();
        let unchanged = h.cli(&show).await.unwrap().json().unwrap();
        assert_eq!(unchanged["result"], waiting);
        let other = h
            .cli(&[
                "--json",
                "mail",
                "send",
                "--account",
                &beta,
                "--draft",
                &drafts[1],
                "--request-id",
                "b954d3ec-7f7a-4b31-9fca-e584de17cb52",
                "--wait",
            ])
            .await
            .unwrap();
        assert_eq!(other.status, 0);
        assert_eq!(
            h.google
                .control()
                .accepted_sends("beta@example.test")
                .await
                .len(),
            1
        );
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            0
        );
        let snapshot = h.google.control().snapshot().await;
        assert_eq!(
            snapshot
                .requests
                .iter()
                .filter(|r| r.method == "POST"
                    && r.path.ends_with("/messages/send")
                    && r.account.as_deref() == Some("alpha@example.test"))
                .map(|r| r.count)
                .sum::<u64>(),
            1
        );
        std::fs::write(h.artifacts.join("barriers/clock-offset-ms"), "3602000").unwrap();
        h.google
            .control()
            .advance(std::time::Duration::from_secs(3602))
            .await;
        let mut waited = send.to_vec();
        waited.push("--wait");
        let result = h.cli(&waited).await.unwrap();
        assert_eq!(result.status, 0, "{:?}", result.json());
        assert_eq!(result.json().unwrap()["result"]["id"], id);
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            1
        );
        assert_eq!(
            h.google
                .control()
                .accepted_sends("beta@example.test")
                .await
                .len(),
            1
        );
        h.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn actual_cli_reply_reply_all_and_forward_submit_correct_mime_once_under_concurrent_retries()
{
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let raw = include_str!("../../fixtures/mime/reply-forward.eml")
        .replace('\n', "\r\n")
        .into_bytes();
    h.google
        .control()
        .add_message(
            "alpha@example.test",
            "send-source",
            "source-thread",
            raw,
            ["INBOX".into()].into(),
        )
        .await
        .unwrap();
    let source = h.google.control().snapshot().await.mail["alpha@example.test"].messages
        ["send-source"]
        .clone();
    assert_eq!(
        h.cli(&["--json", "sync", "--account", &account, "--wait"])
            .await
            .unwrap()
            .status,
        0
    );
    let list = h
        .cli(&["--json", "mail", "list", "--account", &account])
        .await
        .unwrap()
        .json()
        .unwrap();
    let message = list["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["provider_id"] == "send-source")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let body = h.artifacts.join("reply-body.txt");
    std::fs::write(&body, "My actual response").unwrap();
    let attachment = h.artifacts.join("extra.bin");
    let bytes = [0, 1, 255, 13, 10, 128];
    std::fs::write(&attachment, bytes).unwrap();
    for (index, kind) in ["reply", "reply-all", "forward"].iter().enumerate() {
        let mut prepare = vec![
            "--json",
            "mail",
            kind,
            "--account",
            &account,
            "--message",
            message,
            "--body-file",
            body.to_str().unwrap(),
        ];
        if *kind == "forward" {
            prepare.extend(["--to", "recipient@example.test"]);
        }
        let draft = h.cli(&prepare).await.unwrap();
        assert_eq!(draft.status, 0);
        let draft = draft.json().unwrap();
        let id = draft["result"]["id"].as_str().unwrap();
        assert_eq!(
            h.cli(&[
                "--json",
                "mail",
                "draft",
                "attach",
                "--account",
                &account,
                "--draft",
                id,
                "--version",
                "1",
                "--file",
                attachment.to_str().unwrap()
            ])
            .await
            .unwrap()
            .status,
            0
        );
        let request = format!("7f568a7b-dff0-4560-b7f7-{index:012}");
        let send = [
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            id,
            "--request-id",
            &request,
            "--version",
            "2",
            "--wait",
        ];
        let (a, b, c, d) = tokio::join!(h.cli(&send), h.cli(&send), h.cli(&send), h.cli(&send));
        let mut identity = None;
        for output in [a, b, c, d] {
            let output = output.unwrap();
            assert_eq!(output.status, 0, "{kind}: {:?}", output.json());
            let result = output.json().unwrap();
            let id = result["result"]["id"].clone();
            if let Some(original) = &identity {
                assert_eq!(original, &id);
            } else {
                identity = Some(id);
            }
            assert_eq!(result["result"]["state"], "applied");
        }
        let snapshot = h.google.control().snapshot().await;
        let sends = &snapshot.mail["alpha@example.test"].accepted_sends;
        assert_eq!(sends.len(), index + 1);
        let accepted = sends.last().unwrap();
        let remote = &snapshot.mail["alpha@example.test"].messages[&accepted.id];
        let parsed = mailparse::parse_mail(&accepted.raw).unwrap();
        let addresses = |name: &str| {
            let Some(header) = parsed.headers.get_first_header(name) else {
                return std::collections::BTreeSet::new();
            };
            let addresses = mailparse::addrparse_header(header).unwrap();
            addresses
                .iter()
                .flat_map(|a| match a {
                    mailparse::MailAddr::Single(a) => vec![a.addr.clone()],
                    mailparse::MailAddr::Group(g) => {
                        g.addrs.iter().map(|a| a.addr.clone()).collect()
                    }
                })
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(addresses("From"), ["alpha@example.test".to_owned()].into());
        let expected_to: std::collections::BTreeSet<String> = match *kind {
            "reply" => ["alice@example.test", "bob@example.test"]
                .map(String::from)
                .into(),
            "reply-all" => [
                "alice@example.test",
                "bob@example.test",
                "carol@example.test",
            ]
            .map(String::from)
            .into(),
            _ => ["recipient@example.test".to_owned()].into(),
        };
        assert_eq!(addresses("To"), expected_to);
        assert!(addresses("Bcc").is_empty());
        assert_eq!(
            addresses("Cc"),
            if *kind == "reply-all" {
                ["david@example.test".to_owned()].into()
            } else {
                std::collections::BTreeSet::new()
            }
        );
        assert!(remote.attachments.values().any(|a| a == &bytes));
        if *kind == "forward" {
            assert_ne!(accepted.thread_id, "source-thread");
            assert!(remote.in_reply_to.is_empty());
            assert!(remote.references.is_empty());
            for original in source.attachments.values() {
                assert!(remote.attachments.values().any(|a| a == original));
            }
            assert_eq!(remote.attachments.len(), source.attachments.len() + 1);
        } else {
            assert_eq!(accepted.thread_id, "source-thread");
            assert_eq!(remote.in_reply_to, "<incoming@example.test>");
            assert_eq!(
                remote.references.split_whitespace().collect::<Vec<_>>(),
                [
                    "<root@example.test>",
                    "<parent@example.test>",
                    "<incoming@example.test>"
                ]
            );
            assert_eq!(remote.attachments.len(), 1);
        }
        assert_eq!(snapshot.mail["beta@example.test"].accepted_sends.len(), 0);
    }
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn lost_google_send_acknowledgements_reconcile_or_require_audited_decisions_without_blind_resend(
) {
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    for decision in ["positive_read", "abandon", "confirm_applied", "resend"] {
        let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
        let account = h.connect_google("alpha@example.test").await.unwrap();
        let file = h.artifacts.join("send.json");
        std::fs::write(&file,br#"{"to":[{"address":"recipient@example.test"}],"subject":"Lost acknowledgement","text":"Keep this frozen"}"#).unwrap();
        let saved = h
            .cli(&[
                "--json",
                "mail",
                "draft",
                "save",
                "--account",
                &account,
                "--file",
                file.to_str().unwrap(),
            ])
            .await
            .unwrap();
        assert_eq!(saved.status, 0);
        let draft = saved.json().unwrap()["result"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        h.google
            .control()
            .inject(Fault {
                method: "POST".into(),
                path: "/gmail/v1/users/me/messages/send".into(),
                account: Some("alpha@example.test".into()),
                call: None,
                phase: Phase::After,
                action: FaultAction::Withhold {
                    barrier: "accepted-without-ack".into(),
                },
            })
            .await;
        let send = [
            "--json",
            "mail",
            "send",
            "--account",
            &account,
            "--draft",
            &draft,
            "--request-id",
            "9b4106a6-71de-4bc4-a337-8f35d3873e8a",
        ];
        let queued = h.cli(&send).await.unwrap();
        assert_eq!(queued.status, 0);
        let original = queued.json().unwrap()["result"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        h.google
            .control()
            .wait_for_barrier("accepted-without-ack")
            .await
            .unwrap();
        let accepted = h
            .google
            .control()
            .accepted_sends("alpha@example.test")
            .await;
        assert_eq!(accepted.len(), 1);
        h.force_kill().await.unwrap();
        h.google
            .control()
            .release_barrier("accepted-without-ack")
            .await;
        if decision != "positive_read" {
            h.google
                .control()
                .delete_message("alpha@example.test", &accepted[0].id)
                .await
                .unwrap();
        }
        h.restart().await.unwrap();
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
        let mut wait = send.to_vec();
        wait.push("--wait");
        let recovered = h.cli(&wait).await.unwrap();
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            1,
            "restart and repeated request identity must never resend"
        );
        if decision == "positive_read" {
            assert_eq!(recovered.status, 0);
            assert_eq!(recovered.json().unwrap()["result"]["state"], "applied");
            let history = h
                .cli(&[
                    "--json",
                    "operation",
                    "attempts",
                    "--account",
                    &account,
                    "--operation",
                    &original,
                ])
                .await
                .unwrap()
                .json()
                .unwrap();
            assert_eq!(history["result"]["items"][0]["kind"], "reconcile");
            assert_eq!(
                history["result"]["items"][0]["receipts"][0]["source"],
                "positive_read"
            );
            assert_eq!(
                history["result"]["items"][0]["receipts"][0]["provider_id"],
                accepted[0].id
            );
            h.shutdown().await.unwrap();
            continue;
        }
        assert_eq!(recovered.status, 5);
        let uncertain = recovered.json().unwrap()["error"]["operation"].clone();
        assert_eq!(uncertain["state"], "uncertain");
        assert_eq!(uncertain["id"], original);
        assert_eq!(uncertain["needs_reconciliation"], false);
        let version = uncertain["version"].as_u64().unwrap().to_string();
        let resolution = h.artifacts.join("decision.json");
        let decision_json = match decision {
            "abandon" => {
                serde_json::json!({"decision":"abandon","reason":"Keep the delivery outcome unresolved"})
            }
            "confirm_applied" => {
                serde_json::json!({"decision":"confirm_applied","evidence":{"provider_id":accepted[0].id,"message_id":accepted[0].message_id.trim_matches(['<','>']),"observed_at_ms":uncertain["updated_at_ms"],"note":"Independent acceptance log confirms this exact Message-ID"}})
            }
            "resend" => {
                serde_json::json!({"decision":"resend","request_id":"01efc2a3-a158-4125-a49c-7a5e58855414","reason":"Explicitly accept another submission"})
            }
            _ => unreachable!(),
        };
        std::fs::write(&resolution, serde_json::to_vec(&decision_json).unwrap()).unwrap();
        let mut resolve = vec![
            "--json",
            "operation",
            "resolve",
            "--account",
            &account,
            "--operation",
            &original,
            "--version",
            &version,
            "--file",
            resolution.to_str().unwrap(),
        ];
        if decision == "resend" {
            resolve.push("--accept-duplicate-risk");
        }
        let revision = h
            .cli(&["--json", "system", "status"])
            .await
            .unwrap()
            .json()
            .unwrap()["result"]["storage"]["revision"]
            .as_u64()
            .unwrap();
        let resolved = h.cli(&resolve).await.unwrap();
        assert_eq!(
            resolved.status, 0,
            "audited {decision} must work through the actual client"
        );
        let resolved = resolved.json().unwrap()["result"].clone();
        let count = if decision == "resend" { 2 } else { 1 };
        let watched = h
            .cli(&[
                "--json",
                "system",
                "watch",
                "--after",
                &revision.to_string(),
                "--limit",
                &count.to_string(),
            ])
            .await
            .unwrap();
        assert_eq!(watched.status, 0);
        let events = String::from_utf8(watched.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["result"].clone())
            .collect::<Vec<_>>();
        assert_eq!(events.len(), count);
        let mut ids = std::collections::BTreeSet::new();
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event["revision"], revision + index as u64 + 1);
            assert_eq!(event["kind"], "operation");
            assert_eq!(event["account_id"], account);
            ids.insert(event["resource_id"].as_str().unwrap().to_owned());
        }
        assert!(ids.contains(&original));
        if decision == "resend" {
            assert!(ids.contains(
                resolved["resolutions"][0]["replacement_id"]
                    .as_str()
                    .unwrap()
            ));
        }
        assert_eq!(resolved["resolutions"].as_array().unwrap().len(), 1);
        let expected_sends = if decision == "resend" { 2 } else { 1 };
        if decision == "resend" {
            assert_eq!(resolved["state"], "uncertain");
            let new_id = resolved["resolutions"][0]["replacement_id"]
                .as_str()
                .unwrap();
            let replacement = h
                .cli(&[
                    "--json",
                    "operation",
                    "wait",
                    "--account",
                    &account,
                    "--operation",
                    new_id,
                ])
                .await
                .unwrap();
            assert_eq!(replacement.status, 0);
            assert_eq!(replacement.json().unwrap()["result"]["state"], "applied");
            let sends = h
                .google
                .control()
                .accepted_sends("alpha@example.test")
                .await;
            assert_eq!(sends.len(), 2);
            assert_ne!(sends[0].message_id, sends[1].message_id);
            let first = mailparse::parse_mail(&sends[0].raw).unwrap();
            let second = mailparse::parse_mail(&sends[1].raw).unwrap();
            assert_eq!(
                first.get_body_raw().unwrap(),
                second.get_body_raw().unwrap()
            );
        } else if decision == "abandon" {
            assert_eq!(resolved["state"], "uncertain");
            assert_eq!(resolved["disposition"], "abandoned");
        } else {
            assert_eq!(resolved["state"], "applied");
            assert_eq!(resolved["disposition"], "manual_confirmed");
        }
        h.force_kill().await.unwrap();
        h.restart().await.unwrap();
        let retried = h.cli(&resolve).await.unwrap();
        assert_eq!(retried.status, 0);
        assert_eq!(retried.json().unwrap()["result"], resolved);
        assert_eq!(
            h.google
                .control()
                .accepted_sends("alpha@example.test")
                .await
                .len(),
            expected_sends
        );
        assert!(h
            .google
            .control()
            .accepted_sends("beta@example.test")
            .await
            .is_empty());
        h.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn actual_cli_sends_frozen_mime_once_and_exposes_provider_attempt_receipts() {
    let mut h = E2eHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = h.connect_google("alpha@example.test").await.unwrap();
    let path = h.artifacts.join("send.json");
    std::fs::write(&path,br#"{"to":[{"address":"recipient@example.test"}],"cc":[{"address":"copy@example.test"}],"bcc":[{"address":"private@example.test"}],"subject":"Actual subprocess submission","text":"Frozen send body"}"#).unwrap();
    let draft = h
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
    assert_eq!(draft.status, 0);
    let draft = draft.json().unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let attachment = h.artifacts.join("binary.dat");
    std::fs::write(&attachment, b"\0\xff\x10\r\nopaque\n").unwrap();
    assert_eq!(
        h.cli(&[
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
            attachment.to_str().unwrap()
        ])
        .await
        .unwrap()
        .status,
        0
    );
    let command = [
        "--json",
        "mail",
        "send",
        "--account",
        &account,
        "--draft",
        &draft,
        "--request-id",
        "6c6552cc-1877-46ba-aefe-389a1efb9c07",
        "--version",
        "2",
        "--wait",
    ];
    let sent = h.cli(&command).await.unwrap();
    assert_eq!(
        sent.status, 0,
        "--wait must confirm the durable provider outcome"
    );
    let op = sent.json().unwrap()["result"].clone();
    assert_eq!(op["state"], "applied");
    let sends = h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await;
    assert_eq!(
        sends.len(),
        1,
        "local success must correspond to one independent accepted send"
    );
    let wire = mailparse::parse_mail(&sends[0].raw).unwrap();
    for (header, address) in [
        ("From", "alpha@example.test"),
        ("To", "recipient@example.test"),
        ("Cc", "copy@example.test"),
        ("Bcc", "private@example.test"),
    ] {
        let parsed = mailparse::addrparse(&wire.headers.get_first_value(header).unwrap()).unwrap();
        assert_eq!(parsed.count_addrs(), 1);
        assert_eq!(parsed.extract_single_info().unwrap().addr, address);
    }
    assert_eq!(wire.subparts.len(), 2);
    assert_eq!(
        wire.subparts[0].get_body().unwrap().trim_end(),
        "Frozen send body"
    );
    assert_eq!(
        wire.subparts[1].get_body_raw().unwrap(),
        b"\0\xff\x10\r\nopaque\n"
    );
    let desired: serde_json::Value =
        serde_json::from_str(op["desired_state_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        desired["message_id"].as_str().unwrap(),
        sends[0].message_id.trim_matches(['<', '>'])
    );
    let attempts = h
        .cli(&[
            "--json",
            "operation",
            "attempts",
            "--account",
            &account,
            "--operation",
            op["id"].as_str().unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(attempts.status, 0);
    let attempts = attempts.json().unwrap();
    assert_eq!(attempts["result"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(attempts["result"]["items"][0]["outcome"], "applied");
    assert_eq!(
        attempts["result"]["items"][0]["receipts"][0]["provider_id"],
        sends[0].id
    );
    assert_eq!(
        attempts["result"]["items"][0]["receipts"][0]["source"],
        "acknowledgement"
    );
    h.force_kill().await.unwrap();
    h.restart().await.unwrap();
    let repeated = h.cli(&command).await.unwrap();
    assert_eq!(repeated.status, 0);
    assert_eq!(repeated.json().unwrap()["result"], op);
    assert_eq!(
        h.google
            .control()
            .accepted_sends("alpha@example.test")
            .await
            .len(),
        1
    );
    assert!(h
        .google
        .control()
        .accepted_sends("beta@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}
