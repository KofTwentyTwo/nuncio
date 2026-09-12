#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "support/backup_system.rs"]
mod backup_system;
#[path = "support/imap_effects.rs"]
mod imap_effects;
mod imap_read_system;
mod imap_write_system;
#[path = "support/smtp_resolution_evidence.rs"]
mod smtp_resolution_evidence;
#[allow(dead_code)]
mod support;

use nuncio_proto::v2;
use nuncio_test_support::{google::Seed, imap::MockMailPlus, TestError};
use serde_json::json;
use support::system::SystemHarness;

async fn configuration(
    mock: &MockMailPlus,
    start_tls: bool,
    user: &str,
) -> Result<v2::ImapAccountConfig, TestError> {
    Ok(v2::ImapAccountConfig {
        address: user.into(),
        imap: Some(v2::MailEndpoint {
            host: "127.0.0.1".into(),
            port: u32::from(if start_tls {
                mock.ready.ports.imap
            } else {
                mock.ready.ports.imaps
            }),
            tls: if start_tls {
                v2::MailTls::StartTls
            } else {
                v2::MailTls::Implicit
            }
            .into(),
            username: user.into(),
        }),
        smtp: Some(v2::MailEndpoint {
            host: "127.0.0.1".into(),
            port: u32::from(if start_tls {
                mock.ready.ports.smtp
            } else {
                mock.ready.ports.smtps
            }),
            tls: if start_tls {
                v2::MailTls::StartTls
            } else {
                v2::MailTls::Implicit
            }
            .into(),
            username: user.into(),
        }),
        sent_policy: v2::SentPolicy::ClientAppend.into(),
        sent_folder: "Sent".into(),
        archive_folder: Some("Archive".into()),
        trash_folder: Some("Trash".into()),
        trusted_ca_pem: Some(tokio::fs::read_to_string(&mock.ready.ca_file).await?),
    })
}
async fn request(
    mock: &MockMailPlus,
    tls: bool,
    user: &str,
) -> Result<v2::ConnectImapRequest, TestError> {
    let password = mock.credential(user)?;
    Ok(v2::ConnectImapRequest {
        config: Some(configuration(mock, tls, user).await?),
        credentials: Some(v2::ImapCredentials {
            imap_password: password.clone(),
            smtp_password: password,
        }),
        account_id: None,
    })
}

#[tokio::test]
async fn google_and_imap_share_two_network_slots_and_preserve_remote_state() -> Result<(), TestError>
{
    use nuncio_test_support::google::{Fault, FaultAction, Phase};
    use support::auth::{begin, finish};
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let alpha = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let auth = begin(&h, "beta@example.test", None).await;
    let callback = support::auth::consent(&auth.browser_url).await;
    let imap = h
        .accounts()
        .connect_imap(request(&mock, false, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .unwrap()
        .id;
    let before = mock
        .control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
        .await?;
    mock.control(json!({"command":"inject","name":"resource-fetch","protocol":"imap","verb":"UID FETCH","phase":"before","action":"withhold"})).await?;
    let run = h
        .authenticated()
        .start_sync(v2::StartSyncRequest {
            account_id: imap.clone(),
            full: true,
            fetch_message_id: None,
        })
        .await?
        .into_inner();
    mock.control(json!({"command":"wait_fault","name":"resource-fetch"}))
        .await?;
    let control = h.google.control();
    let beta_checks = control
        .snapshot()
        .await
        .requests
        .iter()
        .filter(|r| r.account.as_deref() == Some("beta@example.test") && r.path == "/token")
        .map(|r| r.count)
        .sum::<u64>();
    control
        .inject(Fault {
            method: "POST".into(),
            path: "/calendar/v3/freeBusy".into(),
            account: Some("alpha@example.test".into()),
            call: Some(1),
            phase: Phase::Before,
            action: FaultAction::Withhold {
                barrier: "shared-network".into(),
            },
        })
        .await;
    let mut calendar = h.calendar();
    let busy = tokio::spawn(async move {
        calendar
            .query_free_busy(v2::FreeBusyRequest {
                account_id: alpha,
                from: "2026-10-02T00:00:00Z".into(),
                to: "2026-10-04T00:00:00Z".into(),
                time_zone: "UTC".into(),
                provider_calendar_ids: vec!["primary".into()],
            })
            .await
    });
    control.wait_for_barrier("shared-network").await?;
    let check = tokio::spawn(async move { support::auth::browser().get(callback).send().await });
    let held = tokio::time::timeout(std::time::Duration::from_millis(750), async {
        loop {
            let status = h
                .authenticated()
                .get_status(v2::GetStatusRequest {})
                .await?
                .into_inner();
            let resources = status.resources.unwrap();
            if resources.requests_waiting == 1 {
                return Ok::<_, TestError>(resources);
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await??;
    assert_eq!(held.requests_active, 2);
    assert_eq!(held.requests_peak, 2);
    assert_eq!(
        control
            .snapshot()
            .await
            .requests
            .iter()
            .filter(|r| r.account.as_deref() == Some("beta@example.test") && r.path == "/token")
            .map(|r| r.count)
            .sum::<u64>(),
        beta_checks
    );
    mock.control(json!({"command":"release","name":"resource-fetch"}))
        .await?;
    control.release_barrier("shared-network").await;
    assert!(busy.await??.into_inner().complete);
    assert_eq!(check.await??.status(), 200);
    assert!(h
        .accounts()
        .get_auth_status(v2::GetAuthStatusRequest {
            session_id: auth.session_id
        })
        .await?
        .into_inner()
        .account_id
        .is_some());
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let current = h
                .authenticated()
                .get_sync_run(v2::SyncRunRequest {
                    account_id: imap.clone(),
                    run_id: run.id.clone(),
                })
                .await?
                .into_inner();
            if current.state == "succeeded" {
                return Ok::<_, TestError>(());
            }
            assert!(
                matches!(current.state.as_str(), "queued" | "running"),
                "{current:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    let after = h
        .authenticated()
        .get_status(v2::GetStatusRequest {})
        .await?
        .into_inner()
        .resources
        .unwrap();
    assert_eq!(after.requests_active, 0);
    assert_eq!(after.requests_waiting, 0);
    assert_eq!(after.requests_peak, 2);
    assert!(after.bytes_received > held.bytes_received);
    assert!(after.storage_page_batches > 0);
    assert_eq!(
        mock.control(json!({"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}))
            .await?,
        before
    );
    assert_eq!(
        mock.control(json!({"command":"snapshot"})).await?["smtp_deliveries"],
        json!([])
    );
    let remote = control.snapshot().await;
    for mailbox in remote.mail.values() {
        assert!(mailbox.accepted_sends.is_empty());
        assert_eq!(mailbox.message_copies, 0);
    }
    for calendars in remote.calendars.values() {
        for calendar in calendars.values() {
            assert!(calendar.notifications.is_empty());
        }
    }
    if let Some(directory) = std::env::var_os("NUNCIO_TEST_ARTIFACTS") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("shared-network-resources.json"),
            serde_json::to_vec_pretty(&json!({"held":held,"after":after}))?,
        )?;
    }
    h.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn imap_account_authenticates_both_tls_servers_and_reopens_without_sending(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&["MOVE", "CONDSTORE", "QRESYNC"]).await?;
    let mut harness = SystemHarness::start(Seed::TwoAccounts).await?;
    assert_eq!(
        harness
            .anonymous_accounts()
            .connect_imap(request(&mock, true, "alpha@example.test").await?)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let result = harness
        .accounts()
        .connect_imap(request(&mock, true, "alpha@example.test").await?)
        .await;
    if result.is_err() {
        eprintln!(
            "Provider observation: {}",
            mock.control(json!({"command":"snapshot"})).await?
        );
    }
    let result = result?.into_inner();
    let alpha = result.account.ok_or("missing account")?;
    assert_eq!(
        (alpha.provider.as_str(), alpha.state.as_str()),
        ("imap", "connected")
    );
    let caps = result.capabilities.ok_or("missing capabilities")?;
    assert!(!caps.move_messages && !caps.condstore && !caps.qresync);
    assert!(caps.uidplus && caps.idle);
    assert!(!result.credential_cleanup_pending);
    let beta = harness
        .accounts()
        .connect_imap(request(&mock, false, "beta@example.test").await?)
        .await?
        .into_inner()
        .account
        .ok_or("missing beta")?;
    assert_ne!(alpha.id, beta.id);
    for account in [&alpha, &beta] {
        assert_eq!(
            harness
                .accounts()
                .check_account(v2::AccountRequest {
                    account_id: account.id.clone()
                })
                .await?
                .into_inner()
                .state,
            "connected"
        );
    }
    let mut mismatch = request(&mock, true, "beta@example.test").await?;
    mismatch.account_id = Some(alpha.id.clone());
    assert_eq!(
        harness
            .accounts()
            .connect_imap(mismatch)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    harness.shutdown().await?;
    harness.restart().await?;
    assert_eq!(
        harness
            .accounts()
            .list_accounts(v2::ListAccountsRequest {})
            .await?
            .into_inner()
            .accounts
            .len(),
        2
    );
    assert_eq!(
        harness
            .accounts()
            .check_account(v2::AccountRequest {
                account_id: alpha.id.clone()
            })
            .await?
            .into_inner()
            .id,
        alpha.id
    );
    let local = harness
        .accounts()
        .get_imap_config(v2::AccountRequest {
            account_id: alpha.id.clone(),
        })
        .await?
        .into_inner();
    assert_eq!(local.config.unwrap().imap.unwrap().host, "127.0.0.1");
    for transport in ["starttls", "tls"] {
        assert_eq!(
            mock.control(json!({"command":"smtp","transport":transport}))
                .await?["total"],
            0
        );
    }
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert_eq!(snapshot["smtp_deliveries"], json!([]));
    harness
        .accounts()
        .disconnect_account(v2::AccountRequest {
            account_id: alpha.id.clone(),
        })
        .await?;
    assert_eq!(
        harness
            .accounts()
            .check_account(v2::AccountRequest {
                account_id: alpha.id
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    harness.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn account_setup_rejects_bad_trust_and_credentials_without_persisting_or_sending(
) -> Result<(), TestError> {
    let mut mock = MockMailPlus::start(&[]).await?;
    let mut harness = SystemHarness::start(Seed::TwoAccounts).await?;
    for starttls in [false, true] {
        let mut untrusted = request(&mock, starttls, "alpha@example.test").await?;
        untrusted.config.as_mut().unwrap().trusted_ca_pem = None;
        assert_eq!(
            harness
                .accounts()
                .connect_imap(untrusted)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::FailedPrecondition
        );
    }
    let snapshot = mock.control(json!({"command":"snapshot"})).await?;
    assert!(snapshot["requests"].get("imap AUTHENTICATE").is_none());
    for smtp in [false, true] {
        let mut bad = request(&mock, true, "alpha@example.test").await?;
        let value = bad.credentials.as_mut().unwrap();
        if smtp {
            value.smtp_password = "synthetic-wrong-canary".into();
        } else {
            value.imap_password = "synthetic-wrong-canary".into();
        }
        let error = harness.accounts().connect_imap(bad).await.unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unauthenticated);
        assert!(!error.message().contains("canary"));
    }
    assert!(harness
        .accounts()
        .list_accounts(v2::ListAccountsRequest {})
        .await?
        .into_inner()
        .accounts
        .is_empty());
    harness
        .secrets
        .fail_put
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        harness
            .accounts()
            .connect_imap(request(&mock, true, "alpha@example.test").await?)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Internal
    );
    assert!(harness
        .accounts()
        .list_accounts(v2::ListAccountsRequest {})
        .await?
        .into_inner()
        .accounts
        .is_empty());
    let connected = harness
        .accounts()
        .connect_imap(request(&mock, true, "alpha@example.test").await?)
        .await?
        .into_inner()
        .account
        .unwrap();
    for (protocol, command) in [("imap", "AUTHENTICATE"), ("smtp", "AUTH")] {
        mock.control(json!({"command":"inject","name":protocol,"protocol":protocol,"verb":command,"phase":"before","action":"reject"})).await?;
        let result = harness
            .accounts()
            .check_account(v2::AccountRequest {
                account_id: connected.id.clone(),
            })
            .await
            .unwrap_err();
        assert_eq!(
            result.code(),
            tonic::Code::Unavailable,
            "temporary rejection must preserve saved credentials"
        );
        assert_eq!(
            harness
                .accounts()
                .list_accounts(v2::ListAccountsRequest {})
                .await?
                .into_inner()
                .accounts[0]
                .state,
            "connected"
        );
        assert_eq!(
            harness
                .accounts()
                .check_account(v2::AccountRequest {
                    account_id: connected.id.clone()
                })
                .await?
                .into_inner()
                .id,
            connected.id
        );
    }
    assert_eq!(mock.control(json!({"command":"smtp"})).await?["total"], 0);
    harness.shutdown().await?;
    harness.restart().await?;
    assert_eq!(
        harness
            .accounts()
            .check_account(v2::AccountRequest {
                account_id: connected.id
            })
            .await?
            .into_inner()
            .state,
        "connected"
    );
    harness.shutdown().await?;
    mock.shutdown().await?;
    Ok(())
}
