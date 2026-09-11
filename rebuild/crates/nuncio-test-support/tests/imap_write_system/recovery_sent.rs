use super::recovery::{reconcile, stable};
use super::*;
use crate::{
    backup_system::{backup, upload},
    imap_effects::effects,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mailparse::MailHeaderMap;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn restored_accepted_smtp_observes_later_sent_copy_without_repeating_delivery_or_append(
) -> Result<(), TestError> {
    for server_sent in [true, false] {
        let mut mock = MockMailPlus::start_with_sent_policy(&[], server_sent).await?;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
        let mut connect = request(&mock, true, "alpha@example.test").await?;
        connect.config.as_mut().unwrap().sent_policy = if server_sent {
            v2::SentPolicy::Server
        } else {
            v2::SentPolicy::ClientAppend
        } as i32;
        let account = h
            .accounts()
            .connect_imap(connect.clone())
            .await?
            .into_inner()
            .account
            .unwrap()
            .id;
        sync(&h, &mut mock, &account, true).await?;
        let draft=h.mail().save_draft(v2::SaveDraftRequest{account_id:account.clone(),draft_id:None,expected_version:None,content:Some(serde_json::from_value(json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"alpha@example.test"}],"subject":"Restored accepted SMTP and independent Sent copy","text":"Exact frozen Sent bytes including private Bcc"}))?)}).await?.into_inner();
        h.arm("operation_after_smtp_acceptance")?;
        let queued = h
            .mail()
            .send_draft(v2::SendDraftRequest {
                account_id: account.clone(),
                draft_id: draft.id,
                expected_version: Some(draft.version),
                request_id: "dd5da7b3-b05d-45da-99d4-cba154c21555".into(),
            })
            .await?
            .into_inner();
        h.wait("operation_after_smtp_acceptance").await?;
        if server_sent {
            mock.control(json!({"command":"wait_server_sent","count":1}))
                .await?;
        }
        let bytes = backup(&h).await;
        let at_snapshot = effects(&mut mock).await?;
        assert_eq!(at_snapshot["writes"]["smtp DATA"], json!([1, 1]));
        assert_eq!(at_snapshot["writes"]["imap APPEND"], json!([0, 0]));
        if !server_sent {
            assert!(at_snapshot["mailboxes"]["Sent"]["messages"]
                .as_array()
                .unwrap()
                .is_empty());
            mock.control(json!({"command":"inject","name":"sent-ack-lost","protocol":"imap","verb":"APPEND","phase":"after","action":"disconnect"})).await?;
        }
        h.release("operation_after_smtp_acceptance")?;
        stable(
            &h,
            &account,
            &queued.id,
            if server_sent { "applied" } else { "uncertain" },
        )
        .await?;
        let report = h
            .maintenance()
            .restore_backup(upload(&bytes))
            .await?
            .into_inner();
        assert_eq!(report.held_operations, 1);
        h.shutdown().await?;
        h.directory = report.directory.into();
        h.restart().await?;
        connect.account_id = Some(account.clone());
        assert_eq!(
            h.accounts()
                .connect_imap(connect)
                .await?
                .into_inner()
                .account
                .unwrap()
                .id,
            account
        );
        let held = stable(&h, &account, &queued.id, "uncertain").await?;
        let before = effects(&mut mock).await?;
        assert_eq!(before["writes"]["smtp DATA"], json!([1, 1]));
        assert_eq!(
            before["writes"]["imap APPEND"],
            json!([u64::from(!server_sent), u64::from(!server_sent)])
        );
        assert_eq!(
            before["mailboxes"]["Sent"]["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let sent = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":1}),
            )
            .await?;
        let raw = STANDARD.decode(sent["raw_base64"].as_str().unwrap())?;
        let parsed = mailparse::parse_mail(&raw)?;
        let bcc = mailparse::addrparse(&parsed.headers.get_first_value("Bcc").unwrap())?;
        assert_eq!(bcc.len(), 1);
        assert!(
            matches!(&bcc[0],mailparse::MailAddr::Single(address) if address.addr=="alpha@example.test")
        );
        let delivered = mock.control(json!({"command":"smtp"})).await?;
        assert_eq!(delivered["total"], 1);
        let wire=mock.control(json!({"command":"smtp_raw","transport":"starttls","id":delivered["messages"][0]["ID"]})).await?;
        let wire = STANDARD.decode(wire["raw_base64"].as_str().unwrap())?;
        let size = before["deliveries"][0]["wire_size"].as_u64().unwrap() as usize;
        assert!(wire.len() >= size);
        let submitted = &wire[wire.len() - size..];
        assert_eq!(
            format!("{:x}", Sha256::digest(submitted)),
            before["deliveries"][0]["wire_sha256"]
        );
        assert!(mailparse::parse_mail(submitted)?
            .headers
            .get_first_value("Bcc")
            .is_none());
        let (input, applied) =
            reconcile(&h, &held, v2::ReconciliationMode::Observe, "applied").await?;
        assert_eq!(effects(&mut mock).await?, before);
        let attempts = h
            .operations()
            .list_attempts(v2::ListOperationAttemptsRequest {
                account_id: account.clone(),
                operation_id: queued.id.clone(),
                page_size: 100,
                page_token: None,
            })
            .await?
            .into_inner()
            .items;
        assert_eq!(attempts.len(), 2);
        let original = attempts.iter().find(|a| a.ordinal == 1).unwrap();
        assert_eq!(original.receipts.len(), 1);
        assert_eq!(original.receipts[0].kind, "smtp_accepted");
        assert_eq!(original.receipts[0].source, "acknowledgement");
        let observed = attempts.iter().find(|a| a.ordinal == 2).unwrap();
        assert_eq!(observed.kind, "reconcile");
        assert_eq!(observed.receipts.len(), if server_sent { 2 } else { 1 });
        assert!(observed
            .receipts
            .iter()
            .all(|r| r.source == "positive_read"));
        if server_sent {
            assert!(observed
                .receipts
                .iter()
                .any(|r| r.kind == "server_sent_observed"));
        }
        assert!(observed.receipts.iter().any(|r| r.kind == "sent_copy"));
        let mail = list(&h, &account).await?;
        assert_eq!(
            mail.items
                .iter()
                .filter(|m| m.subject.as_deref()
                    == Some("Restored accepted SMTP and independent Sent copy"))
                .count(),
            1
        );
        assert_eq!(applied.request_id, queued.request_id);
        h.shutdown().await?;
        h.restart().await?;
        assert_eq!(
            serde_json::to_value(
                h.operations()
                    .reconcile_operation(input)
                    .await?
                    .into_inner()
            )?,
            serde_json::to_value(applied)?
        );
        assert_eq!(effects(&mut mock).await?, before);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn manual_client_sent_observation_requires_unique_faithful_current_copy(
) -> Result<(), TestError> {
    for scenario in [
        "exact",
        "absent",
        "duplicate",
        "missing_bcc",
        "changed_epoch",
    ] {
        let mut mock = MockMailPlus::start(&[]).await?;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
        let account = h
            .accounts()
            .connect_imap(request(&mock, true, "alpha@example.test").await?)
            .await?
            .into_inner()
            .account
            .unwrap()
            .id;
        sync(&h, &mut mock, &account, true).await?;
        let draft=h.mail().save_draft(v2::SaveDraftRequest{account_id:account.clone(),draft_id:None,expected_version:None,content:Some(serde_json::from_value(json!({"to":[{"address":"beta@example.test"}],"bcc":[{"address":"alpha@example.test"}],"subject":"Exact Sent identity","text":"Frozen private copy"}))?)}).await?.into_inner();
        mock.control(json!({"command":"inject","name":"copy-ack-lost","protocol":"imap","verb":"APPEND","phase":"after","action":"disconnect"})).await?;
        let op = h
            .mail()
            .send_draft(v2::SendDraftRequest {
                account_id: account.clone(),
                draft_id: draft.id,
                expected_version: Some(draft.version),
                request_id: "158ef1bb-873f-4eeb-8da8-1e7a5e0a7e7b".into(),
            })
            .await?
            .into_inner();
        let held = stable(&h, &account, &op.id, "uncertain").await?;
        assert_eq!(
            held.error_code.as_deref(),
            Some("imap_sent_copy_identity_unknown")
        );
        let raw = mock
            .control(
                json!({"command":"raw","account":"alpha@example.test","mailbox":"Sent","uid":1}),
            )
            .await?;
        let raw = STANDARD.decode(raw["raw_base64"].as_str().unwrap())?;
        if matches!(scenario, "absent" | "missing_bcc") {
            mock.control(json!({"command":"expunge","account":"alpha@example.test","mailbox":"Sent","uid":1})).await?;
        }
        if matches!(scenario, "duplicate" | "missing_bcc") {
            let bytes = if scenario == "missing_bcc" {
                String::from_utf8(raw.clone())?
                    .lines()
                    .filter(|line| !line.starts_with("Bcc:"))
                    .collect::<Vec<_>>()
                    .join("\r\n")
                    + "\r\n"
            } else {
                String::from_utf8(raw.clone())?
            };
            mock.control(json!({"command":"append","account":"alpha@example.test","mailbox":"Sent","raw_base64":STANDARD.encode(bytes)})).await?;
        }
        if scenario == "changed_epoch" {
            let mailbox = mock
                .control(
                    json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Sent"}),
                )
                .await?;
            mock.control(json!({"command":"uidvalidity","account":"alpha@example.test","mailbox":"Sent","value":mailbox["uidvalidity"].as_u64().unwrap()+1})).await?;
        }
        let before = effects(&mut mock).await?;
        assert_eq!(before["writes"]["smtp DATA"], json!([1, 1]));
        assert_eq!(before["writes"]["imap APPEND"], json!([1, 1]));
        let (input, observed) = reconcile(
            &h,
            &held,
            v2::ReconciliationMode::Observe,
            if scenario == "exact" {
                "applied"
            } else {
                "uncertain"
            },
        )
        .await?;
        if scenario != "exact" {
            assert_eq!(
                observed.error_code.as_deref(),
                Some(match scenario {
                    "duplicate" => "imap_client_sent_ambiguous",
                    "changed_epoch" => "imap_sent_epoch_changed",
                    _ => "imap_client_sent_unconfirmed",
                }),
                "{scenario}"
            );
            let (_, held_again) = reconcile(
                &h,
                &observed,
                v2::ReconciliationMode::ResumeSafe,
                "uncertain",
            )
            .await?;
            assert_eq!(held_again.error_code, observed.error_code);
        }
        assert_eq!(effects(&mut mock).await?, before, "{scenario}");
        let history = h
            .operations()
            .list_attempts(v2::ListOperationAttemptsRequest {
                account_id: account.clone(),
                operation_id: op.id.clone(),
                page_size: 100,
                page_token: None,
            })
            .await?
            .into_inner()
            .items;
        assert_eq!(
            history
                .iter()
                .flat_map(|a| &a.receipts)
                .filter(|r| r.kind == "smtp_accepted" && r.source == "acknowledgement")
                .count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .flat_map(|a| &a.receipts)
                .filter(|r| r.kind == "sent_copy" && r.source == "positive_read")
                .count(),
            usize::from(scenario == "exact")
        );
        assert!(history
            .iter()
            .flat_map(|a| &a.receipts)
            .all(|r| r.kind != "imap_append"));
        h.shutdown().await?;
        h.restart().await?;
        let replay = h
            .operations()
            .reconcile_operation(input)
            .await?
            .into_inner();
        assert_eq!(replay.id, op.id);
        assert_eq!(effects(&mut mock).await?, before);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
