use super::*;

fn repair_request(account: &str, dry_run: bool) -> v2::RepairProjectionRequest {
    v2::RepairProjectionRequest {
        account_id: account.into(),
        scope: v2::ProjectionScope::Mail.into(),
        dry_run,
        window: None,
    }
}
fn unchanged(actual: v2::ListMailResponse, expected: &v2::ListMailResponse) {
    assert!(actual.revision >= expected.revision);
    assert_eq!(actual.items, expected.items);
    assert_eq!(actual.coverage, expected.coverage);
    assert_eq!(actual.next_page_token, expected.next_page_token);
}
async fn repair(h: &SystemHarness, account: &str) -> Result<v2::SyncRun, TestError> {
    let mut run = h
        .maintenance()
        .repair_projection(repair_request(account, false))
        .await?
        .into_inner()
        .run
        .ok_or("repair run missing")?;
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while matches!(run.state.as_str(), "queued" | "running") {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            run = h
                .authenticated()
                .get_sync_run(v2::SyncRunRequest {
                    account_id: account.into(),
                    run_id: run.id,
                })
                .await?
                .into_inner();
        }
        Ok::<_, TestError>(run)
    })
    .await?
}

#[tokio::test]
async fn imap_repair_is_scoped_read_only_and_atomic_on_independent_literal_failure(
) -> Result<(), TestError> {
    for start_tls in [false, true] {
        let mut mock = MockMailPlus::start(&[]).await?;
        let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
        let mut accounts = Vec::new();
        for user in ["alpha@example.test", "beta@example.test"] {
            let id = h
                .accounts()
                .connect_imap(request(&mock, start_tls, user).await?)
                .await?
                .into_inner()
                .account
                .ok_or("account missing")?
                .id;
            assert_eq!(repair(&h, &id).await?.state, "succeeded");
            accounts.push(id);
        }
        let account = &accounts[0];
        let beta = list(&h, &accounts[1]).await?;
        let original = list(&h, account).await?;
        let requests = mock.control(json!({"command":"snapshot"})).await?["requests"].clone();
        let preview = h
            .maintenance()
            .repair_projection(repair_request(account, true))
            .await?
            .into_inner();
        assert_eq!(preview.revision, original.revision);
        let counts = preview.projection.ok_or("projection missing")?;
        assert_eq!(counts.messages, 2);
        assert_eq!(counts.attachments, 1);
        assert_eq!(counts.search_entries, 2);
        assert!(preview.run.is_none());
        for scope in [0, 999, v2::ProjectionScope::Calendar as i32] {
            let mut invalid = repair_request(account, false);
            invalid.scope = scope;
            assert_eq!(
                h.maintenance()
                    .repair_projection(invalid)
                    .await
                    .unwrap_err()
                    .code(),
                tonic::Code::InvalidArgument
            );
        }
        let mut invalid = repair_request(account, false);
        invalid.window = Some(v2::AgendaWindow {
            from: "2026-03-01".into(),
            to: "2026-03-02".into(),
        });
        assert_eq!(
            h.maintenance()
                .repair_projection(invalid)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
        assert_eq!(
            h.maintenance()
                .repair_projection(repair_request(
                    "723dc86b-bf06-44b4-a3ce-85a7ae5f2a1d",
                    false
                ))
                .await
                .unwrap_err()
                .code(),
            tonic::Code::NotFound
        );
        assert_eq!(
            mock.control(json!({"command":"snapshot"})).await?["requests"],
            requests
        );
        mock.control(json!({"command":"copy","account":"alpha@example.test","mailbox":"INBOX","uid":1,"destination":"Archive"})).await?;
        mock.control(
            json!({"command":"expunge","account":"alpha@example.test","mailbox":"INBOX","uid":2}),
        )
        .await?;
        let inbox = mock
            .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?;
        let archive = mock
            .control(
                json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Archive"}),
            )
            .await?;
        mock.control(json!({"command":"inject","name":"repair-literal","protocol":"imap","verb":"UID FETCH","phase":"after","action":"truncate"})).await?;
        assert_eq!(repair(&h, account).await?.state, "failed");
        assert_eq!(
            mock.control(json!({"command":"wait_fault","name":"repair-literal"}))
                .await?["entered"],
            true
        );
        unchanged(list(&h, account).await?, &original);
        unchanged(list(&h, &accounts[1]).await?, &beta);
        h.shutdown().await?;
        h.restart().await?;
        unchanged(list(&h, account).await?, &original);
        assert_eq!(repair(&h, account).await?.state, "succeeded");
        let after = list(&h, account).await?;
        assert_eq!(after.items.len(), 2);
        assert!(after
            .items
            .iter()
            .all(|m| m.subject.as_deref() == Some("Independent MailPlus fixture 0")));
        unchanged(list(&h, &accounts[1]).await?, &beta);
        assert_eq!(
            mock.control(
                json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"})
            )
            .await?,
            inbox
        );
        assert_eq!(
            mock.control(
                json!({"command":"mailbox","account":"alpha@example.test","mailbox":"Archive"})
            )
            .await?,
            archive
        );
        let snapshot = mock.control(json!({"command":"snapshot"})).await?;
        for kind in ["requests", "accepted"] {
            for verb in [
                "smtp DATA",
                "imap APPEND",
                "imap UID COPY",
                "imap UID MOVE",
                "imap UID STORE",
                "imap UID EXPUNGE",
                "imap EXPUNGE",
            ] {
                assert_eq!(
                    snapshot[kind][verb].as_u64().unwrap_or(0),
                    0,
                    "unexpected remote effect {verb}"
                );
            }
        }
        assert_eq!(snapshot["smtp_deliveries"], json!([]));
        assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
        h.shutdown().await?;
        mock.shutdown().await?;
    }
    Ok(())
}
